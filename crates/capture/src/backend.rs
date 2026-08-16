use serde::{Deserialize, Serialize};
use thiserror::Error;
use xcap::{Monitor, Window};

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error(
        "screen capture permission denied for the current app process; grant Asterism in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen Asterism"
    )]
    PermissionDenied,
    #[error("capture backend unavailable")]
    Unavailable,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub origin_physical: (i32, i32),
    pub origin_logical: (f64, f64),
    pub scale_factor: f64,
    pub capture_size: (u32, u32),
}

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA。冻结 Overlay 不得先走 PNG 编码。
    pub bgra: Vec<u8>,
    pub monitor: MonitorInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app: String,
    pub size: (u32, u32),
}

pub trait CaptureBackend: Send + Sync {
    fn permission_preflight(&self) -> Result<(), CaptureError>;
    fn list_monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    fn capture_display(&self, monitor: &MonitorInfo) -> Result<CapturedFrame, CaptureError>;
    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;
    fn capture_window(&self, id: u32) -> Result<CapturedFrame, CaptureError>;
}

#[derive(Default)]
pub struct XcapBackend;

impl CaptureBackend for XcapBackend {
    fn permission_preflight(&self) -> Result<(), CaptureError> {
        #[cfg(target_os = "macos")]
        crate::macos_perm::ensure_screen_access()?;
        Monitor::all().map(|_| ()).map_err(|e| {
            let msg = e.to_string();
            if msg.to_ascii_lowercase().contains("permission") || msg.contains("denied") {
                CaptureError::PermissionDenied
            } else {
                CaptureError::Failed(msg)
            }
        })
    }

    fn list_monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let mut monitors = Monitor::all().map_err(|e| CaptureError::Failed(e.to_string()))?;
        monitors.sort_by_key(|monitor| !monitor.is_primary().unwrap_or(false));
        Ok(monitors.iter().map(to_info).collect())
    }

    fn capture_display(&self, monitor: &MonitorInfo) -> Result<CapturedFrame, CaptureError> {
        let found = Monitor::all()
            .map_err(|e| CaptureError::Failed(e.to_string()))?
            .into_iter()
            .find(|m| m.id().ok() == Some(monitor.id))
            .ok_or(CaptureError::Unavailable)?;
        let img = found.capture_image().map_err(|e| CaptureError::Failed(e.to_string()))?;
        let width = img.width();
        let height = img.height();
        let rgba = img.into_raw();
        let mut bgra = vec![0u8; rgba.len()];
        for (src, dst) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        Ok(CapturedFrame { width, height, bgra, monitor: monitor.clone() })
    }

    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        let windows = Window::all().map_err(|e| CaptureError::Failed(e.to_string()))?;
        Ok(windows
            .iter()
            .filter_map(|w| {
                Some(WindowInfo {
                    id: w.id().ok()?,
                    title: w.title().unwrap_or_default(),
                    app: w.app_name().unwrap_or_default(),
                    size: (w.width().unwrap_or(0), w.height().unwrap_or(0)),
                })
            })
            .collect())
    }

    fn capture_window(&self, id: u32) -> Result<CapturedFrame, CaptureError> {
        let found = Window::all()
            .map_err(|e| CaptureError::Failed(e.to_string()))?
            .into_iter()
            .find(|w| w.id().ok() == Some(id))
            .ok_or(CaptureError::Unavailable)?;
        let img = found.capture_image().map_err(|e| CaptureError::Failed(e.to_string()))?;
        let monitor = self.list_monitors()?.into_iter().next().unwrap_or(MonitorInfo {
            id: 0,
            name: "window".into(),
            origin_physical: (0, 0),
            origin_logical: (0.0, 0.0),
            scale_factor: 1.0,
            capture_size: (img.width(), img.height()),
        });
        let width = img.width();
        let height = img.height();
        let rgba = img.into_raw();
        let mut bgra = vec![0u8; rgba.len()];
        for (src, dst) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        Ok(CapturedFrame { width, height, bgra, monitor })
    }
}

fn cursor_point() -> Option<(i32, i32)> {
    #[cfg(target_os = "macos")]
    {
        return crate::macos_perm::cursor_point();
    }
    #[allow(unreachable_code)]
    None
}

/// 优先光标所在屏，否则主屏（list_monitors 已把 primary 排到前面）。
pub fn preferred_monitor(monitors: &[MonitorInfo]) -> Option<&MonitorInfo> {
    if let Some((x, y)) = cursor_point()
        && let Some(hit) = monitors.iter().find(|m| contains_point(m, x, y))
    {
        return Some(hit);
    }
    monitors.first()
}

fn contains_point(monitor: &MonitorInfo, x: i32, y: i32) -> bool {
    let (ox, oy) = monitor.origin_physical;
    let (w, h) = monitor.capture_size;
    x >= ox && y >= oy && x < ox.saturating_add(w as i32) && y < oy.saturating_add(h as i32)
}

fn to_info(m: &Monitor) -> MonitorInfo {
    let id = m.id().unwrap_or(0);
    let name = m.name().unwrap_or_default();
    let x = m.x().unwrap_or(0);
    let y = m.y().unwrap_or(0);
    let w = m.width().unwrap_or(0);
    let h = m.height().unwrap_or(0);
    let scale = m.scale_factor().unwrap_or(1.0) as f64;
    MonitorInfo {
        id,
        name,
        origin_physical: (x, y),
        origin_logical: (x as f64 / scale, y as f64 / scale),
        scale_factor: scale,
        capture_size: (w, h),
    }
}
