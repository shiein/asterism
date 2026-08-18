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

/// 防止 Path Traversal 路径穿透漏洞：确保拼接的相对路径不含 `..`、绝对根路径、盘符或非法字符，
/// 且最终绝对路径必须严格落在 `base_dir` 内部。
pub fn safe_join_relative(base_dir: &Path, relative: &str) -> std::io::Result<std::path::PathBuf> {
    let clean_rel = relative.replace('\\', "/");
    let clean_rel = clean_rel.trim_start_matches('/');
    if clean_rel.is_empty() || clean_rel.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid or empty relative path",
        ));
    }
    for segment in clean_rel.split('/') {
        if segment == ".." || segment == "." || segment.contains(':') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "path traversal attempt detected",
            ));
        }
    }
    let target = base_dir.join(clean_rel);
    if !target.starts_with(base_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path escapes base directory",
        ));
    }
    Ok(target)
}

/// 跨平台开机自启配置
pub fn configure_autostart(label: &str, exe: &Path) -> std::io::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let path = write_autostart_plist(label, exe)?;
        Ok(path.display().to_string())
    }
    #[cfg(windows)]
    {
        write_windows_autostart_registry(label, exe)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        write_linux_autostart_desktop(label, exe)
    }
}

pub fn write_autostart_plist(label: &str, exe: &Path) -> std::io::Result<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&home).join("Library/LaunchAgents");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{label}.plist"));
    let escaped_label = xml_escape(label);
    let escaped_exe = xml_escape(&exe.display().to_string());
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{escaped_label}</string>
  <key>ProgramArguments</key><array><string>{escaped_exe}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#
    );
    fs::write(&path, body)?;
    Ok(path)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn write_windows_autostart_registry(label: &str, exe: &Path) -> std::io::Result<String> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::HSTRING;

    let subkey = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let mut hkey = windows::Win32::System::Registry::HKEY::default();
    unsafe {
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, 0, KEY_SET_VALUE, &mut hkey);
        if status.is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to open registry key: {:?}", status),
            ));
        }
        let val_name = HSTRING::from(label);
        let val_data = format!("\"{}\"", exe.display());
        let val_wide: Vec<u16> = val_data.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(
            val_wide.as_ptr() as *const u8,
            val_wide.len() * std::mem::size_of::<u16>(),
        );
        let set_status = RegSetValueExW(hkey, &val_name, 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        if set_status.is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to set registry value: {:?}", set_status),
            ));
        }
    }
    Ok(format!("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run -> {}", label))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn write_linux_autostart_desktop(label: &str, exe: &Path) -> std::io::Result<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&home).join(".config/autostart");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{label}.desktop"));
    let body = format!(
        "[Desktop Entry]\nType=Application\nName={label}\nExec=\"{}\"\nHidden=false\nNoDisplay=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    fs::write(&path, body)?;
    Ok(path.display().to_string())
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

    #[test]
    fn path_traversal_prevention() {
        let base = Path::new("/var/app/data");
        assert!(safe_join_relative(base, "images/pic.png").is_ok());
        assert_eq!(
            safe_join_relative(base, "images/pic.png").unwrap(),
            Path::new("/var/app/data/images/pic.png")
        );
        assert!(safe_join_relative(base, "../etc/passwd").is_err());
        assert!(safe_join_relative(base, "sub/../../secret").is_err());
        assert!(safe_join_relative(base, "C:\\Windows\\system32").is_err());
        assert!(safe_join_relative(base, "file\0name").is_err());
    }
}
