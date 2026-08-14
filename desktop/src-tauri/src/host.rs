use std::sync::Arc;

use asterism_clipboard::SelfWriteGuard;
use asterism_crypto::AccountVaultKey;
use asterism_platform::{AppPaths, LocalIdentity};
use parking_lot::RwLock;

use crate::settings::SyncSettings;

/// 应用数据/缓存路径。不含任意用户文件写权限。
pub struct HostPaths {
    pub paths: AppPaths,
}

/// 剪贴板自写登记与当前文件缓存 pin。
pub struct HostClipboard {
    pub guard: Arc<SelfWriteGuard>,
    pub cache_pin: Arc<RwLock<Option<String>>>,
}

/// 账户与同步配置。仅声明 `credential.account` 的插件可 require。
pub struct HostAccount {
    pub identity: LocalIdentity,
    pub vault: AccountVaultKey,
    pub settings: SyncSettings,
}
