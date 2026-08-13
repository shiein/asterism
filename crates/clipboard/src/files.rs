use std::collections::HashSet;
use std::fs::{self, FileType};
use std::path::{Path, PathBuf};

use asterism_core::content::{
    FileEntry, FileEntryKind, FileManifest, UnsupportedEntry, UnsupportedReason,
};
use asterism_core::id::ManifestId;
use asterism_core::policy::{LOCAL_MAX_ENUMERATION_ENTRIES, LOCAL_MAX_VISIT_DEPTH};

use crate::error::{ClipboardError, Result};

/// 只读 metadata，不读正文、不算全量 Hash。Symlink/Junction 默认不跟随。
pub fn preflight_paths(paths: &[PathBuf]) -> Result<FileManifest> {
    let root_name = if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into())
    } else {
        format!("{} items", paths.len())
    };

    let mut entries = Vec::new();
    let mut unsupported = Vec::new();
    let mut count = 0u64;
    let mut used_roots = HashSet::new();

    for path in paths {
        let root = unique_name(
            &mut used_roots,
            path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into()),
        );
        visit(path, Path::new(&root), 0, &mut entries, &mut unsupported, &mut count)?;
    }

    Ok(FileManifest { id: ManifestId::new(), root_name, entries, unsupported })
}

fn unique_name(used: &mut HashSet<String>, raw: String) -> String {
    if used.insert(raw.clone()) {
        return raw;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{raw}.{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n = n.saturating_add(1);
        if n == 0 {
            return format!("{raw}.dup");
        }
    }
}

fn visit(
    abs: &Path,
    rel: &Path,
    depth: u32,
    entries: &mut Vec<FileEntry>,
    unsupported: &mut Vec<UnsupportedEntry>,
    count: &mut u64,
) -> Result<()> {
    *count += 1;
    if *count > LOCAL_MAX_ENUMERATION_ENTRIES {
        return Err(ClipboardError::TooManyEntries);
    }
    if depth > LOCAL_MAX_VISIT_DEPTH {
        push_unsupported(rel, UnsupportedReason::Unreadable, unsupported);
        return Ok(());
    }

    let meta = match fs::symlink_metadata(abs) {
        Ok(m) => m,
        Err(_) => {
            push_unsupported(rel, UnsupportedReason::Unreadable, unsupported);
            return Ok(());
        }
    };
    let ft = meta.file_type();
    let relative_path = match asterism_core::content::sanitize_relative_path(&rel.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => {
            push_unsupported(rel, UnsupportedReason::InvalidName, unsupported);
            return Ok(());
        }
    };

    if is_reparse_point(&ft, abs) {
        let reason = if ft.is_symlink() { UnsupportedReason::Symlink } else { UnsupportedReason::Junction };
        push_unsupported(Path::new(&relative_path), reason, unsupported);
        return Ok(());
    }
    if !ft.is_file() && !ft.is_dir() {
        push_unsupported(Path::new(&relative_path), UnsupportedReason::SpecialDevice, unsupported);
        return Ok(());
    }

    if ft.is_dir() {
        entries.push(FileEntry {
            relative_path: relative_path.clone(),
            size: 0,
            kind: FileEntryKind::Directory,
            blob_id: None,
        });
        let read = match fs::read_dir(abs) {
            Ok(rd) => rd,
            Err(_) => {
                push_unsupported(
                    Path::new(&relative_path),
                    UnsupportedReason::Unreadable,
                    unsupported,
                );
                return Ok(());
            }
        };
        for child in read {
            let child = match child {
                Ok(c) => c,
                Err(_) => continue,
            };
            let name = child.file_name();
            let child_rel = Path::new(&relative_path).join(name);
            visit(&child.path(), &child_rel, depth + 1, entries, unsupported, count)?;
        }
        return Ok(());
    }

    entries.push(FileEntry {
        relative_path,
        size: meta.len(),
        kind: FileEntryKind::File,
        blob_id: None,
    });
    Ok(())
}

fn push_unsupported(rel: &Path, reason: UnsupportedReason, out: &mut Vec<UnsupportedEntry>) {
    let relative_path = rel.to_string_lossy().replace('\\', "/");
    out.push(UnsupportedEntry { relative_path, reason });
}

/// 将剪贴板文件快照到本地 cache。不跟随 symlink，避免源文件删除后无法回写剪贴板。
pub fn materialize_to_cache(dest: &Path, sources: &[PathBuf]) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dest)?;
    let mut written = Vec::new();
    let mut used = HashSet::new();
    for src in sources {
        let raw = src
            .file_name()
            .ok_or_else(|| ClipboardError::Platform(format!("file has no name: {}", src.display())))?
            .to_string_lossy()
            .into_owned();
        let name = unique_name(&mut used, raw);
        let target = dest.join(name);
        copy_no_follow(src, &target, 0)?;
        written.push(target);
    }
    Ok(written)
}

fn copy_no_follow(src: &Path, dest: &Path, depth: u32) -> Result<()> {
    if depth > LOCAL_MAX_VISIT_DEPTH {
        return Ok(());
    }
    let meta = fs::symlink_metadata(src)?;
    if is_reparse_point(&meta.file_type(), src) {
        return Ok(());
    }
    if meta.file_type().is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_no_follow(&entry.path(), &dest.join(entry.file_name()), depth + 1)?;
        }
        return Ok(());
    }
    if meta.file_type().is_file() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dest)?;
    }
    Ok(())
}

fn is_reparse_point(ft: &FileType, path: &Path) -> bool {
    if ft.is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{FileTypeExt, MetadataExt};
        if ft.is_symlink_dir() || ft.is_symlink_file() {
            return true;
        }
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if let Ok(meta) = fs::symlink_metadata(path)
            && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_directory_without_following_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();
        let nested = dir.path().join("sub");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("b.txt"), b"there").unwrap();

        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let manifest = preflight_paths(&[dir.path().to_path_buf()]).unwrap();
        assert!(manifest.entries.iter().any(|e| e.relative_path.ends_with("a.txt")));
        assert!(manifest.entries.iter().any(|e| e.relative_path.ends_with("b.txt")));
        #[cfg(unix)]
        assert!(manifest.unsupported.iter().any(|u| u.reason == UnsupportedReason::Symlink));
    }

    #[test]
    fn materialize_copies_files_not_symlinks() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("note.txt"), b"hello").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(src.path().join("note.txt"), src.path().join("link.txt"))
            .unwrap();

        let dest = tempfile::tempdir().unwrap();
        let written = materialize_to_cache(dest.path(), &[src.path().join("note.txt")]).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(std::fs::read(&written[0]).unwrap(), b"hello");
    }

    #[test]
    fn duplicate_root_names_are_disambiguated() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("report.txt"), b"one").unwrap();
        std::fs::write(b.path().join("report.txt"), b"two").unwrap();
        let manifest = preflight_paths(&[a.path().join("report.txt"), b.path().join("report.txt")]).unwrap();
        let names: Vec<_> = manifest.entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert!(names.contains(&"report.txt"));
        assert!(names.iter().any(|n| *n != "report.txt"));
    }
}
