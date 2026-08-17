use std::io::{Read, Write};

use asterism_capture::backend::{CaptureBackend, CapturedFrame, MonitorInfo, XcapBackend};
use asterism_capture::{OverlayOutcome, Selection, select_region_with_windows};
use asterism_kernel::CancelToken;

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
    let windows = XcapBackend.list_windows().unwrap_or_default();
    match select_region_with_windows(&frame, &windows) {
        Ok(Some(outcome)) => match serde_json::to_string(&outcome) {
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
            println!("{{\"action\":\"cancel\"}}");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub fn select_region_subprocess(
    frame: &CapturedFrame,
    cancel: Option<&CancelToken>,
) -> anyhow::Result<Option<Selection>> {
    match select_overlay_subprocess(frame, cancel)? {
        Some(OverlayOutcome::Complete { selection, .. })
        | Some(OverlayOutcome::Download { selection, .. })
        | Some(OverlayOutcome::Pin { selection, .. })
        | Some(OverlayOutcome::Scroll { selection }) => Ok(Some(selection)),
        _ => Ok(None),
    }
}

pub fn select_overlay_subprocess(
    frame: &CapturedFrame,
    cancel: Option<&CancelToken>,
) -> anyhow::Result<Option<OverlayOutcome>> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("--overlay-select")
        .arg(frame.width.to_string())
        .arg(frame.height.to_string())
        .arg(frame.monitor.origin_physical.0.to_string())
        .arg(frame.monitor.origin_physical.1.to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut lease = asterism_kernel::ChildProcessLease::new(child);
    {
        let mut stdin = lease.take_stdin().ok_or_else(|| anyhow::anyhow!("overlay stdin"))?;
        stdin.write_all(&frame.bgra)?;
    }
    let mut stdout = lease.take_stdout().ok_or_else(|| anyhow::anyhow!("overlay stdout"))?;
    let mut stderr = lease.take_stderr().ok_or_else(|| anyhow::anyhow!("overlay stderr"))?;
    loop {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            drop(lease);
            return Ok(None);
        }
        match lease.try_wait()? {
            Some(status) => {
                let mut out = Vec::new();
                let mut err = Vec::new();
                stdout.read_to_end(&mut out)?;
                stderr.read_to_end(&mut err)?;
                if !status.success() {
                    let err = String::from_utf8_lossy(&err);
                    anyhow::bail!("overlay process failed: {err}");
                }
                let text = String::from_utf8_lossy(&out);
                let text = text.trim();
                if text.is_empty() {
                    return Ok(None);
                }
                let outcome: OverlayOutcome = serde_json::from_str(text)?;
                match outcome {
                    OverlayOutcome::Cancel => return Ok(None),
                    other => return Ok(Some(other)),
                }
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}
