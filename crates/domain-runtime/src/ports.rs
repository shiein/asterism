use std::sync::Arc;
use std::time::Duration;

use asterism_core::content::{ContentItem, ContentStatus, FileManifest};
use asterism_core::id::{BlobId, ContentId, ManifestId};
use asterism_storage::{OutboxEvent, StorageError, Store};

/// 密封存储句柄。插件与 Sync 只看见这组方法，拿不到 SQLite `Store`。
pub struct DomainStore {
    inner: Arc<Store>,
}

impl DomainStore {
    pub fn wrap(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self { inner: store })
    }

    pub fn get(&self, id: ContentId) -> Result<ContentItem, StorageError> {
        self.inner.get(id)
    }

    pub fn contains(&self, id: ContentId) -> Result<bool, StorageError> {
        self.inner.contains(id)
    }

    pub fn get_blob(&self, id: &BlobId) -> Result<Vec<u8>, StorageError> {
        self.inner.get_blob(id)
    }

    pub fn load_manifest(&self, id: ManifestId) -> Result<FileManifest, StorageError> {
        self.inner.load_manifest(id)
    }

    pub fn pending_outbox_for(
        &self,
        event_type: &str,
        consumer_id: &str,
        limit: u32,
    ) -> Result<Vec<OutboxEvent>, StorageError> {
        self.inner.pending_outbox_for(event_type, consumer_id, limit)
    }

    pub fn ack_outbox_consumer(&self, id: i64, consumer_id: &str) -> Result<bool, StorageError> {
        self.inner.ack_outbox_consumer(id, consumer_id)
    }

    pub fn retry_outbox_consumer(
        &self,
        id: i64,
        consumer_id: &str,
        delay: Duration,
    ) -> Result<(), StorageError> {
        self.inner.retry_outbox_consumer(id, consumer_id, delay)
    }

    pub fn ensure_committed(&self, item: &ContentItem) -> Result<(), StorageError> {
        self.inner.ensure_committed(item)
    }

    pub fn pending_sync(&self, limit: u32) -> Result<Vec<ContentItem>, StorageError> {
        self.inner.pending_sync(limit)
    }

    pub fn set_status(&self, id: ContentId, status: ContentStatus) -> Result<(), StorageError> {
        self.inner.set_status(id, status)
    }

    pub fn hub_cursor(&self) -> Result<Option<String>, StorageError> {
        self.inner.hub_cursor()
    }

    pub fn set_hub_cursor(&self, cursor: &str) -> Result<(), StorageError> {
        self.inner.set_hub_cursor(cursor)
    }

    pub fn kv_get(&self, scope: &str) -> Result<Option<String>, StorageError> {
        self.inner.kv_get(scope)
    }

    pub fn kv_set(&self, scope: &str, value: &str) -> Result<(), StorageError> {
        self.inner.kv_set(scope, value)
    }

    pub fn cache_pins(&self) -> Result<Vec<String>, StorageError> {
        self.inner.cache_pins()
    }

    pub fn gc_blobs(&self, grace: Duration) -> Result<u64, StorageError> {
        self.inner.gc_blobs(grace)
    }

    pub fn gc_outbox(&self, grace: Duration) -> Result<u64, StorageError> {
        self.inner.gc_outbox(grace)
    }

    pub fn sweep_orphan_blobs(&self) -> Result<u64, StorageError> {
        self.inner.sweep_orphan_blobs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_core::content::{
        ContentFlags, ContentKind, ContentStatus, ItemMetadata, PayloadRef,
    };
    use asterism_core::id::DeviceId;
    use bytes::Bytes;

    #[test]
    fn domain_store_does_not_expose_writer_insert() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let domain = DomainStore::wrap(store);
        let item = ContentItem::from_trusted(
            ContentId::new(),
            DeviceId::new(),
            ContentKind::Text,
            1,
            1,
            1,
            [1; 32],
            ContentFlags::empty(),
            ContentStatus::Local,
            ItemMetadata::default(),
            PayloadRef::Inline { bytes: Bytes::from_static(b"x") },
            Bytes::new(),
        );
        assert!(matches!(domain.get(item.id()), Err(StorageError::NotFound)));
    }
}
