//! 系统剪贴板：Normalize → Dedup → Policy，然后才允许进入 History / Transport。

pub mod capture;
pub mod error;
pub mod files;
pub mod guard;
pub mod image;
pub mod normalize;
pub mod sensitive;
pub mod watcher;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

pub use capture::{CapturedClipboard, ClipboardBackend, NativeClipboard};
pub use error::ClipboardError;
pub use files::{materialize_to_cache, preflight_paths};
pub use guard::SelfWriteGuard;
pub use normalize::NormalizedContent;
pub use watcher::{ClipboardEvent, WatcherConfig, spawn_watcher};
