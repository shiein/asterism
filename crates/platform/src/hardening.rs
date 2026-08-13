use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use fs4::fs_std::FileExt;

/// 崩溃恢复：启动时若发现上次未正常退出，清理瞬时 capture 会话目录。
/// 同时对 runtime.lock 加排他锁，避免两个 Desktop 抢同一套 SQLite / Watcher。
pub struct CrashGuard {
    marker: std::path::PathBuf,
    _lock: File,
}

impl CrashGuard {
    pub fn acquire(data_dir: &Path) -> std::io::Result<(Self, bool)> {
        fs::create_dir_all(data_dir)?;
        let marker = data_dir.join("runtime.lock");
        let existed = marker.exists();
        let mut lock =
            OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&marker)?;
        lock.try_lock_exclusive().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "another Asterism desktop instance is already using this data directory",
            )
        })?;
        let unclean = existed && lock.metadata()?.len() > 0;
        if unclean {
            let _ = fs::remove_dir_all(data_dir.join("tmp-capture"));
        }
        lock.set_len(0)?;
        lock.write_all(b"running")?;
        Ok((Self { marker, _lock: lock }, unclean))
    }
}

impl Drop for CrashGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.marker);
    }
}

pub fn write_autostart_plist(label: &str, exe: &Path) -> std::io::Result<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&home).join("Library/LaunchAgents");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{label}.plist"));
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
        exe.display()
    );
    fs::write(&path, body)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_exit_allows_the_next_acquire() {
        let root = std::env::temp_dir()
            .join(format!("asterism-lock-{}", asterism_core::id::DeviceId::new()));
        std::fs::create_dir_all(&root).unwrap();
        let (first, unclean) = CrashGuard::acquire(&root).unwrap();
        assert!(!unclean);
        drop(first);
        let (second, unclean) = CrashGuard::acquire(&root).unwrap();
        assert!(!unclean);
        drop(second);
        let _ = std::fs::remove_dir_all(root);
    }
}
