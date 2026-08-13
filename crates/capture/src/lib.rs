//! Capture / Overlay / Scroll / Annotation export。
//!
//! 业务层只依赖这些类型，不绑定 WGC / ScreenCaptureKit。

pub mod annotation;
pub mod backend;
pub mod overlay;
pub mod scroll;

pub use annotation::{Annotation, AnnotationKind, AnnotationScene, export_png};
pub use backend::{CaptureError, CapturedFrame, MonitorInfo, XcapBackend};
pub use overlay::{FastOverlay, OverlayEvent, Selection};
pub use scroll::{ScrollCaptureEngine, StitchFrame};
