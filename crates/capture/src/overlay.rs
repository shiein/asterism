use serde::{Deserialize, Serialize};

use crate::backend::{CaptureError, CapturedFrame};

/// Native Fast Overlay：冻结帧、选区、多屏、DPI。不承担复杂标注。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    /// 图片逻辑坐标，不是 CSS。
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub enum OverlayEvent {
    Selected(Selection),
    Cancelled,
}

pub trait FastOverlay: Send {
    fn show_frozen(&mut self, frame: &CapturedFrame) -> Result<(), CaptureError>;
    fn take_event(&mut self) -> Option<OverlayEvent>;
    fn dismiss(&mut self);
}

/// 会话编排：热路径先冻结，再并行唤醒标注 WebView。
pub struct OverlaySession {
    pub frame: CapturedFrame,
    pub selection: Option<Selection>,
}

impl OverlaySession {
    pub fn crop_bgra(&self) -> Option<(u32, u32, Vec<u8>)> {
        let sel = self.selection.as_ref()?;
        let x = sel.x.max(0.0) as u32;
        let y = sel.y.max(0.0) as u32;
        let w = sel.width.max(1.0) as u32;
        let h = sel.height.max(1.0) as u32;
        if x + w > self.frame.width || y + h > self.frame.height {
            return None;
        }
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row * self.frame.width + x) * 4) as usize;
            let end = start + (w * 4) as usize;
            out.extend_from_slice(&self.frame.bgra[start..end]);
        }
        Some((w, h, out))
    }
}
