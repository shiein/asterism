use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use asterism_clipboard::normalize::NormalizedContent;
use asterism_clipboard::{NativeClipboard, SelfWriteGuard};
use asterism_core::content::{ContentFlags, ContentItem, ContentKind, PayloadRef};
use asterism_core::{ContentDraft, IngestionOutcome};
use asterism_domain_runtime::{
    CapturePlugin, DomainFoundationPlugin, HistoryPlugin, Ingestion, MediaPlugin,
};
use asterism_domain_runtime::{ContentLookup, DomainReadStore, DomainStore};
use asterism_kernel::{BootPlan, MountedRuntime, ServiceRegistry};
use asterism_platform::hardening::CrashGuard;
use asterism_platform::{AppPaths, LocalIdentity, LocalVault};
use asterism_plugin_api::{ActionRegistry, ContentReadGrant, PermissionBroker};
use asterism_storage::Store;
use tauri::AppHandle;

use crate::host::{HostAccount, HostClipboard, HostPaths};
use crate::plugins::{DesktopActionPlugin, DesktopClipboardPlugin, DesktopSyncPlugin};
use crate::settings::SyncSettings;
use crate::sync_engine::SyncHandle;

pub struct DesktopState {
    _crash: CrashGuard,
    pub(crate) ingestion: Arc<Ingestion>,
    pub(crate) broker: PermissionBroker,
    pub(crate) guard: Arc<SelfWriteGuard>,
    pub(crate) identity: LocalIdentity,
    pub(crate) paths: AppPaths,
    pub(crate) clipboard: NativeClipboard,
    pub(crate) vault: parking_lot::RwLock<LocalVault>,
    pub(crate) avk: Arc<parking_lot::RwLock<asterism_crypto::AccountVaultKey>>,
    pub(crate) sync: SyncHandle,
    pub(crate) actions: Arc<ActionRegistry>,
    pub(crate) recording: Arc<RecordingController>,
    /// 最后析构：先释放 sender / handler，再撤销 Registry，最后 join 线程。
    _runtime: MountedRuntime,
}

impl DesktopState {
    pub fn start(app: AppHandle) -> anyhow::Result<Self> {
        let paths = AppPaths::detect();
        paths.ensure()?;
        let (crash, unclean) = CrashGuard::acquire(&paths.data_dir)?;
        if unclean {
            tracing::warn!("previous session did not exit cleanly; temp capture purged");
        }
        let identity = LocalIdentity::load_or_create(&paths.config_dir)?;
        let vault = LocalVault::load_or_create(&paths.config_dir)?;
        let store = Store::open(&paths.data_dir)?;
        let guard = Arc::new(SelfWriteGuard::default());
        let cache_pin = Arc::new(parking_lot::RwLock::new(None));
        let avk = Arc::new(parking_lot::RwLock::new(asterism_crypto::AccountVaultKey::from_bytes(
            *vault.avk.as_bytes(),
        )));
        let ingestion =
            Ingestion::new(Arc::clone(&store), paths.clone(), identity.device_id, Arc::clone(&avk));
        let settings = SyncSettings::load(&paths.config_dir);

        let registry = ServiceRegistry::new();
        registry.provide(Arc::clone(&ingestion)).map_err(|err| anyhow::anyhow!(err))?;
        let actions = Arc::new(ActionRegistry::new());
        registry.provide(Arc::clone(&actions)).map_err(|err| anyhow::anyhow!(err))?;
        registry
            .provide(Arc::new(HostPaths { paths: paths.clone() }))
            .map_err(|err| anyhow::anyhow!(err))?;
        registry
            .provide_gated(
                Arc::new(HostClipboard {
                    guard: Arc::clone(&guard),
                    cache_pin: Arc::clone(&cache_pin),
                }),
                "clipboard.write",
            )
            .map_err(|err| anyhow::anyhow!(err))?;
        registry
            .provide_gated(
                Arc::new(HostAccount {
                    identity: identity.clone(),
                    vault: asterism_crypto::AccountVaultKey::from_bytes(*vault.avk.as_bytes()),
                    settings,
                }),
                "credential.account",
            )
            .map_err(|err| anyhow::anyhow!(err))?;
        registry
            .provide_gated(DomainReadStore::wrap(Arc::clone(&store)), "content.read")
            .map_err(|err| anyhow::anyhow!(err))?;
        registry
            .provide_gated(DomainStore::wrap(Arc::clone(&store)), "storage.domain")
            .map_err(|err| anyhow::anyhow!(err))?;

        let mut plan = BootPlan::new("asterism-desktop");
        plan.push(DomainFoundationPlugin);
        plan.push(HistoryPlugin);
        plan.push(DesktopSyncPlugin { app: app.clone() });
        plan.push(DesktopClipboardPlugin { app: app.clone() });
        plan.push(CapturePlugin);
        plan.push(MediaPlugin);
        plan.push(DesktopActionPlugin);
        let runtime = plan.mount_with(registry).map_err(|err| anyhow::anyhow!(err))?;
        tracing::info!(order = ?runtime.boot_order(), "desktop boot plan");
        let sync = runtime.registry.require::<SyncHandle>().map_err(|err| anyhow::anyhow!(err))?;
        let sync = (*sync).clone();

        Ok(Self {
            _crash: crash,
            ingestion,
            broker: PermissionBroker::host(),
            guard,
            identity,
            paths,
            clipboard: NativeClipboard,
            vault: parking_lot::RwLock::new(vault),
            avk,
            sync,
            actions,
            recording: Arc::new(RecordingController::default()),
            _runtime: runtime,
        })
    }

    pub fn begin_capture(&self) -> CaptureSession {
        CaptureSession { scope: self._runtime.scope.fork(), temps: Vec::new() }
    }

    pub fn query(&self) -> asterism_domain_runtime::ContentQueryService<'_> {
        asterism_domain_runtime::ContentQueryService::new(&self.ingestion)
    }
}

#[derive(Default)]
pub struct RecordingController {
    next_id: AtomicU64,
    active: parking_lot::Mutex<Option<ActiveRecording>>,
}

struct ActiveRecording {
    id: u64,
    stop: Arc<AtomicBool>,
}

impl RecordingController {
    pub fn begin(self: &Arc<Self>) -> anyhow::Result<RecordingLease> {
        let mut active = self.active.lock();
        if active.is_some() {
            anyhow::bail!("another recording is already active");
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let stop = Arc::new(AtomicBool::new(false));
        *active = Some(ActiveRecording { id, stop: Arc::clone(&stop) });
        Ok(RecordingLease { controller: Arc::clone(self), id, stop })
    }

    pub fn request_stop(&self) -> bool {
        let active = self.active.lock();
        let Some(active) = active.as_ref() else {
            return false;
        };
        active.stop.store(true, Ordering::Release);
        true
    }

    fn clear(&self, id: u64) {
        let mut active = self.active.lock();
        if active.as_ref().is_some_and(|item| item.id == id) {
            *active = None;
        }
    }
}

pub struct RecordingLease {
    controller: Arc<RecordingController>,
    id: u64,
    stop: Arc<AtomicBool>,
}

impl RecordingLease {
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

impl Drop for RecordingLease {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.controller.clear(self.id);
    }
}

pub fn ingest_image(
    state: &DesktopState,
    png: Vec<u8>,
    width: u32,
    height: u32,
    kind: ContentKind,
    mime_hint: Option<&str>,
    producer: &str,
) -> anyhow::Result<asterism_core::ContentId> {
    let draft = ContentDraft {
        producer_plugin_id: producer.into(),
        source_device_id: state.identity.device_id,
        source_event_id: asterism_core::ContentId::new().to_string(),
        change_token: None,
        kind_hint: kind,
        source_app: Some("asterism".into()),
        parent_content_id: None,
        kind_override: Some(kind),
        mime_hint: mime_hint.map(str::to_owned),
    };
    match state.ingestion.submit_image(draft, png, width, height)? {
        IngestionOutcome::Committed(id) => {
            state.sync.drain_outbox();
            Ok(id)
        }
        IngestionOutcome::Ignored(_) => anyhow::bail!("capture ignored as duplicate source event"),
        IngestionOutcome::RejectedPolicy => anyhow::bail!("capture rejected by policy"),
    }
}

pub struct CaptureSession {
    scope: asterism_kernel::Scope,
    temps: Vec<std::path::PathBuf>,
}

impl CaptureSession {
    pub fn cancel_token(&self) -> asterism_kernel::CancelToken {
        self.scope.cancel_token()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn track_temp(&mut self, path: std::path::PathBuf) {
        self.temps.push(path);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn temp_dir(&mut self, paths: &AppPaths) -> std::io::Result<std::path::PathBuf> {
        let dir = paths.cache_dir.join("capture-sessions").join(self.scope.id().raw().to_string());
        std::fs::create_dir_all(&dir)?;
        self.track_temp(dir.clone());
        Ok(dir)
    }

    pub fn is_cancelled(&self) -> bool {
        self.scope.is_closed()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.scope.dispose();
        for path in self.temps.drain(..).rev() {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

pub fn item_to_clipboard(
    item: &ContentItem,
    store: &impl ContentLookup,
    paths: &AppPaths,
    grant: &ContentReadGrant,
) -> anyhow::Result<NormalizedContent> {
    if !grant.is_valid(item.id()) {
        anyhow::bail!("content read grant invalid");
    }
    if !matches!(item.kind(), ContentKind::Files) && item.payload_size() > grant.max_bytes() {
        anyhow::bail!("content exceeds grant max_bytes");
    }
    match item.kind() {
        ContentKind::Text => {
            let text = match &item.payload_ref() {
                PayloadRef::Inline { bytes } => String::from_utf8_lossy(bytes).into_owned(),
                PayloadRef::Blob { blob_id } => {
                    String::from_utf8_lossy(&store.get_blob(blob_id)?).into_owned()
                }
                PayloadRef::FileManifest { .. } => anyhow::bail!("text item has file payload"),
            };
            Ok(NormalizedContent::Text {
                text: text.clone(),
                dedup_tag: asterism_crypto::local_dedup_tag(text.as_bytes()),
                flags: item.flags(),
                source_app: item.metadata().source_app.clone(),
            })
        }
        ContentKind::Image | ContentKind::Screenshot => {
            let png = load_bytes(item, store)?;
            let (width, height) =
                item.metadata().image.as_ref().map(|i| (i.width, i.height)).unwrap_or((0, 0));
            Ok(NormalizedContent::Image {
                png: png.clone(),
                width,
                height,
                dedup_tag: asterism_crypto::local_dedup_tag(&png),
                flags: item.flags(),
                source_app: item.metadata().source_app.clone(),
            })
        }
        ContentKind::Gif | ContentKind::Video => {
            let bytes = load_bytes(item, store)?;
            let cache = paths.item_cache(item.id());
            std::fs::create_dir_all(&cache)?;
            let name = if item.kind() == ContentKind::Gif {
                "clip.gif"
            } else if item.metadata().mime_hint.as_deref() == Some("video/x-msvideo") {
                "clip.avi"
            } else {
                "clip.mp4"
            };
            let path = cache.join(name);
            std::fs::write(&path, &bytes)?;
            let dedup_tag = asterism_clipboard::files_local_dedup_tag(std::slice::from_ref(&path));
            Ok(NormalizedContent::Files {
                paths: vec![path],
                manifest: asterism_core::FileManifest {
                    id: asterism_core::ManifestId::new(),
                    root_name: name.into(),
                    entries: vec![asterism_core::FileEntry {
                        relative_path: name.into(),
                        size: bytes.len() as u64,
                        kind: asterism_core::FileEntryKind::File,
                        blob_id: None,
                    }],
                    unsupported: Vec::new(),
                },
                dedup_tag,
                flags: item.flags() | ContentFlags::FROM_REMOTE,
                source_app: item.metadata().source_app.clone(),
            })
        }
        ContentKind::Files => {
            let cache = item
                .metadata()
                .local_cache_rel
                .as_ref()
                .map(|rel| paths.cache_dir.join("items").join(rel))
                .unwrap_or_else(|| paths.item_cache(item.id()));
            let mut file_paths = Vec::new();
            if cache.exists() {
                for entry in std::fs::read_dir(&cache)? {
                    file_paths.push(entry?.path());
                }
            }
            if file_paths.is_empty() {
                anyhow::bail!("cached files missing");
            }
            Ok(NormalizedContent::Files {
                paths: file_paths.clone(),
                manifest: if let PayloadRef::FileManifest { manifest_id } = item.payload_ref() {
                    store.load_manifest(*manifest_id)?
                } else {
                    anyhow::bail!("files item missing manifest")
                },
                dedup_tag: asterism_clipboard::files_local_dedup_tag(&file_paths),
                flags: item.flags() | ContentFlags::FROM_REMOTE,
                source_app: item.metadata().source_app.clone(),
            })
        }
        other => anyhow::bail!("cannot copy kind {other}"),
    }
}

fn load_bytes(item: &ContentItem, store: &impl ContentLookup) -> anyhow::Result<Vec<u8>> {
    match &item.payload_ref() {
        PayloadRef::Inline { bytes } => Ok(bytes.to_vec()),
        PayloadRef::Blob { blob_id } => Ok(store.get_blob(blob_id)?),
        PayloadRef::FileManifest { .. } => anyhow::bail!("expected blob payload"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_core::id::DeviceId;

    #[test]
    fn recording_controller_allows_only_one_active_session() {
        let controller = Arc::new(RecordingController::default());
        let first = controller.begin().unwrap();
        assert!(controller.begin().is_err());
        assert!(controller.request_stop());
        assert!(first.stop_requested());
        drop(first);
        assert!(!controller.request_stop());
        assert!(controller.begin().is_ok());
    }

    #[test]
    fn repeated_user_copy_creates_a_new_history_item() {
        let root = std::env::temp_dir()
            .join(format!("asterism-repeat-copy-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let avk = Arc::new(parking_lot::RwLock::new(asterism_crypto::AccountVaultKey::generate()));
        let ingestion = Ingestion::new(Arc::clone(&store), paths.clone(), DeviceId::new(), avk);
        let draft = |event: &str| ContentDraft {
            producer_plugin_id: "asterism.clipboard".into(),
            source_device_id: DeviceId::new(),
            source_event_id: event.into(),
            change_token: None,
            kind_hint: ContentKind::Text,
            source_app: None,
            parent_content_id: None,
            kind_override: None,
            mime_hint: None,
        };
        let first = ingestion.submit_text(draft("1"), "copy again".into()).unwrap();
        let second = ingestion.submit_text(draft("2"), "copy again".into()).unwrap();
        assert!(matches!(first, IngestionOutcome::Committed(_)));
        assert!(matches!(second, IngestionOutcome::Committed(_)));
        assert_ne!(first, second);
        assert_eq!(store.history(asterism_storage::HistoryQuery::recent(10)).unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_clipboard_tag_matches_watcher_path_fingerprint() {
        let root = std::env::temp_dir()
            .join(format!("asterism-file-tag-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let src = root.join("note.txt");
        std::fs::write(&src, b"hello").unwrap();
        let avk = Arc::new(parking_lot::RwLock::new(asterism_crypto::AccountVaultKey::generate()));
        let ingestion = Ingestion::new(Arc::clone(&store), paths.clone(), DeviceId::new(), avk);
        let draft = ContentDraft {
            producer_plugin_id: "asterism.clipboard".into(),
            source_device_id: DeviceId::new(),
            source_event_id: "file-1".into(),
            change_token: None,
            kind_hint: ContentKind::Files,
            source_app: None,
            parent_content_id: None,
            kind_override: None,
            mime_hint: None,
        };
        let IngestionOutcome::Committed(id) =
            ingestion.submit_files(draft, vec![src.clone()], None).unwrap()
        else {
            panic!("expected commit");
        };
        let grant = PermissionBroker::host().grant_read(id).unwrap();
        let item = asterism_domain_runtime::ContentCommandService::new(&ingestion)
            .get(&grant, id)
            .unwrap();
        let grant = PermissionBroker::host().grant_copy(item.id(), item.kind()).unwrap();
        let written =
            item_to_clipboard(&item, &DomainReadStore::wrap(Arc::clone(&store)), &paths, &grant)
                .unwrap();
        let file = paths.item_cache(id).join("note.txt");
        assert_eq!(written.dedup_tag(), asterism_clipboard::files_local_dedup_tag(&[file]));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_session_drop_removes_tracked_temp() {
        let root = std::env::temp_dir()
            .join(format!("asterism-capture-lease-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let dir = {
            let mut session =
                CaptureSession { scope: asterism_kernel::Scope::root(), temps: Vec::new() };
            let dir = session.temp_dir(&paths).unwrap();
            std::fs::write(dir.join("frame.bin"), b"x").unwrap();
            assert!(dir.exists());
            dir
        };
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
