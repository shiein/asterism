use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use asterism_core::ContentHandle;
use asterism_core::content::ContentKind;
use asterism_core::id::ContentId;
use asterism_kernel::DeclaredPermissions;

/// Permission 是需求；Grant 是本次调用实际拿到的范围。
#[derive(Clone, Debug)]
pub struct ContentReadGrant {
    content_id: ContentId,
    max_bytes: u64,
    expires_at: Instant,
}

impl ContentReadGrant {
    fn issue(content_id: ContentId, max_bytes: u64, ttl: Duration) -> Self {
        Self { content_id, max_bytes, expires_at: Instant::now() + ttl }
    }

    pub fn content_id(&self) -> ContentId {
        self.content_id
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn is_valid(&self, id: ContentId) -> bool {
        self.content_id == id && Instant::now() < self.expires_at
    }
}

#[derive(Clone, Debug)]
pub struct ContentCommandGrant {
    content_id: ContentId,
    favorite: bool,
    delete: bool,
}

impl ContentCommandGrant {
    pub fn content_id(&self) -> ContentId {
        self.content_id
    }
    pub fn favorite(&self) -> bool {
        self.favorite
    }
    pub fn delete(&self) -> bool {
        self.delete
    }
}

#[derive(Clone, Debug)]
pub struct HistoryQueryGrant {
    _private: (),
}

#[derive(Clone, Debug)]
pub struct SelectedPathGrant {
    path: PathBuf,
}

impl SelectedPathGrant {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(any(test, feature = "host-issuer"))]
const HOST_PERMISSIONS: &[&str] = &[
    "content.read",
    "content.favorite",
    "content.delete",
    "clipboard.write",
    "clipboard.read",
    "history.query",
    "capture.screen",
    "filesystem.write.selected",
    "credential.account",
    "storage.domain",
];

#[derive(Clone, Debug)]
pub struct PermissionBroker {
    permissions: &'static [&'static str],
    host: bool,
}

impl PermissionBroker {
    #[cfg(any(test, feature = "host-issuer"))]
    pub fn host() -> Self {
        Self { permissions: HOST_PERMISSIONS, host: true }
    }

    /// 仅绑定 Mount 时声明的权限，调用方不能自行拼权限表。
    pub fn for_declared(permissions: DeclaredPermissions) -> Self {
        Self { permissions: permissions.as_slice(), host: false }
    }

    #[cfg(test)]
    pub fn for_plugin(permissions: &'static [&'static str]) -> Self {
        Self { permissions, host: false }
    }

    pub fn is_host(&self) -> bool {
        self.host
    }

    fn allows(&self, permission: &str) -> bool {
        self.host || self.permissions.contains(&permission)
    }

    fn max_bytes_for(kind: ContentKind) -> u64 {
        match kind {
            ContentKind::Files => u64::MAX,
            ContentKind::Gif | ContentKind::Video => 2 * 1024 * 1024 * 1024,
            _ => 64 * 1024 * 1024,
        }
    }

    pub fn grant_read(&self, id: ContentId) -> Option<ContentReadGrant> {
        self.allows("content.read")
            .then(|| ContentReadGrant::issue(id, 64 * 1024 * 1024, Duration::from_secs(60)))
    }

    pub fn grant_read_handle(&self, handle: ContentHandle) -> Option<ContentReadGrant> {
        self.grant_read(handle.id())
    }

    pub fn grant_history(&self) -> Option<HistoryQueryGrant> {
        self.allows("history.query").then_some(HistoryQueryGrant { _private: () })
    }

    pub fn grant_copy(&self, id: ContentId, kind: ContentKind) -> Option<ContentReadGrant> {
        if !self.allows("clipboard.write") && !self.allows("content.read") {
            return None;
        }
        Some(ContentReadGrant::issue(id, Self::max_bytes_for(kind), Duration::from_secs(60)))
    }

    pub fn grant_save(&self, id: ContentId, kind: ContentKind) -> Option<ContentReadGrant> {
        if !self.allows("content.read") || !self.allows("filesystem.write.selected") {
            return None;
        }
        Some(ContentReadGrant::issue(id, Self::max_bytes_for(kind), Duration::from_secs(60)))
    }

    pub fn grant_host_transfer(&self, id: ContentId) -> Option<ContentReadGrant> {
        self.host.then(|| ContentReadGrant::issue(id, u64::MAX, Duration::from_secs(60)))
    }

    pub fn grant_write_selected(&self, path: PathBuf) -> Option<SelectedPathGrant> {
        self.allows("filesystem.write.selected").then_some(SelectedPathGrant { path })
    }

    pub fn grant_command(
        &self,
        id: ContentId,
        favorite: bool,
        delete: bool,
    ) -> Option<ContentCommandGrant> {
        if favorite && !self.allows("content.favorite") {
            return None;
        }
        if delete && !self.allows("content.delete") {
            return None;
        }
        if !favorite && !delete {
            return None;
        }
        Some(ContentCommandGrant { content_id: id, favorite, delete })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_grant_is_invalid() {
        let id = ContentId::new();
        let grant = ContentReadGrant { content_id: id, max_bytes: 1, expires_at: Instant::now() };
        std::thread::sleep(Duration::from_millis(2));
        assert!(!grant.is_valid(id));
    }

    #[test]
    fn plugin_broker_cannot_issue_host_or_undeclared_grants() {
        let id = ContentId::new();
        let plugin = PermissionBroker::for_plugin(&["content.read"]);
        assert!(plugin.grant_read(id).is_some());
        assert!(plugin.grant_command(id, false, true).is_none());
        assert!(plugin.grant_host_transfer(id).is_none());
        assert!(plugin.grant_history().is_none());
        assert!(plugin.grant_write_selected(PathBuf::from("/tmp/out")).is_none());
        assert!(plugin.grant_save(id, ContentKind::Gif).is_none());
    }

    #[test]
    fn save_grant_matches_copy_limits_and_needs_both_permissions() {
        let id = ContentId::new();
        let save = PermissionBroker::for_plugin(&["content.read", "filesystem.write.selected"]);
        let gif = save.grant_save(id, ContentKind::Gif).unwrap();
        let copy = PermissionBroker::for_plugin(&["clipboard.write"])
            .grant_copy(id, ContentKind::Gif)
            .unwrap();
        assert_eq!(gif.max_bytes(), copy.max_bytes());
        assert_eq!(gif.max_bytes(), 2 * 1024 * 1024 * 1024);
        assert!(
            PermissionBroker::for_plugin(&["content.read"])
                .grant_save(id, ContentKind::Gif)
                .is_none()
        );
    }

    #[test]
    fn host_broker_can_transfer() {
        let id = ContentId::new();
        let host = PermissionBroker::host();
        assert!(host.is_host());
        assert!(host.grant_host_transfer(id).is_some());
        assert!(host.grant_command(id, true, true).is_some());
        assert!(host.grant_history().is_some());
        assert!(host.grant_write_selected(PathBuf::from("/tmp/out")).is_some());
    }
}
