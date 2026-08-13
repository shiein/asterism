use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use asterism_core::content::sanitize_relative_path;

use crate::error::{Result, SyncError};

const MAGIC: &[u8; 4] = b"ASF1";

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
}
