use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use asterism_core::content::{FileManifest, sanitize_relative_path};

use crate::error::{Result, SyncError};

const MAGIC: &[u8; 4] = b"ASF1";
const BUNDLE_MAGIC: &[u8; 4] = b"ASB2";

pub fn pack_file_bundle(manifest: &FileManifest, root: &Path) -> Result<Vec<u8>> {
    let manifest_json = serde_json::to_vec(manifest)
        .map_err(|err| SyncError::Protocol(format!("encode file manifest: {err}")))?;
    let manifest_len = u32::try_from(manifest_json.len())
        .map_err(|_| SyncError::Protocol("file manifest is too large".into()))?;
    let archive = pack_tree(root)?;
    let mut out = Vec::with_capacity(8 + manifest_json.len() + archive.len());
    out.extend_from_slice(BUNDLE_MAGIC);
    out.extend_from_slice(&manifest_len.to_le_bytes());
    out.extend_from_slice(&manifest_json);
    out.extend_from_slice(&archive);
    Ok(out)
}

pub fn unpack_file_bundle(bytes: &[u8], dest: &Path) -> Result<(FileManifest, Vec<PathBuf>)> {
    if bytes.len() < 8 || &bytes[..4] != BUNDLE_MAGIC {
        return Err(SyncError::Protocol("bad file bundle magic".into()));
    }
    let manifest_len = u32::from_le_bytes(bytes[4..8].try_into().expect("fixed slice")) as usize;
    let archive_offset =
        8usize
            .checked_add(manifest_len)
            .filter(|offset| *offset <= bytes.len())
            .ok_or_else(|| SyncError::Protocol("truncated file bundle manifest".into()))?;
    let manifest: FileManifest = serde_json::from_slice(&bytes[8..archive_offset])
        .map_err(|err| SyncError::Protocol(format!("decode file manifest: {err}")))?;
    validate_manifest(&manifest)?;
    let roots = unpack_tree(&bytes[archive_offset..], dest)?;
    Ok((manifest, roots))
}

fn validate_manifest(manifest: &FileManifest) -> Result<()> {
    for entry in &manifest.entries {
        sanitize_relative_path(&entry.relative_path)
            .map_err(|err| SyncError::Failed(err.to_string()))?;
    }
    for entry in &manifest.unsupported {
        sanitize_relative_path(&entry.relative_path)
            .map_err(|err| SyncError::Failed(err.to_string()))?;
    }
    Ok(())
}

/// 将缓存目录打成自描述归档，不跟随 symlink。
pub fn pack_tree(root: &Path) -> Result<Vec<u8>> {
    let mut out = Vec::from(*MAGIC);
    let mut entries = Vec::new();
    collect(root, Path::new(""), &mut entries)?;
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (rel, path, is_dir) in entries {
        let name = rel.to_string_lossy().replace('\\', "/");
        let name_b = name.as_bytes();
        out.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        out.extend_from_slice(name_b);
        out.push(u8::from(is_dir));
        if is_dir {
            out.extend_from_slice(&0u64.to_le_bytes());
            continue;
        }
        let bytes = std::fs::read(&path)?;
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

pub fn unpack_tree(bytes: &[u8], dest: &Path) -> Result<Vec<PathBuf>> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(SyncError::Protocol("bad archive magic".into()));
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut i = 8usize;
    let mut roots = Vec::new();
    std::fs::create_dir_all(dest)?;
    for _ in 0..count {
        if i + 3 > bytes.len() {
            return Err(SyncError::Protocol("truncated archive".into()));
        }
        let nlen = u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()) as usize;
        i += 2;
        let name = std::str::from_utf8(&bytes[i..i + nlen])
            .map_err(|e| SyncError::Protocol(e.to_string()))?;
        i += nlen;
        let is_dir = bytes[i] == 1;
        i += 1;
        let size = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap()) as usize;
        i += 8;
        let rel = sanitize_relative_path(name).map_err(|e| SyncError::Failed(e.to_string()))?;
        let path = dest.join(&rel);
        if is_dir {
            std::fs::create_dir_all(&path)?;
        } else {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut f = std::fs::File::create(&path)?;
            f.write_all(&bytes[i..i + size])?;
            i += size;
        }
        if !rel.contains('/') {
            roots.push(path);
        }
    }
    if roots.is_empty() {
        roots.push(dest.to_path_buf());
    }
    Ok(roots)
}

fn collect(abs: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf, bool)>) -> Result<()> {
    let meta = std::fs::symlink_metadata(abs)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() && rel.as_os_str().is_empty() {
        for e in std::fs::read_dir(abs)? {
            let e = e?;
            collect(&e.path(), Path::new(&e.file_name()), out)?;
        }
        return Ok(());
    }
    if meta.is_dir() {
        out.push((rel.to_path_buf(), abs.to_path_buf(), true));
        for e in std::fs::read_dir(abs)? {
            let e = e?;
            collect(&e.path(), &rel.join(e.file_name()), out)?;
        }
    } else if meta.is_file() {
        out.push((rel.to_path_buf(), abs.to_path_buf(), false));
    }
    Ok(())
}

pub fn read_all(mut r: impl Read) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    r.read_to_end(&mut b)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), b"there").unwrap();
        let packed = pack_tree(dir.path()).unwrap();
        let out = tempfile::tempdir().unwrap();
        unpack_tree(&packed, out.path()).unwrap();
        assert_eq!(std::fs::read(out.path().join("a.txt")).unwrap(), b"hi");
        assert_eq!(std::fs::read(out.path().join("sub/b.txt")).unwrap(), b"there");
    }

    #[test]
    fn file_bundle_preserves_manifest_and_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
        let manifest = asterism_core::FileManifest {
            id: asterism_core::ManifestId::new(),
            root_name: "a.txt".into(),
            entries: vec![asterism_core::FileEntry {
                relative_path: "a.txt".into(),
                size: 2,
                kind: asterism_core::FileEntryKind::File,
                blob_id: None,
            }],
            unsupported: Vec::new(),
        };
        let packed = pack_file_bundle(&manifest, dir.path()).unwrap();
        let out = tempfile::tempdir().unwrap();

        let (restored, roots) = unpack_file_bundle(&packed, out.path()).unwrap();

        assert_eq!(restored, manifest);
        assert_eq!(roots, vec![out.path().join("a.txt")]);
        assert_eq!(std::fs::read(&roots[0]).unwrap(), b"hi");
    }
}
