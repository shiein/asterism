//! 无业务语义的运行时微内核。
//!
//! 不依赖 Content、Storage、Crypto、Tauri 或平台 API。

pub mod boot;
pub mod effect;
pub mod error;
pub mod graph;
pub mod health;
pub mod ids;
pub mod plugin;
pub mod registry;
pub mod scope;
pub mod task;

pub use boot::{BootPlan, MountedRuntime};
pub use effect::{EffectGuard, MountGuard};
pub use error::{KernelError, Result};
pub use graph::{PluginNode, resolve_boot_order};
pub use health::{Health, HealthBoard};
pub use ids::validate_plugin_id;
pub use plugin::{KernelManifest, MountContext, Plugin, TrustTier, mount_plugin};
pub use registry::ServiceRegistry;
pub use scope::{CancelToken, Scope, ScopeId};
pub use task::{ChildProcessLease, OsThreadLease, TaskGroup};
