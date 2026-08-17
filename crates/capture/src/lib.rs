//! Capture / Overlay / Scroll / Annotation export。
//!
//! 业务层只依赖这些类型，不绑定 WGC / ScreenCaptureKit。

pub mod annotation;
pub mod backend;
pub mod overlay;
pub mod scroll;

#[cfg(target_os = "macos")]
pub mod macos_perm;

pub use annotation::{Annotation, AnnotationKind, AnnotationScene, export_png};
pub use backend::{
    CaptureBackend, CaptureError, CapturedFrame, MonitorInfo, WindowInfo, XcapBackend,
    preferred_monitor,
};
pub use overlay::{
    ActiveTool, FastOverlay, OverlayOutcome, OverlaySession, Selection, select_region,
    select_region_with_windows,
};
pub use scroll::{ScrollCaptureEngine, StitchFrame};
