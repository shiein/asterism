use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use asterism_clipboard::capture::CapturedClipboard;
use asterism_clipboard::files::materialize_to_cache;
use asterism_clipboard::guard::SelfWriteGuard;
use asterism_clipboard::normalize::NormalizedContent;
use asterism_core::content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, FileManifest, ItemMetadata, PayloadRef,
};
use asterism_core::id::DeviceId;
use asterism_core::{
    ContentDraft, DedupDecision, FILE_WORKER_DEBOUNCE_MS, IngestionOutcome,
    should_skip_duplicate_payload_tag,
};
use asterism_crypto::AccountVaultKey;
use asterism_platform::AppPaths;
use asterism_storage::ContentCommitPort;
use asterism_storage::Store;
use parking_lot::{Mutex, RwLock};

pub struct Ingestion {
    store: Arc<Store>,
    commit: ContentCommitPort,
    paths: AppPaths,
    device_id: DeviceId,
    avk: Arc<RwLock<AccountVaultKey>>,
    seen_events: Mutex<HashMap<String, Instant>>,
    last_file_tag: Mutex<Option<([u8; 32], Instant)>>,
}

impl Ingestion {
    pub fn new(
        store: Arc<Store>,
        paths: AppPaths,
        device_id: DeviceId,
        avk: Arc<RwLock<AccountVaultKey>>,
    ) -> Arc<Self> {
        let commit = ContentCommitPort::new(Arc::clone(&store));
        Arc::new(Self {
            store,
            commit,
            paths,
            device_id,
            avk,
            seen_events: Mutex::new(HashMap::new()),
            last_file_tag: Mutex::new(None),
        })
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }

    /// Clipboard Plugin 只交原始捕获；Normalize / Sensitive / Policy 在此执行。
    pub fn submit_capture(
        &self,
        mut draft: ContentDraft,
        captured: CapturedClipboard,
        guard: &SelfWriteGuard,
    ) -> anyhow::Result<IngestionOutcome> {
        let policy = asterism_core::CapturePolicy::default();
        let remote = asterism_core::RemotePolicy::default();
        let Some(content) = asterism_clipboard::normalize::normalize(&captured, &policy, &remote)?
        else {
            return Ok(IngestionOutcome::RejectedPolicy);
        };
        if guard.is_self_write(None, &content.dedup_tag()) {
            return Ok(IngestionOutcome::Ignored(DedupDecision::SelfWrite));
        }
        if draft.source_event_id.is_empty() {
            draft.source_event_id = captured.change_token.to_string();
        }
        draft.change_token = Some(captured.change_token);
        if draft.source_app.is_none() {
            draft.source_app = captured.source_app.clone();
        }
        draft.source_device_id = self.device_id;
        self.submit_local(draft, content, false)
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn decide(
        &self,
        draft: &ContentDraft,
        is_self_write: bool,
        file_tag: Option<[u8; 32]>,
    ) -> DedupDecision {
        if is_self_write {
            return DedupDecision::SelfWrite;
        }
        self.prune_seen();
        if !draft.source_event_id.is_empty() {
            let key = format!("{}:{}", draft.producer_plugin_id, draft.source_event_id);
            if self.seen_events.lock().contains_key(&key) {
                return DedupDecision::SameSourceEvent;
            }
        }
        if let Some(tag) = file_tag {
            let now = Instant::now();
            let window = std::time::Duration::from_millis(FILE_WORKER_DEBOUNCE_MS);
            let skip = should_skip_duplicate_payload_tag(
                self.last_file_tag.lock().as_ref().map(|(prev, at)| (prev, *at)),
                &tag,
                now,
                window,
            );
            if skip {
                return DedupDecision::SameSourceEvent;
            }
            *self.last_file_tag.lock() = Some((tag, now));
        }
        DedupDecision::NewCapture
    }

    pub fn submit_local(
        &self,
        draft: ContentDraft,
        content: NormalizedContent,
        is_self_write: bool,
    ) -> anyhow::Result<IngestionOutcome> {
        if let NormalizedContent::Files { paths, flags, source_app, .. } = content {
            return self.submit_files(draft, paths, flags, source_app, is_self_write);
        }
        let file_tag = None;
        match self.decide(&draft, is_self_write, file_tag) {
            DedupDecision::NewCapture => {}
            other => return Ok(IngestionOutcome::Ignored(other)),
        }
        let event_key = event_key(&draft);
        let Some(item) = self.materialize(draft, content)? else {
            return Ok(IngestionOutcome::RejectedPolicy);
        };
        if !item.may_enter_history() {
            return Ok(IngestionOutcome::RejectedPolicy);
        }
        let id = item.id();
        self.commit.commit(item, None)?;
        self.remember_event_key(event_key);
        Ok(IngestionOutcome::Committed(id))
    }

    pub fn submit_files(
        &self,
        draft: ContentDraft,
        sources: Vec<std::path::PathBuf>,
        flags: ContentFlags,
        source_app: Option<String>,
        is_self_write: bool,
    ) -> anyhow::Result<IngestionOutcome> {
        let file_tag = asterism_clipboard::files_local_dedup_tag(&sources);
        match self.decide(&draft, is_self_write, Some(file_tag)) {
            DedupDecision::NewCapture => {}
            other => return Ok(IngestionOutcome::Ignored(other)),
        }
        let event_key = event_key(&draft);
        let Some((mut item, manifest)) = self.materialize_files(sources, flags, source_app)? else {
            return Ok(IngestionOutcome::RejectedPolicy);
        };
        apply_provenance(&mut item, &draft);
        let id = item.id();
        self.commit.commit(item, Some(manifest))?;
        self.remember_event_key(event_key);
        Ok(IngestionOutcome::Committed(id))
    }

    #[allow(dead_code)]
    pub(crate) fn submit_prepared(
        &self,
        mut item: ContentItem,
        manifest: Option<FileManifest>,
    ) -> anyhow::Result<IngestionOutcome> {
        if !item.may_enter_history() {
            return Ok(IngestionOutcome::RejectedPolicy);
        }
        apply_hmac(&mut item, &self.avk.read());
        let id = item.id();
        self.commit.commit(item, manifest)?;
        Ok(IngestionOutcome::Committed(id))
    }

    /// 远端/LAN 项的唯一组装入口。调用方不得自行 `from_trusted`。
    pub fn assemble_remote(
        &self,
        spec: RemoteItemSpec,
        payload: RemoteItemBody,
    ) -> anyhow::Result<(ContentItem, Option<FileManifest>)> {
        let mut flags = spec.flags;
        flags.insert(ContentFlags::FROM_REMOTE);
        flags.remove(ContentFlags::REMOTE_ALLOWED);
        let created_at_ms = spec.created_at_ms.unwrap_or_else(asterism_platform::now_ms);
        let status = if spec.from_lan && spec.kind == ContentKind::Text {
            ContentStatus::DeliveredToPeer
        } else {
            ContentStatus::SyncedToHub
        };
        match payload {
            RemoteItemBody::Bytes(bytes) => {
                let logical_size =
                    spec.logical_size.filter(|n| *n > 0).unwrap_or(bytes.len() as u64);
                let payload_size =
                    spec.payload_size.filter(|n| *n > 0).unwrap_or(bytes.len() as u64);
                let payload_ref = match spec.kind {
                    ContentKind::Text => PayloadRef::Inline { bytes: bytes::Bytes::from(bytes) },
                    ContentKind::Image
                    | ContentKind::Screenshot
                    | ContentKind::Gif
                    | ContentKind::Video => {
                        let blob = self.store.put_blob(&bytes)?;
                        PayloadRef::Blob { blob_id: blob }
                    }
                    ContentKind::Files => anyhow::bail!("files require FileManifest payload"),
                    other => anyhow::bail!("unsupported kind {other}"),
                };
                let mut item = ContentItem::from_trusted(
                    spec.id,
                    spec.origin,
                    spec.kind,
                    created_at_ms,
                    logical_size,
                    payload_size,
                    spec.tag,
                    flags,
                    status,
                    spec.metadata,
                    payload_ref,
                    bytes::Bytes::new(),
                );
                apply_remote_policy(&mut item);
                item.or_flags(ContentFlags::FROM_REMOTE);
                Ok((item, None))
            }
            RemoteItemBody::Files(manifest) => {
                let mut metadata = spec.metadata;
                metadata.local_cache_rel = Some(spec.id.to_string());
                metadata.files = Some(manifest.summary());
                let manifest_id = manifest.id;
                let logical_size = spec
                    .logical_size
                    .filter(|n| *n > 0)
                    .unwrap_or_else(|| manifest.entries.iter().map(|e| e.size).sum());
                let payload_size = spec.payload_size.filter(|n| *n > 0).unwrap_or(logical_size);
                let mut item = ContentItem::from_trusted(
                    spec.id,
                    spec.origin,
                    ContentKind::Files,
                    created_at_ms,
                    logical_size,
                    payload_size,
                    spec.tag,
                    flags,
                    status,
                    metadata,
                    PayloadRef::FileManifest { manifest_id },
                    bytes::Bytes::new(),
                );
                apply_remote_policy(&mut item);
                item.or_flags(ContentFlags::FROM_REMOTE);
                Ok((item, Some(manifest)))
            }
        }
    }

    pub fn commit_remote(
        &self,
        item: ContentItem,
        manifest: Option<FileManifest>,
    ) -> anyhow::Result<bool> {
        let id = item.id();
        if self.store.contains(id)? {
            return Ok(false);
        }
        match self.commit.commit(item, manifest) {
            Ok(_) => Ok(true),
            Err(_) if self.store.contains(id).unwrap_or(false) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    fn remember_event_key(&self, key: Option<String>) {
        if let Some(key) = key {
            self.seen_events.lock().insert(key, Instant::now());
        }
    }

    fn prune_seen(&self) {
        let now = Instant::now();
        let ttl = std::time::Duration::from_secs(10 * 60);
        let mut seen = self.seen_events.lock();
        seen.retain(|_, at| now.duration_since(*at) < ttl);
        const MAX: usize = 4096;
        if seen.len() > MAX {
            let mut entries: Vec<_> = seen.iter().map(|(k, t)| (k.clone(), *t)).collect();
            entries.sort_by_key(|(_, t)| *t);
            for (key, _) in entries.into_iter().take(seen.len() - MAX) {
                seen.remove(&key);
            }
        }
    }

    fn materialize(
        &self,
        draft: ContentDraft,
        content: NormalizedContent,
    ) -> anyhow::Result<Option<ContentItem>> {
        let now = asterism_platform::now_ms();
        match content {
            NormalizedContent::Files { .. } => {
                anyhow::bail!("files must go through submit_files")
            }
            NormalizedContent::Image { png, width, height, dedup_tag, flags, source_app } => {
                let blob_id = self.store.put_blob(&png)?;
                let mut item = NormalizedContent::Image {
                    png: Vec::new(),
                    width,
                    height,
                    dedup_tag,
                    flags,
                    source_app,
                }
                .into_item(self.device_id, now);
                if let Some(kind) = draft.kind_override {
                    item.set_kind(kind);
                }
                item.set_payload(PayloadRef::Blob { blob_id }, png.len() as u64, png.len() as u64);
                apply_provenance(&mut item, &draft);
                apply_remote_policy(&mut item);
                apply_hmac(&mut item, &self.avk.read());
                Ok(Some(item))
            }
            other => {
                let mut item = other.into_item(self.device_id, now);
                if let Some(kind) = draft.kind_override {
                    item.set_kind(kind);
                }
                apply_provenance(&mut item, &draft);
                apply_remote_policy(&mut item);
                apply_hmac(&mut item, &self.avk.read());
                Ok(Some(item))
            }
        }
    }

    fn materialize_files(
        &self,
        sources: Vec<std::path::PathBuf>,
        flags: ContentFlags,
        source_app: Option<String>,
    ) -> anyhow::Result<Option<(ContentItem, FileManifest)>> {
        let manifest = asterism_clipboard::preflight_paths(&sources)?;
        let file_count = manifest.entries.iter().filter(|e| e.kind.is_file()).count() as u64;
        let logical_size: u64 = manifest.entries.iter().map(|e| e.size).sum();
        if logical_size > asterism_core::policy::LOCAL_MAX_MATERIALIZE_BYTES {
            anyhow::bail!("file item exceeds local materialize limit");
        }
        let largest = manifest.entries.iter().map(|e| e.size).max().unwrap_or(0);
        let remote = asterism_core::RemotePolicy::default();
        let flags = if remote
            .check_preflight_ext(ContentKind::Files, file_count, logical_size, largest)
            .is_ok()
        {
            flags | ContentFlags::REMOTE_ALLOWED
        } else {
            flags
        };
        let fingerprint = {
            let mut buf = Vec::new();
            for entry in &manifest.entries {
                buf.extend_from_slice(entry.relative_path.as_bytes());
                buf.push(0);
                buf.extend_from_slice(&entry.size.to_le_bytes());
            }
            buf
        };
        let now = asterism_platform::now_ms();
        let mut item = NormalizedContent::Files {
            paths: sources.clone(),
            manifest: manifest.clone(),
            dedup_tag: asterism_crypto::local_dedup_tag(&fingerprint),
            flags,
            source_app,
        }
        .into_item(self.device_id, now);
        let cache = self.paths.item_cache(item.id());
        materialize_to_cache(&cache, &sources)?;
        item.metadata_mut().local_cache_rel = Some(item.id().to_string());
        apply_hmac(&mut item, &self.avk.read());
        Ok(Some((item, manifest)))
    }
}

fn event_key(draft: &ContentDraft) -> Option<String> {
    if draft.source_event_id.is_empty() {
        None
    } else {
        Some(format!("{}:{}", draft.producer_plugin_id, draft.source_event_id))
    }
}

pub struct RemoteItemSpec {
    pub id: asterism_core::id::ContentId,
    pub origin: DeviceId,
    pub kind: ContentKind,
    pub flags: ContentFlags,
    pub tag: [u8; 32],
    pub metadata: ItemMetadata,
    pub from_lan: bool,
    pub created_at_ms: Option<i64>,
    pub logical_size: Option<u64>,
    pub payload_size: Option<u64>,
}

pub enum RemoteItemBody {
    Bytes(Vec<u8>),
    Files(FileManifest),
}

fn apply_provenance(item: &mut ContentItem, draft: &ContentDraft) {
    let provenance = draft.provenance();
    let meta = item.metadata_mut();
    meta.producer_plugin_id = Some(provenance.producer_plugin_id);
    meta.source_event_id = Some(provenance.source_event_id);
    meta.change_token = provenance.change_token;
    meta.parent_content_id = provenance.parent_content_id;
    if let Some(mime) = draft.mime_hint.clone() {
        meta.mime_hint = Some(mime.clone());
        if let Some(image) = meta.image.as_mut() {
            image.mime = mime;
        }
    }
}

fn apply_remote_policy(item: &mut ContentItem) {
    let remote = asterism_core::RemotePolicy::default();
    let mut flags = item.flags();
    flags.remove(ContentFlags::REMOTE_ALLOWED);
    if remote.check_preflight_ext(item.kind(), 0, item.logical_size(), 0).is_ok() {
        flags.insert(ContentFlags::REMOTE_ALLOWED);
    }
    item.set_flags(flags);
}

fn apply_hmac(item: &mut ContentItem, avk: &AccountVaultKey) {
    item.set_dedup_tag(avk.dedup_tag(&item.dedup_tag()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_core::content::ItemMetadata;
    use asterism_core::id::ContentId;
    use bytes::Bytes;

    fn harness() -> (tempfile::TempDir, Arc<Ingestion>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
            config_dir: dir.path().join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let avk = Arc::new(RwLock::new(AccountVaultKey::generate()));
        let ingestion = Ingestion::new(store, paths, DeviceId::new(), avk);
        (dir, ingestion)
    }

    fn draft(event: &str) -> ContentDraft {
        ContentDraft {
            producer_plugin_id: "asterism.clipboard".into(),
            source_device_id: DeviceId::new(),
            source_event_id: event.into(),
            change_token: None,
            kind_hint: ContentKind::Text,
            source_app: None,
            parent_content_id: None,
            kind_override: None,
            mime_hint: None,
        }
    }

    #[test]
    fn same_source_event_is_ignored() {
        let (_dir, ingestion) = harness();
        let content = NormalizedContent::Text {
            text: "hello".into(),
            dedup_tag: asterism_crypto::local_dedup_tag(b"hello"),
            flags: ContentFlags::REMOTE_ALLOWED,
            source_app: None,
        };
        let first = ingestion.submit_local(draft("tok-1"), content.clone(), false).unwrap();
        let second = ingestion.submit_local(draft("tok-1"), content, false).unwrap();
        assert!(matches!(first, IngestionOutcome::Committed(_)));
        assert_eq!(second, IngestionOutcome::Ignored(DedupDecision::SameSourceEvent));
    }

    #[test]
    fn different_events_create_two_items() {
        let (_dir, ingestion) = harness();
        let content = NormalizedContent::Text {
            text: "hello".into(),
            dedup_tag: asterism_crypto::local_dedup_tag(b"hello"),
            flags: ContentFlags::REMOTE_ALLOWED,
            source_app: None,
        };
        ingestion.submit_local(draft("tok-1"), content.clone(), false).unwrap();
        ingestion.submit_local(draft("tok-2"), content, false).unwrap();
        assert_eq!(
            ingestion.store.history(asterism_storage::HistoryQuery::recent(10)).unwrap().len(),
            2
        );
    }

    #[test]
    fn self_write_is_ignored_after_normalize() {
        let (_dir, ingestion) = harness();
        let captured = asterism_clipboard::CapturedClipboard {
            change_token: 9,
            source_app: None,
            formats: vec!["public.utf8-plain-text".into()],
            text: Some("mine".into()),
            image: None,
            files: vec![],
            sensitive: false,
        };
        let tag = asterism_crypto::local_dedup_tag(b"mine");
        let guard = SelfWriteGuard::default();
        guard.remember(ContentId::new(), tag);
        let outcome = ingestion
            .submit_capture(
                ContentDraft {
                    producer_plugin_id: "asterism.clipboard".into(),
                    source_device_id: DeviceId::new(),
                    source_event_id: "9".into(),
                    change_token: Some(9),
                    kind_hint: ContentKind::Text,
                    source_app: None,
                    parent_content_id: None,
                    kind_override: None,
                    mime_hint: None,
                },
                captured,
                &guard,
            )
            .unwrap();
        assert_eq!(outcome, IngestionOutcome::Ignored(DedupDecision::SelfWrite));
    }

    #[test]
    fn remote_same_id_is_idempotent() {
        let (_dir, ingestion) = harness();
        let item = ContentItem::from_trusted(
            ContentId::new(),
            DeviceId::new(),
            ContentKind::Text,
            1,
            1,
            1,
            [1; 32],
            ContentFlags::FROM_REMOTE | ContentFlags::REMOTE_ALLOWED,
            asterism_core::ContentStatus::SyncedToHub,
            ItemMetadata::default(),
            PayloadRef::Inline { bytes: Bytes::from_static(b"x") },
            Bytes::new(),
        );
        assert!(ingestion.commit_remote(item.clone(), None).unwrap());
        assert!(!ingestion.commit_remote(item, None).unwrap());
    }

    #[test]
    fn mime_hint_overrides_image_png_for_gif() {
        let (_dir, ingestion) = harness();
        let content = NormalizedContent::Image {
            png: vec![0x89, b'P', b'N', b'G'],
            width: 1,
            height: 1,
            dedup_tag: asterism_crypto::local_dedup_tag(&[1]),
            flags: ContentFlags::REMOTE_ALLOWED,
            source_app: None,
        };
        let mut draft = draft("gif-1");
        draft.kind_hint = ContentKind::Gif;
        draft.kind_override = Some(ContentKind::Gif);
        draft.mime_hint = Some("image/gif".into());
        let IngestionOutcome::Committed(id) =
            ingestion.submit_local(draft, content, false).unwrap()
        else {
            panic!("expected commit");
        };
        let item = ingestion.store().get(id).unwrap();
        assert_eq!(item.kind(), ContentKind::Gif);
        assert_eq!(item.metadata().mime_hint.as_deref(), Some("image/gif"));
        assert_eq!(item.metadata().image.as_ref().map(|m| m.mime.as_str()), Some("image/gif"));
    }

    #[test]
    fn seen_events_are_capped() {
        let (_dir, ingestion) = harness();
        for i in 0..4100 {
            let d = draft(&format!("e-{i}"));
            let _ = ingestion.decide(&d, false, None);
            ingestion.remember_event_key(event_key(&d));
        }
        ingestion.prune_seen();
        assert!(ingestion.seen_events.lock().len() <= 4096);
    }

    #[test]
    fn assemble_remote_strips_caller_remote_allowed_then_reapplies_policy() {
        let (_dir, ingestion) = harness();
        let (item, _) = ingestion
            .assemble_remote(
                RemoteItemSpec {
                    id: ContentId::new(),
                    origin: DeviceId::new(),
                    kind: ContentKind::Text,
                    flags: ContentFlags::REMOTE_ALLOWED,
                    tag: [4; 32],
                    metadata: ItemMetadata::default(),
                    from_lan: true,
                    created_at_ms: Some(1),
                    logical_size: Some(1),
                    payload_size: Some(1),
                },
                RemoteItemBody::Bytes(b"x".to_vec()),
            )
            .unwrap();
        assert!(item.flags().contains(ContentFlags::FROM_REMOTE));
        assert!(item.flags().contains(ContentFlags::REMOTE_ALLOWED));
        assert_eq!(item.status(), ContentStatus::DeliveredToPeer);
    }
}
