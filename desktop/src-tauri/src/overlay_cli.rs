use asterism_capture::backend::{CapturedFrame, MonitorInfo};
use asterism_capture::{Selection, select_region};

/// 独立进程入口：当前进程没有 Tauri/tao 事件循环，可以安全地跑 winit EventLoop。
pub fn run_overlay_select() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let Some(idx) = args.iter().position(|a| a == "--overlay-select") else {
        eprintln!("missing --overlay-select");
        return 2;
    };
    let path = args.get(idx + 1);
    let width = args.get(idx + 2).and_then(|s| s.parse().ok());
    let height = args.get(idx + 3).and_then(|s| s.parse().ok());
    let (Some(path), Some(width), Some(height)) = (path, width, height) else {
        eprintln!("usage: --overlay-select FRAME.bgra WIDTH HEIGHT");
        return 2;
    };
    let bgra = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("read frame: {err}");
            return 1;
        }
    };
    let frame = CapturedFrame {
        width,
        height,
        bgra,
        monitor: MonitorInfo {
            id: 0,
            name: "overlay".into(),
            origin_physical: (0, 0),
            origin_logical: (0.0, 0.0),
            scale_factor: 1.0,
            capture_size: (width, height),
        },
    };
    match select_region(&frame) {
        Ok(Some(selection)) => match serde_json::to_string(&selection) {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(err) => {
                eprintln!("{err}");
                1
            }
        },
        Ok(None) => {
            println!("{{\"cancelled\":true}}");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub fn select_region_subprocess(frame: &CapturedFrame) -> anyhow::Result<Option<Selection>> {
    let dir = private_overlay_dir()?;
    let frame_path = dir.join("frame.bgra");
    std::fs::write(&frame_path, &frame.bgra)?;
    let exe = std::env::current_exe()?;
    let output = std::process::Command::new(exe)
        .arg("--overlay-select")
        .arg(&frame_path)
        .arg(frame.width.to_string())
        .arg(frame.height.to_string())
        .output()?;
    let _ = std::fs::remove_file(&frame_path);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("overlay process failed: {err}");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.contains("\"cancelled\"") {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(text)?))
}

fn private_overlay_dir() -> anyhow::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("asterism-overlay-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}
