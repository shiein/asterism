use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temporary_path(path);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("json.{}.{}.tmp", std::process::id(), nonce))
}

#[cfg(unix)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp, path)
}

#[cfg(not(unix))]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return fs::rename(temp, path);
    }
    let backup = path.with_extension("json.backup");
    let _ = fs::remove_file(&backup);
    fs::rename(path, &backup)?;
    if let Err(err) = fs::rename(temp, path) {
        let _ = fs::rename(&backup, path);
        return Err(err);
    }
    let _ = fs::remove_file(backup);
    Ok(())
}
