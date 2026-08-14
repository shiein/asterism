//! 插件契约。不包含 SQLite、密钥字节或 Tauri。

pub mod action;
pub mod grant;
pub mod ids;

pub use action::{ActionDescriptor, ActionRegistry};
pub use asterism_core::{ContentHandle, Provenance};
pub use grant::{
    ContentCommandGrant, ContentReadGrant, HistoryQueryGrant, PermissionBroker, SelectedPathGrant,
};
pub use ids::{ActionKey, PluginManifest, TrustTier};
