use std::fs;
use std::path::Path;

/// 崩溃恢复：启动时若发现上次未正常退出，清理瞬时 capture 会话目录。
pub struct CrashGuard {
    marker: std::path::PathBuf,
}

impl CrashGuard {
    pub fn acquire(data_dir: &Path) -> std::io::Result<(Self, bool)> {
        fs::create_dir_all(data_dir)?;
        let marker = data_dir.join("runtime.lock");
        let unclean = marker.exists();
        if unclean {
            let _ = fs::remove_dir_all(data_dir.join("tmp-capture"));
        }
        fs::write(&marker, b"running")?;
        Ok((Self { marker }, unclean))
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
