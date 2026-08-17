use std::io::{Read, Write};
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use std::sync::mpsc;

use asterism_capture::backend::{CaptureBackend, CapturedFrame, MonitorInfo, XcapBackend};
use asterism_capture::{FrameSource, OverlayOutcome, WindowSource, run_overlay};
use asterism_kernel::{CancelToken, ChildProcessLease};

/// 冻结帧走 stdin 的帧头。放在 stdin 而不是命令行参数，
/// 这样父进程可以"先拉起子进程，再送帧"——子进程的启动与屏幕捕获得以重叠。
const FRAME_MAGIC: &[u8; 8] = b"ASTFRM01";
const HEADER_LEN: usize = 8 + 4 + 4 + 4 + 4 + 8;

fn encode_header(frame: &CapturedFrame) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[0..8].copy_from_slice(FRAME_MAGIC);
    header[8..12].copy_from_slice(&frame.width.to_le_bytes());
    header[12..16].copy_from_slice(&frame.height.to_le_bytes());
    header[16..20].copy_from_slice(&frame.monitor.origin_physical.0.to_le_bytes());
    header[20..24].copy_from_slice(&frame.monitor.origin_physical.1.to_le_bytes());
    header[24..32].copy_from_slice(&frame.monitor.scale_factor.to_le_bytes());
    header
}

fn decode_header(header: &[u8; HEADER_LEN]) -> Result<(u32, u32, i32, i32, f64), String> {
    if &header[0..8] != FRAME_MAGIC {
        return Err("bad overlay frame header".into());
    }
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ])
    };
    let i32_at = |offset: usize| {
        i32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ])
    };
    let mut scale = [0u8; 8];
    scale.copy_from_slice(&header[24..32]);
    Ok((u32_at(8), u32_at(12), i32_at(16), i32_at(20), f64::from_le_bytes(scale)))
}

/// 独立进程入口：当前进程没有 Tauri/tao 事件循环，可以安全地跑 winit EventLoop。
///
/// 关键是不要在"显示 overlay"之前做任何阻塞的事：
/// 帧读取与窗口枚举都放到后台线程，事件循环立即启动，
/// 帧一到就建窗口、画完首帧再显示，用户不会看到分两步的过程。
pub fn run_overlay_select() -> i32 {
    let (frame_tx, frame_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = frame_tx.send(read_frame_from_stdin());
    });

    // list_windows 在 macOS 上可能要几百毫秒，不能挡住 overlay 出现。
    let (windows_tx, windows_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = windows_tx.send(XcapBackend.list_windows().unwrap_or_default());
    });

    match run_overlay(FrameSource::Pending(frame_rx), WindowSource::Pending(windows_rx)) {
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

fn read_frame_from_stdin() -> Result<CapturedFrame, String> {
    let mut stdin = std::io::stdin().lock();
    let mut header = [0u8; HEADER_LEN];
    stdin.read_exact(&mut header).map_err(|err| format!("read overlay header: {err}"))?;
    let (width, height, origin_x, origin_y, scale_factor) = decode_header(&header)?;
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "overlay frame too large".to_string())?;
    let mut bgra = vec![0u8; expected];
    stdin.read_exact(&mut bgra).map_err(|err| format!("read overlay frame: {err}"))?;
    Ok(CapturedFrame {
        width,
        height,
        bgra,
        monitor: MonitorInfo {
            id: 0,
            name: "overlay".into(),
            origin_physical: (origin_x, origin_y),
            origin_logical: (
                f64::from(origin_x) / scale_factor,
                f64::from(origin_y) / scale_factor,
            ),
            scale_factor,
            capture_size: (width, height),
        },
    })
}

/// 已启动、正在等待冻结帧的 overlay 子进程。
pub struct OverlayProcess {
    lease: ChildProcessLease,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

/// 立即拉起 overlay 子进程。调用方应当先 spawn、再去做隐藏窗口/屏幕捕获，
/// 让子进程的加载时间被这些必要工作掩盖掉。
pub fn spawn_overlay() -> anyhow::Result<OverlayProcess> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("--overlay-select")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut lease = ChildProcessLease::new(child);
    let stdin = lease.take_stdin();
    let stdout = lease.take_stdout();
    let stderr = lease.take_stderr();
    Ok(OverlayProcess { lease, stdin, stdout, stderr })
}

impl OverlayProcess {
    /// 送入冻结帧。写完立刻关闭 stdin，子进程才能确认帧结束。
    pub fn submit(&mut self, frame: &CapturedFrame) -> anyhow::Result<()> {
        let mut stdin = self.stdin.take().ok_or_else(|| anyhow::anyhow!("overlay stdin closed"))?;
        stdin.write_all(&encode_header(frame))?;
        stdin.write_all(&frame.bgra)?;
        stdin.flush()?;
        Ok(())
    }

    pub fn wait(mut self, cancel: Option<&CancelToken>) -> anyhow::Result<Option<OverlayOutcome>> {
        let mut stdout = self.stdout.take().ok_or_else(|| anyhow::anyhow!("overlay stdout"))?;
        let mut stderr = self.stderr.take().ok_or_else(|| anyhow::anyhow!("overlay stderr"))?;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Ok(None);
            }
            match self.lease.try_wait()? {
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
                    return match outcome {
                        OverlayOutcome::Cancel => Ok(None),
                        other => Ok(Some(other)),
                    };
                }
                // 轮询间隔直接体现在"松手到出图"的手感上，不要放大。
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> CapturedFrame {
        CapturedFrame {
            width: 3,
            height: 2,
            bgra: vec![7u8; 3 * 2 * 4],
            monitor: MonitorInfo {
                id: 1,
                name: "m".into(),
                origin_physical: (-1440, 200),
                origin_logical: (-720.0, 100.0),
                scale_factor: 2.0,
                capture_size: (3, 2),
            },
        }
    }

    #[test]
    fn frame_header_roundtrip_preserves_geometry_and_scale() {
        let header = encode_header(&frame());
        let (w, h, ox, oy, scale) = decode_header(&header).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!((ox, oy), (-1440, 200));
        assert_eq!(scale, 2.0);
    }

    #[test]
    fn rejects_foreign_header() {
        let mut header = encode_header(&frame());
        header[0] = b'X';
        assert!(decode_header(&header).is_err());
    }
}
