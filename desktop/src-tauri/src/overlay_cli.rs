use std::io::{Read, Write};

use asterism_capture::backend::{CapturedFrame, MonitorInfo};
use asterism_capture::{Selection, select_region};

/// 独立进程入口：当前进程没有 Tauri/tao 事件循环，可以安全地跑 winit EventLoop。
/// 冻结帧走 stdin，避免 4K BGRA 落盘。
pub fn run_overlay_select() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let Some(idx) = args.iter().position(|a| a == "--overlay-select") else {
        eprintln!("missing --overlay-select");
        return 2;
    };
    let width = args.get(idx + 1).and_then(|s| s.parse().ok());
    let height = args.get(idx + 2).and_then(|s| s.parse().ok());
    let origin_x = args.get(idx + 3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let origin_y = args.get(idx + 4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let (Some(width), Some(height)) = (width, height) else {
        eprintln!("usage: --overlay-select WIDTH HEIGHT [ORIGIN_X ORIGIN_Y] < frame.bgra");
        return 2;
    };
    let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
    let mut bgra = Vec::new();
    if let Err(err) = std::io::stdin().read_to_end(&mut bgra) {
        eprintln!("read frame: {err}");
        return 1;
    }
    if bgra.len() < expected {
        eprintln!("truncated frame: {} < {expected}", bgra.len());
        return 1;
    }
    bgra.truncate(expected);
    let frame = CapturedFrame {
        width,
        height,
        bgra,
        monitor: MonitorInfo {
            id: 0,
            name: "overlay".into(),
            origin_physical: (origin_x, origin_y),
            origin_logical: (origin_x as f64, origin_y as f64),
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
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new(exe)
        .arg("--overlay-select")
        .arg(frame.width.to_string())
        .arg(frame.height.to_string())
        .arg(frame.monitor.origin_physical.0.to_string())
        .arg(frame.monitor.origin_physical.1.to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| anyhow::anyhow!("overlay stdin"))?;
        stdin.write_all(&frame.bgra)?;
    }
    let output = child.wait_with_output()?;
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
