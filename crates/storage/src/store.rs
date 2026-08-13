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

    pub fn insert(&self, item: ContentItem, manifest: Option<FileManifest>) -> Result<ContentId> {
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
        self.blobs.put(bytes)
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
        self.blobs.remove_orphans(&known)
    }

    fn call<T>(&self, build: impl FnOnce(SyncSender<Result<T>>) -> WriteOp) -> Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.writer.send(build(tx)).map_err(|_| StorageError::WriterStopped)?;
        rx.recv().map_err(|_| StorageError::WriterStopped)?
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
                let id = item.id;
                let result =
                    conn.unchecked_transaction().map_err(StorageError::from).and_then(|tx| {
                        repo::insert_item(&tx, &item, manifest.as_deref())?;
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
                        tx.commit()?;
                        Ok(blob)
                    });
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
        ContentItem {
            id: ContentId::new(),
            origin_device_id: DeviceId::new(),
            kind: ContentKind::Text,
            created_at_ms: 1_700_000_000_000,
            logical_size: bytes.len() as u64,
            payload_size: bytes.len() as u64,
            dedup_tag: asterism_crypto::local_dedup_tag(text.as_bytes()),
            flags: ContentFlags::REMOTE_ALLOWED,
            status: ContentStatus::Local,
            metadata: ItemMetadata {
                text_preview: Some(text.to_string()),
                ..ItemMetadata::default()
            },
            payload_ref: PayloadRef::Inline { bytes },
            encrypted_metadata: Bytes::new(),
        }
    }

    #[test]
    fn insert_search_favorite_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("你好世界 hello asterism");
        let id = store.insert(item.clone(), None).unwrap();

        let loaded = store.get(id).unwrap();
        assert_eq!(loaded.kind, ContentKind::Text);

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
        assert!(fav[0].flags.contains(ContentFlags::FAVORITE));

        store.delete(id).unwrap();
        assert!(matches!(store.get(id), Err(StorageError::NotFound)));
    }

    #[test]
    fn rejects_sensitive_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut item = sample("secret");
        item.flags = ContentFlags::SENSITIVE;
        assert!(store.insert(item, None).is_err());
    }

    #[test]
    fn pending_sync_survives_status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let item = sample("retry me");
        let id = store.insert(item, None).unwrap();

        assert_eq!(store.pending_sync(10).unwrap()[0].id, id);
        store.set_status(id, ContentStatus::Uploading).unwrap();
        assert_eq!(store.pending_sync(10).unwrap()[0].status, ContentStatus::Uploading);
        store.set_status(id, ContentStatus::Failed).unwrap();
        assert_eq!(store.pending_sync(10).unwrap()[0].status, ContentStatus::Failed);
        store.set_status(id, ContentStatus::SyncedToHub).unwrap();
        assert!(store.pending_sync(10).unwrap().is_empty());
    }

    #[test]
    fn blob_gc_removes_released_file_through_writer_queue() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let blob = store.put_blob(b"image bytes").unwrap();
        let mut item = sample("image");
        item.kind = ContentKind::Image;
        item.payload_ref = PayloadRef::Blob { blob_id: blob.clone() };
        let id = store.insert(item, None).unwrap();

        store.delete(id).unwrap();
        assert!(store.blobs().exists(&blob));
        assert_eq!(store.gc_blobs(Duration::ZERO).unwrap(), 1);
        assert!(!store.blobs().exists(&blob));
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
}
