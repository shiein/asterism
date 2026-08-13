//! 平台路径、网络变化感知入口、前台进程推断（Best Effort）。

pub mod atomic;
pub mod hardening;
pub mod identity;
pub mod net;
pub mod paths;
pub mod trust;
pub mod vault;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

pub use identity::LocalIdentity;
pub use net::{LanCandidate, local_candidates, spawn_change_watch};
pub use paths::AppPaths;
pub use trust::{TrustStore, TrustedPeer};
pub use vault::LocalVault;

#[derive(Clone, Debug, Default)]
pub struct ForegroundApp {
    pub name: Option<String>,
    pub identifier: Option<String>,
}

pub fn foreground_app() -> ForegroundApp {
    #[cfg(target_os = "macos")]
    {
        macos::foreground_app()
    }
    #[cfg(windows)]
    {
        windows::foreground_app()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        ForegroundApp::default()
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
