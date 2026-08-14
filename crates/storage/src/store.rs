use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::Duration;

use asterism_core::content::{ContentItem, ContentStatus, FileManifest};
use asterism_core::id::{BlobId, ContentId};
use parking_lot::Mutex;
use rusqlite::Connection;

use crate::blob::BlobStore;
use crate::error::{Result, StorageError};
use crate::paths::db_path;
use crate::repo::{self, HistoryQuery};
use crate::schema;

const READER_COUNT: usize = 3;

enum WriteOp {
    Insert {
        item: Box<ContentItem>,
        manifest: Option<Box<FileManifest>>,
        reply: SyncSender<Result<ContentId>>,
    },
    Favorite {
        id: ContentId,
        favorite: bool,
        reply: SyncSender<Result<()>>,
    },
    Status {
        id: ContentId,
        status: ContentStatus,
        reply: SyncSender<Result<()>>,
    },
    Delete {
        id: ContentId,
        reply: SyncSender<Result<Option<BlobId>>>,
    },
    GcBlobs {
        released_before_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    PutBlob {
        bytes: Vec<u8>,
        reply: SyncSender<Result<BlobId>>,
    },
    SetCursor {
        scope: String,
        cursor: String,
        reply: SyncSender<Result<()>>,
    },
    EnsureCommitted {
        item: Box<ContentItem>,
        reply: SyncSender<Result<()>>,
    },
    AckOutbox {
        id: i64,
        consumer_id: String,
        reply: SyncSender<Result<bool>>,
    },
    RetryOutbox {
        id: i64,
        consumer_id: String,
        delay_ms: i64,
        reply: SyncSender<Result<()>>,
    },
    Shutdown,
}

pub struct Store {
    writer: SyncSender<WriteOp>,
    readers: ReaderPool,
    blobs: BlobStore,
    writer_thread: Mutex<Option<thread::JoinHandle<()>>>,
}

struct ReaderPool {
    conns: Vec<Mutex<Connection>>,
}

impl ReaderPool {
    fn open(db: &Path, n: usize) -> Result<Self> {
        let mut conns = Vec::with_capacity(n);
        for _ in 0..n {
            let conn = open_read(db)?;
            conns.push(Mutex::new(conn));
        }
        Ok(Self { conns })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        for slot in &self.conns {
            if let Some(conn) = slot.try_lock() {
                return f(&conn);
            }
        }
        let conn = self.conns[0].lock();
        f(&conn)
    }
}

impl Store {
    pub fn open(data_root: impl Into<PathBuf>) -> Result<Arc<Self>> {
        let data_root = data_root.into();
        std::fs::create_dir_all(&data_root)?;
        let db = db_path(&data_root);

        let bootstrap = open_write(&db)?;
        schema::initialize(&bootstrap)?;
        drop(bootstrap);

        let blobs = BlobStore::open(&data_root)?;
        let writer_blobs = blobs.clone();
        let (tx, rx) = mpsc::sync_channel(128);
        let writer_db = db.clone();
        let handle = thread::Builder::new().name("asterism-db-writer".into()).spawn(move || {
            writer_loop(writer_db, writer_blobs, rx);
        })?;

        let store = Arc::new(Self {
            writer: tx,
            readers: ReaderPool::open(&db, READER_COUNT)?,
            blobs,
            writer_thread: Mutex::new(Some(handle)),
        });
        Ok(store)
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// 仅 Ingestion / 本 crate 测试写入。业务路径必须走 `ContentCommitPort`。
    pub(crate) fn insert(
        &self,
        item: ContentItem,
        manifest: Option<FileManifest>,
    ) -> Result<ContentId> {
        if !item.may_enter_history() {
            return Err(StorageError::from(asterism_core::CoreError::PolicyRejected(
                "sensitive or transient items must not enter history",
            )));
        }
        self.call(|reply| WriteOp::Insert {
            item: Box::new(item),
            manifest: manifest.map(Box::new),
            reply,
        })
    }

    pub fn get(&self, id: ContentId) -> Result<ContentItem> {
        self.readers.with(|conn| repo::get_item(conn, id))
    }

    pub fn contains(&self, id: ContentId) -> Result<bool> {
        self.readers.with(|conn| repo::contains_item(conn, id))
    }

    pub fn history(&self, query: HistoryQuery) -> Result<Vec<ContentItem>> {
        self.readers.with(|conn| repo::list_history(conn, &query))
    }

    pub fn set_favorite(&self, id: ContentId, favorite: bool) -> Result<()> {
        self.call(|reply| WriteOp::Favorite { id, favorite, reply })
    }

    pub fn set_status(&self, id: ContentId, status: ContentStatus) -> Result<()> {
        self.call(|reply| WriteOp::Status { id, status, reply })
    }

    pub fn pending_sync(&self, limit: u32) -> Result<Vec<ContentItem>> {
        self.readers.with(|conn| repo::list_pending_sync(conn, limit))
    }

    pub fn delete(&self, id: ContentId) -> Result<()> {
        let blob = self.call(|reply| WriteOp::Delete { id, reply })?;
        // Blob 文件删除由 GC 负责；这里只降 ref_count。
        let _ = blob;
        Ok(())
    }

    pub fn load_manifest(&self, id: asterism_core::id::ManifestId) -> Result<FileManifest> {
        self.readers.with(|conn| repo::load_manifest(conn, id))
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        self.call(|reply| WriteOp::PutBlob { bytes: bytes.to_vec(), reply })
    }

    pub fn get_blob(&self, id: &BlobId) -> Result<Vec<u8>> {
        self.blobs.get(id)
    }

    pub fn gc_blobs(&self, grace: Duration) -> Result<u64> {
        let released_before_ms = crate::repo::now_ms_for_gc()
            .saturating_sub(i64::try_from(grace.as_millis()).unwrap_or(i64::MAX));
        self.call(|reply| WriteOp::GcBlobs { released_before_ms, reply })
    }

    pub fn sweep_orphan_blobs(&self) -> Result<u64> {
        let known = self.readers.with(repo::all_blob_ids)?;
        self.blobs.remove_orphans(&known, Duration::from_secs(60 * 60))
    }

    pub fn hub_cursor(&self) -> Result<Option<String>> {
        self.readers.with(|conn| repo::get_cursor(conn, "hub"))
    }

    pub fn set_hub_cursor(&self, cursor: &str) -> Result<()> {
        self.kv_set("hub", cursor)
    }

    pub fn kv_get(&self, scope: &str) -> Result<Option<String>> {
        self.readers.with(|conn| repo::get_cursor(conn, scope))
    }

    pub fn kv_set(&self, scope: &str, value: &str) -> Result<()> {
        self.call(|reply| WriteOp::SetCursor {
            scope: scope.to_string(),
            cursor: value.to_string(),
            reply,
        })
    }

    pub fn cache_pins(&self) -> Result<Vec<String>> {
        self.readers.with(repo::list_cache_pins)
    }

    pub fn commit_port(self: &Arc<Self>) -> ContentCommitPort {
        ContentCommitPort { store: Arc::clone(self) }
    }

    pub fn ensure_committed(&self, item: &ContentItem) -> Result<()> {
        self.call(|reply| WriteOp::EnsureCommitted { item: Box::new(item.clone()), reply })
    }

    pub fn pending_outbox(&self, limit: u32) -> Result<Vec<crate::OutboxEvent>> {
        let now = crate::repo::now_ms_for_gc();
        self.readers.with(|conn| crate::outbox::pending(conn, limit, now))
    }

    pub fn pending_outbox_for(
        &self,
        event_type: &str,
        consumer_id: &str,
        limit: u32,
    ) -> Result<Vec<crate::OutboxEvent>> {
        let now = crate::repo::now_ms_for_gc();
        let event_type = event_type.to_string();
        let consumer_id = consumer_id.to_string();
        self.readers.with(|conn| {
            crate::outbox::pending_for(conn, Some(&event_type), Some(&consumer_id), limit, now)
        })
    }

    pub fn ack_outbox(&self, id: i64) -> Result<bool> {
        self.ack_outbox_consumer(id, crate::CONSUMER_HUB)
    }

    pub fn ack_outbox_consumer(&self, id: i64, consumer_id: &str) -> Result<bool> {
        let consumer_id = consumer_id.to_string();
        self.call(|reply| WriteOp::AckOutbox { id, consumer_id, reply })
    }

    pub fn retry_outbox(&self, id: i64, delay: Duration) -> Result<()> {
        self.retry_outbox_consumer(id, crate::CONSUMER_HUB, delay)
    }

    pub fn retry_outbox_consumer(&self, id: i64, consumer_id: &str, delay: Duration) -> Result<()> {
        let delay_ms = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
        let consumer_id = consumer_id.to_string();
        self.call(|reply| WriteOp::RetryOutbox { id, consumer_id, delay_ms, reply })
    }

    fn call<T>(&self, build: impl FnOnce(SyncSender<Result<T>>) -> WriteOp) -> Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.writer.send(build(tx)).map_err(|_| StorageError::WriterStopped)?;
        rx.recv().map_err(|_| StorageError::WriterStopped)?
    }
}

/// 领域 Ingestion 的唯一对外提交口。普通插件不得持有 `Store` 写路径。
pub struct ContentCommitPort {
    store: Arc<Store>,
}

impl ContentCommitPort {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn commit(&self, item: ContentItem, manifest: Option<FileManifest>) -> Result<ContentId> {
        self.store.insert(item, manifest)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.writer.send(WriteOp::Shutdown);
        if let Some(handle) = self.writer_thread.lock().take() {
            let _ = handle.join();
        }
    }
}

fn writer_loop(db: PathBuf, blobs: BlobStore, rx: Receiver<WriteOp>) {
    let conn = match open_write(&db) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::error!(error = %err, "db writer failed to open");
            return;
        }
    };
    loop {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(WriteOp::Insert { item, manifest, reply }) => {
                let id = item.id();
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        repo::insert_item(&tx, &item, manifest.as_deref())?;
                        crate::outbox::enqueue_committed(&tx, &item, crate::repo::now_ms_for_gc())?;
                        tx.commit()?;
                        Ok(id)
                    });
                let _ = reply.send(result);
            }
            Ok(WriteOp::Favorite { id, favorite, reply }) => {
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        repo::set_favorite(&tx, id, favorite)?;
                        tx.commit()?;
                        Ok(())
                    });
                let _ = reply.send(result);
            }
            Ok(WriteOp::Status { id, status, reply }) => {
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        repo::set_status(&tx, id, status)?;
                        tx.commit()?;
                        Ok(())
                    });
                let _ = reply.send(result);
            }
            Ok(WriteOp::Delete { id, reply }) => {
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        let blob = repo::delete_item(&tx, id)?;
                        crate::outbox::enqueue_deleted(&tx, id, crate::repo::now_ms_for_gc())?;
                        tx.commit()?;
                        Ok(blob)
                    });
                let _ = reply.send(result);
            }
            Ok(WriteOp::PutBlob { bytes, reply }) => {
                let result = (|| {
                    let id = blobs.put(&bytes)?;
                    repo::reserve_blob(&conn, &id, crate::repo::now_ms_for_gc())?;
                    Ok(id)
                })();
                let _ = reply.send(result);
            }
            Ok(WriteOp::SetCursor { scope, cursor, reply }) => {
                let result = repo::set_cursor(&conn, &scope, &cursor, crate::repo::now_ms_for_gc());
                let _ = reply.send(result);
            }
            Ok(WriteOp::EnsureCommitted { item, reply }) => {
                let result = crate::outbox::latest_unacked(
                    &conn,
                    crate::EVENT_COMMITTED,
                    item.id(),
                    crate::CONSUMER_HUB,
                )
                .and_then(|existing| {
                    if existing.is_some() {
                        return Ok(());
                    }
                    crate::outbox::enqueue_committed(&conn, &item, crate::repo::now_ms_for_gc())
                });
                let _ = reply.send(result);
            }
            Ok(WriteOp::AckOutbox { id, consumer_id, reply }) => {
                let result = crate::outbox::ack_consumer(
                    &conn,
                    id,
                    &consumer_id,
                    crate::repo::now_ms_for_gc(),
                );
                let _ = reply.send(result);
            }
            Ok(WriteOp::RetryOutbox { id, consumer_id, delay_ms, reply }) => {
                let available_at = crate::repo::now_ms_for_gc().saturating_add(delay_ms.max(0));
                let result = crate::outbox::retry_consumer(&conn, id, &consumer_id, available_at);
                let _ = reply.send(result);
            }
            Ok(WriteOp::GcBlobs { released_before_ms, reply }) => {
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        let candidates = repo::unused_blobs(&tx, released_before_ms)?;
                        for id in &candidates {
                            repo::delete_unused_blob_ref(&tx, id, released_before_ms)?;
                        }
                        tx.commit()?;
                        let mut removed = 0u64;
                        for id in candidates {
                            blobs.remove_if_unused(&id)?;
                            removed += 1;
                        }
                        Ok(removed)
                    });
                let _ = reply.send(result);
            }
            Ok(WriteOp::Shutdown) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn open_write(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    schema::initialize(&conn)?;
    Ok(conn)
}

fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA query_only=ON;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        "#,
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_core::content::{
        ContentFlags, ContentKind, ContentStatus, ItemMetadata, PayloadRef,
    };
    use asterism_core::id::DeviceId;
    use bytes::Bytes;

    fn sample(text: &str) -> ContentItem {
        let bytes = Bytes::from(text.as_bytes().to_vec());
        ContentItem::from_trusted(
            ContentId::new(),
            DeviceId::new(),
            ContentKind::Text,
            1_700_000_000_000,
            bytes.len() as u64,
            bytes.len() as u64,
            asterism_crypto::local_dedup_tag(text.as_bytes()),
            ContentFlags::REMOTE_ALLOWED,
            ContentStatus::Local,
            ItemMetadata { text_preview: Some(text.to_string()), ..ItemMetadata::default() },
            PayloadRef::Inline { bytes },
            Bytes::new(),
        )
    }

    #[test]
    fn insert_search_favorite_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("你好世界 hello asterism");
        let id = store.insert(item.clone(), None).unwrap();

        let loaded = store.get(id).unwrap();
        assert_eq!(loaded.kind(), ContentKind::Text);

        let listed = store
            .history(HistoryQuery {
                query: Some("世界".into()),
                limit: 10,
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
        let listed_fts = store
            .history(HistoryQuery {
                query: Some("asterism".into()),
                limit: 10,
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(listed_fts.len(), 1);

        store.set_favorite(id, true).unwrap();
        let fav = store
            .history(HistoryQuery { favorite_only: true, limit: 10, ..HistoryQuery::default() })
            .unwrap();
        assert_eq!(fav.len(), 1);
        assert!(fav[0].flags().contains(ContentFlags::FAVORITE));

        store.delete(id).unwrap();
        assert!(matches!(store.get(id), Err(StorageError::NotFound)));
    }

    #[test]
    fn history_cursor_does_not_skip_equal_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for text in ["one", "two", "three"] {
            store.insert(sample(text), None).unwrap();
        }
        let first = store.history(HistoryQuery::recent(2)).unwrap();
        assert_eq!(first.len(), 2);
        let cursor = first.last().unwrap();
        let second = store
            .history(HistoryQuery {
                limit: 2,
                before_ms: Some(cursor.created_at_ms()),
                before_id: Some(cursor.id()),
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(second.len(), 1);
        assert!(!first.iter().any(|item| item.id() == second[0].id()));
    }

    #[test]
    fn rejects_sensitive_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut item = sample("secret");
        item.set_flags(ContentFlags::SENSITIVE);
        assert!(store.insert(item, None).is_err());
    }

    #[test]
    fn pending_sync_survives_status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("retry me");
        let id = store.insert(item, None).unwrap();

        assert_eq!(store.pending_sync(10).unwrap()[0].id(), id);
        store.set_status(id, ContentStatus::Uploading).unwrap();
        assert_eq!(store.pending_sync(10).unwrap()[0].status(), ContentStatus::Uploading);
        store.set_status(id, ContentStatus::Failed).unwrap();
        assert_eq!(store.pending_sync(10).unwrap()[0].status(), ContentStatus::Failed);
        store.set_status(id, ContentStatus::SyncedToHub).unwrap();
        assert!(store.pending_sync(10).unwrap().is_empty());
    }

    #[test]
    fn blob_gc_removes_released_file_through_writer_queue() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let blob = store.put_blob(b"image bytes").unwrap();
        let mut item = sample("image");
        item.set_kind(ContentKind::Image);
        item.set_payload(
            PayloadRef::Blob { blob_id: blob.clone() },
            item.logical_size(),
            item.payload_size(),
        );
        let id = store.insert(item, None).unwrap();

        store.delete(id).unwrap();
        assert!(store.blobs().exists(&blob));
        assert_eq!(store.gc_blobs(Duration::ZERO).unwrap(), 1);
        assert!(!store.blobs().exists(&blob));
    }

    #[test]
    fn put_blob_without_insert_observes_gc_grace() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let blob = store.put_blob(b"fresh reserved blob").unwrap();
        assert_eq!(store.gc_blobs(Duration::from_secs(24 * 60 * 60)).unwrap(), 0);
        assert_eq!(store.sweep_orphan_blobs().unwrap(), 0);
        assert!(store.blobs().exists(&blob));
    }

    #[test]
    fn concurrent_reads_during_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for i in 0..20 {
            store.insert(sample(&format!("item-{i}")), None).unwrap();
        }
        std::thread::scope(|s| {
            for _ in 0..4 {
                let store = Arc::clone(&store);
                s.spawn(move || {
                    for _ in 0..20 {
                        let _ = store.history(HistoryQuery::recent(10)).unwrap();
                    }
                });
            }
            let store = Arc::clone(&store);
            s.spawn(move || {
                for i in 20..40 {
                    store.insert(sample(&format!("more-{i}")), None).unwrap();
                }
            });
        });
        let all = store.history(HistoryQuery::recent(100)).unwrap();
        assert_eq!(all.len(), 40);
    }

    #[test]
    fn repeated_same_text_inserts_are_distinct_history_items() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let first = sample("same body");
        let second = sample("same body");
        assert_ne!(first.id(), second.id());
        store.insert(first.clone(), None).unwrap();
        store.insert(second.clone(), None).unwrap();
        let listed = store.history(HistoryQuery::recent(10)).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|item| item.id() == first.id()));
        assert!(listed.iter().any(|item| item.id() == second.id()));
    }

    #[test]
    fn insert_and_delete_enqueue_outbox_in_same_writer_tx() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("outbox fact");
        store.insert(item.clone(), None).unwrap();

        let pending = store.pending_outbox(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, crate::EVENT_COMMITTED);
        assert_eq!(pending[0].aggregate_id, item.id());
        assert!(!pending[0].payload().unwrap().from_remote);

        store.delete(item.id()).unwrap();
        assert!(matches!(store.get(item.id()), Err(StorageError::NotFound)));
        let pending = store.pending_outbox(10).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[1].event_type, crate::EVENT_DELETED);
        assert!(store.ack_outbox(pending[0].id).unwrap());
        assert!(!store.ack_outbox(pending[0].id).unwrap());
    }

    #[test]
    fn remote_insert_marks_outbox_from_remote() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut item = sample("from peer");
        item.or_flags(ContentFlags::FROM_REMOTE);
        store.insert(item, None).unwrap();
        let pending = store.pending_outbox(1).unwrap();
        assert!(pending[0].payload().unwrap().from_remote);
    }

    #[test]
    fn deleted_events_do_not_block_committed_consumer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for i in 0..60 {
            let item = sample(&format!("del-{i}"));
            store.insert(item.clone(), None).unwrap();
            store.delete(item.id()).unwrap();
        }
        let live = sample("live");
        store.insert(live.clone(), None).unwrap();
        let lan =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_LAN, 200).unwrap();
        assert!(lan.iter().all(|event| event.event_type == crate::EVENT_COMMITTED));
        assert!(lan.iter().any(|event| event.aggregate_id == live.id()));
        let deleted =
            store.pending_outbox_for(crate::EVENT_DELETED, crate::CONSUMER_HUB_DELETE, 50).unwrap();
        assert_eq!(deleted.len(), 50);
        assert!(deleted.iter().all(|event| event.event_type == crate::EVENT_DELETED));
    }

    #[test]
    fn ensure_committed_reenqueues_after_hub_ack() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("local waiting for hub");
        store.insert(item.clone(), None).unwrap();
        let first =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_HUB, 10).unwrap();
        assert_eq!(first.len(), 1);
        store.ack_outbox_consumer(first[0].id, crate::CONSUMER_HUB).unwrap();
        assert!(
            store
                .pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_HUB, 10)
                .unwrap()
                .is_empty()
        );
        store.ensure_committed(&item).unwrap();
        let again =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_HUB, 10).unwrap();
        assert_eq!(again.len(), 1);
        assert_ne!(again[0].id, first[0].id);
    }

    #[test]
    fn lan_ack_does_not_complete_hub_consumer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("dual consumer");
        store.insert(item, None).unwrap();
        let lan =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_LAN, 10).unwrap();
        assert_eq!(lan.len(), 1);
        assert!(store.ack_outbox_consumer(lan[0].id, crate::CONSUMER_LAN).unwrap());
        assert!(
            store
                .pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_LAN, 10)
                .unwrap()
                .is_empty()
        );
        let hub =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_HUB, 10).unwrap();
        assert_eq!(hub.len(), 1);
        assert_eq!(hub[0].id, lan[0].id);
    }

    #[test]
    fn lan_ack_survives_reopen_without_acking_hub() {
        let dir = tempfile::tempdir().unwrap();
        let item = sample("crash after lan ack");
        let event_id = {
            let store = Store::open(dir.path()).unwrap();
            store.insert(item, None).unwrap();
            let lan =
                store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_LAN, 10).unwrap();
            store.ack_outbox_consumer(lan[0].id, crate::CONSUMER_LAN).unwrap();
            lan[0].id
        };
        let store = Store::open(dir.path()).unwrap();
        assert!(
            store
                .pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_LAN, 10)
                .unwrap()
                .is_empty()
        );
        let hub =
            store.pending_outbox_for(crate::EVENT_COMMITTED, crate::CONSUMER_HUB, 10).unwrap();
        assert_eq!(hub.len(), 1);
        assert_eq!(hub[0].id, event_id);
    }

    #[test]
    fn unacked_outbox_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let item = sample("crash after commit");
        let id = item.id();
        {
            let store = Store::open(dir.path()).unwrap();
            store.insert(item, None).unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        let pending = store.pending_outbox(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].aggregate_id, id);
        assert_eq!(pending[0].event_type, crate::EVENT_COMMITTED);
    }

    #[test]
    fn schema_v1_fixture_gains_outbox_table() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("asterism.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
                INSERT INTO meta(key, value) VALUES ('schema_version', '1');
                "#,
            )
            .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        let item = sample("migrated");
        store.insert(item, None).unwrap();
        assert_eq!(store.pending_outbox(1).unwrap().len(), 1);
    }
}
