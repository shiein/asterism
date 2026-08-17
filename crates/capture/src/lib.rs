//! Capture / Overlay / Scroll / Annotation export。
//!
//! 业务层只依赖这些类型，不绑定 WGC / ScreenCaptureKit。

pub mod annotation;
pub mod backend;
pub mod hud;
pub mod overlay;
pub mod scroll;

#[cfg(target_os = "macos")]
pub mod macos_perm;

pub use annotation::{
    Annotation, AnnotationKind, AnnotationScene, DEFAULT_MOSAIC_BLOCK, GLYPH_HEIGHT, MosaicMask,
    apply_mosaic, draw_annotation, export_png, measure_bitmap_text, mosaic_mask, text_pixel_scale,
};
pub use backend::{
    CaptureBackend, CaptureError, CapturedFrame, MonitorInfo, WindowInfo, XcapBackend,
    preferred_monitor,
};
pub use hud::{Area, Hud, ToolbarAction, ToolbarLayout};
pub use overlay::{
    ActiveTool, FastOverlay, FrameSource, OverlayOutcome, OverlaySession, Selection, WindowSource,
    run_overlay, select_region, select_region_with_windows,
};
pub use scroll::{ScrollCaptureEngine, StitchFrame};
