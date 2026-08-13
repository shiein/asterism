use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::Result;

/// 文件缓存 TTL + LRU。仍被剪贴板引用的 item 由调用方排除。
pub fn evict_item_cache(
    cache_root: &Path,
    ttl: Duration,
    max_bytes: u64,
    pinned: &[String],
) -> Result<u64> {
    let items = cache_root.join("items");
    if !items.exists() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&items)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if pinned.iter().any(|p| p == &name) {
            continue;
        }
        let meta = entry.metadata()?;
        let age = now.duration_since(meta.modified().unwrap_or(now)).unwrap_or_default();
        let size = dir_size(&entry.path())?;
        entries.push((entry.path(), age, size, meta.modified().unwrap_or(now)));
    }
    let mut freed = 0u64;
    let mut total: u64 = entries.iter().map(|e| e.2).sum();
    entries.sort_by_key(|e| e.3);
    for (path, age, size, _) in entries {
        if age > ttl || total > max_bytes {
            let _ = std::fs::remove_dir_all(&path);
            freed += size;
            total = total.saturating_sub(size);
        }
    }
    Ok(freed)
}

fn dir_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut n = 0u64;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            n += dir_size(&e.path())?;
        }
    }
    Ok(n)
}
