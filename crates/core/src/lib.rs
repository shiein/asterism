//! Asterism 域模型。
//!
//! 四个核心抽象中的 Content / Action 以及 Device、Policy 定义在本 crate。
//! 本 crate 不依赖 SQLite、网络或平台 API。

pub mod action;
pub mod builtin_actions;
pub mod content;
pub mod device;
pub mod error;
pub mod id;
pub mod policy;

pub use action::{ActionContext, ActionError, ActionId, ActionResult, ContentAction};
pub use content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, FileEntry, FileEntryKind, FileManifest,
    FileManifestSummary, ImageMeta, ItemMetadata, PayloadRef, UnsupportedEntry, UnsupportedReason,
};
pub use device::{Device, DeviceCapabilities, DevicePlatform};
pub use error::CoreError;
pub use id::{AccountId, BlobId, ContentId, DeviceId, ManifestId};
pub use policy::{
    AppExclusion, CapturePolicy, RemoteLimits, RemotePolicy, SensitiveDecision,
    UniversalClipboardMode,
};
