//! Capture / Overlay / Scroll 边界。
//!
//! V1 Phase 4 才实现具体 backend。业务层只依赖这些 trait，
//! 不绑定 Windows.Graphics.Capture 或 ScreenCaptureKit。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("capture backend unavailable")]
    Unavailable,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub origin_physical: (i32, i32),
    pub origin_logical: (f64, f64),
    pub scale_factor: f64,
    pub capture_size: (u32, u32),
}

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA 原始像素。冻结 Overlay 不得先走 PNG 编码。
    pub bgra: Vec<u8>,
    pub monitor: MonitorInfo,
}

pub trait CaptureBackend: Send + Sync {
    fn permission_preflight(&self) -> Result<(), CaptureError>;
    fn capture_display(&self, monitor: &MonitorInfo) -> Result<CapturedFrame, CaptureError>;
}

/// Native Fast Overlay：冻结帧、选区、多屏、DPI。不承担复杂标注。
pub trait FastOverlay: Send {
    fn show_frozen(&mut self, frame: &CapturedFrame) -> Result<(), CaptureError>;
    fn dismiss(&mut self);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub kind: AnnotationKind,
    /// 图片逻辑坐标，不是 CSS 坐标。
    pub geometry: Vec<f64>,
    pub style: serde_json::Value,
    pub z_index: i32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Brush,
    Text,
    Mosaic,
    Blur,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnnotationScene {
    pub items: Vec<Annotation>,
}

/// 滚动截图独立模块。自动滚动低 confidence 必须停并保留当前结果。
pub trait ScrollCaptureEngine: Send {
    fn start(&mut self) -> Result<(), CaptureError>;
    fn stop_auto_preserve(&mut self) -> Result<CapturedFrame, CaptureError>;
}


