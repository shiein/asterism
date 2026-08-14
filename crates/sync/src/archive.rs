use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use asterism_core::content::{FileManifest, sanitize_relative_path};

use crate::error::{Result, SyncError};

const MAGIC: &[u8; 4] = b"ASF1";
const BUNDLE_MAGIC: &[u8; 4] = b"ASB2";

pub fn pack_file_bundle(manifest: &FileManifest, root: &Path) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    pack_file_bundle_to_writer(manifest, root, &mut out)?;
    Ok(out)
}

pub fn pack_file_bundle_to_writer(
    manifest: &FileManifest,
    root: &Path,
    mut out: impl Write,
) -> Result<u64> {
    let manifest_json = serde_json::to_vec(manifest)
        .map_err(|err| SyncError::Protocol(format!("encode file manifest: {err}")))?;
    let manifest_len = u32::try_from(manifest_json.len())
        .map_err(|_| SyncError::Protocol("file manifest is too large".into()))?;
    out.write_all(BUNDLE_MAGIC)?;
    out.write_all(&manifest_len.to_le_bytes())?;
    out.write_all(&manifest_json)?;
    Ok(8 + manifest_json.len() as u64 + pack_tree_to_writer(root, out)?)
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

pub fn unpack_file_bundle_reader(
    mut input: impl Read,
    dest: &Path,
) -> Result<(FileManifest, Vec<PathBuf>)> {
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != BUNDLE_MAGIC {
        return Err(SyncError::Protocol("bad file bundle magic".into()));
    }
    let manifest_len = read_u32(&mut input)? as usize;
    if manifest_len > 4 * 1024 * 1024 {
        return Err(SyncError::Protocol("file manifest is too large".into()));
    }
    let mut manifest_json = vec![0u8; manifest_len];
    input.read_exact(&mut manifest_json)?;
    let manifest: FileManifest = serde_json::from_slice(&manifest_json)
        .map_err(|err| SyncError::Protocol(format!("decode file manifest: {err}")))?;
    validate_manifest(&manifest)?;
    let roots = unpack_tree_reader(input, dest)?;
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
    let mut out = Vec::new();
    pack_tree_to_writer(root, &mut out)?;
    Ok(out)
}

pub fn pack_tree_to_writer(root: &Path, mut out: impl Write) -> Result<u64> {
    let mut entries = Vec::new();
    collect(root, Path::new(""), &mut entries)?;
    out.write_all(MAGIC)?;
    out.write_all(&(entries.len() as u32).to_le_bytes())?;
    let mut written = 8u64;
    for (rel, path, is_dir) in entries {
        let name = rel.to_string_lossy().replace('\\', "/");
        let name_b = name.as_bytes();
        let name_len = u16::try_from(name_b.len())
            .map_err(|_| SyncError::Protocol("archive path is too long".into()))?;
        out.write_all(&name_len.to_le_bytes())?;
        out.write_all(name_b)?;
        out.write_all(&[u8::from(is_dir)])?;
        written += 3 + name_b.len() as u64;
        if is_dir {
            out.write_all(&0u64.to_le_bytes())?;
            written += 8;
            continue;
        }
        let meta = std::fs::metadata(&path)?;
        const MAX_ARCHIVE_BYTES: u64 = 500 * 1024 * 1024;
        if meta.len() > MAX_ARCHIVE_BYTES || written.saturating_add(meta.len()) > MAX_ARCHIVE_BYTES
        {
            return Err(SyncError::Failed("file tree exceeds remote size limit".into()));
        }
        out.write_all(&meta.len().to_le_bytes())?;
        let mut file = std::fs::File::open(&path)?;
        std::io::copy(&mut file, &mut out)?;
        written += 8 + meta.len();
    }
    Ok(written)
}

fn take<'a>(bytes: &'a [u8], i: &mut usize, n: usize) -> Result<&'a [u8]> {
    let end =
        i.checked_add(n).ok_or_else(|| SyncError::Protocol("archive offset overflow".into()))?;
    let slice =
        bytes.get(*i..end).ok_or_else(|| SyncError::Protocol("truncated archive".into()))?;
    *i = end;
    Ok(slice)
}

pub fn unpack_tree(bytes: &[u8], dest: &Path) -> Result<Vec<PathBuf>> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(SyncError::Protocol("bad archive magic".into()));
    }
    let count = u32::from_le_bytes(
        bytes[4..8].try_into().map_err(|_| SyncError::Protocol("bad count".into()))?,
    ) as usize;
    let mut i = 8usize;
    let mut roots = Vec::new();
    std::fs::create_dir_all(dest)?;
    for _ in 0..count {
        let nlen_bytes = take(bytes, &mut i, 2)?;
        let nlen = u16::from_le_bytes(
            nlen_bytes.try_into().map_err(|_| SyncError::Protocol("name len".into()))?,
        ) as usize;
        let name_bytes = take(bytes, &mut i, nlen)?;
        let name =
            std::str::from_utf8(name_bytes).map_err(|e| SyncError::Protocol(e.to_string()))?;
        let flag = take(bytes, &mut i, 1)?;
        let is_dir = flag[0] == 1;
        let size_bytes = take(bytes, &mut i, 8)?;
        let size = u64::from_le_bytes(
            size_bytes.try_into().map_err(|_| SyncError::Protocol("size".into()))?,
        ) as usize;
        let rel = sanitize_relative_path(name).map_err(|e| SyncError::Failed(e.to_string()))?;
        let path = dest.join(&rel);
        if is_dir {
            std::fs::create_dir_all(&path)?;
        } else {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let payload = take(bytes, &mut i, size)?;
            let mut f = std::fs::File::create(&path)?;
            f.write_all(payload)?;
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

pub fn unpack_tree_reader(mut input: impl Read, dest: &Path) -> Result<Vec<PathBuf>> {
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(SyncError::Protocol("bad archive magic".into()));
    }
    let count = read_u32(&mut input)?;
    let mut roots = Vec::new();
    std::fs::create_dir_all(dest)?;
    let mut extracted = 0u64;
    for _ in 0..count {
        let nlen = read_u16(&mut input)? as usize;
        let mut name_bytes = vec![0u8; nlen];
        input.read_exact(&mut name_bytes)?;
        let name =
            std::str::from_utf8(&name_bytes).map_err(|err| SyncError::Protocol(err.to_string()))?;
        let mut flag = [0u8; 1];
        input.read_exact(&mut flag)?;
        let size = read_u64(&mut input)?;
        let rel = sanitize_relative_path(name).map_err(|err| SyncError::Failed(err.to_string()))?;
        let path = dest.join(&rel);
        if flag[0] == 1 {
            if size != 0 {
                return Err(SyncError::Protocol("directory entry has payload".into()));
            }
            std::fs::create_dir_all(&path)?;
        } else {
            extracted = extracted
                .checked_add(size)
                .filter(|total| *total <= 500 * 1024 * 1024)
                .ok_or_else(|| SyncError::Failed("file tree exceeds remote size limit".into()))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&path)?;
            let copied = std::io::copy(&mut input.by_ref().take(size), &mut file)?;
            if copied != size {
                return Err(SyncError::Protocol("truncated archive payload".into()));
            }
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

fn read_u16(input: &mut impl Read) -> Result<u16> {
    let mut bytes = [0u8; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &mut impl Read) -> Result<u32> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> Result<u64> {
    let mut bytes = [0u8; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
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
    use std::io::Seek;

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

    #[test]
    fn streaming_file_bundle_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("large.bin"), vec![0x7a; 2 * 1024 * 1024]).unwrap();
        let manifest = asterism_core::FileManifest {
            id: asterism_core::ManifestId::new(),
            root_name: "large.bin".into(),
            entries: vec![asterism_core::FileEntry {
                relative_path: "large.bin".into(),
                size: 2 * 1024 * 1024,
                kind: asterism_core::FileEntryKind::File,
                blob_id: None,
            }],
            unsupported: Vec::new(),
        };
        let staged = tempfile::tempfile().unwrap();
        pack_file_bundle_to_writer(&manifest, dir.path(), &staged).unwrap();
        let mut staged = staged;
        staged.rewind().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (restored, _) = unpack_file_bundle_reader(staged, out.path()).unwrap();
        assert_eq!(restored, manifest);
        assert_eq!(std::fs::metadata(out.path().join("large.bin")).unwrap().len(), 2 * 1024 * 1024);
    }

    #[test]
    fn unpack_rejects_truncated_archive() {
        let mut packed = b"ASF1".to_vec();
        packed.extend_from_slice(&1u32.to_le_bytes());
        packed.extend_from_slice(&10u16.to_le_bytes());
        packed.extend_from_slice(b"ab");
        let dest = tempfile::tempdir().unwrap();
        assert!(unpack_tree(&packed, dest.path()).is_err());
    }
}
