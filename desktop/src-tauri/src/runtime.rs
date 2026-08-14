use std::sync::Arc;

use asterism_clipboard::normalize::NormalizedContent;
use asterism_clipboard::{NativeClipboard, SelfWriteGuard};
use asterism_core::content::{ContentFlags, ContentItem, ContentKind, PayloadRef};
use asterism_core::{ContentDraft, IngestionOutcome};
use asterism_domain_runtime::{
    CapturePlugin, DomainFoundationPlugin, HistoryPlugin, Ingestion, MediaPlugin,
};
use asterism_kernel::{BootPlan, MountedRuntime, ServiceRegistry};
use asterism_platform::hardening::CrashGuard;
use asterism_platform::{AppPaths, LocalIdentity, LocalVault};
use asterism_plugin_api::{ActionRegistry, ContentReadGrant, PermissionBroker};
use asterism_storage::Store;
use tauri::AppHandle;

use crate::plugins::{DesktopActionPlugin, DesktopClipboardPlugin, DesktopSyncPlugin};
use crate::settings::SyncSettings;
use crate::sync_engine::SyncHandle;

pub struct DesktopState {
    /// 声明在前，保证 Drop 时最后拆 Runtime（先关 channel，再 join 线程）。
    _runtime: MountedRuntime,
    _crash: CrashGuard,
    pub store: Arc<Store>,
    pub ingestion: Arc<Ingestion>,
    #[allow(dead_code)]
    pub broker: PermissionBroker,
    pub guard: Arc<SelfWriteGuard>,
    pub identity: LocalIdentity,
    pub paths: AppPaths,
    pub clipboard: NativeClipboard,
    pub vault: parking_lot::RwLock<LocalVault>,
    pub avk: Arc<parking_lot::RwLock<asterism_crypto::AccountVaultKey>>,
    pub sync: SyncHandle,
    #[allow(dead_code)]
    pub cache_pin: Arc<parking_lot::RwLock<Option<String>>>,
    pub actions: Arc<ActionRegistry>,
    #[allow(dead_code)]
    pub boot_order: Vec<&'static str>,
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

        let mut registry = ServiceRegistry::new();
        registry.provide(Arc::clone(&ingestion)).map_err(|err| anyhow::anyhow!(err))?;
        let actions = Arc::new(ActionRegistry::new());
        registry.provide(Arc::clone(&actions)).map_err(|err| anyhow::anyhow!(err))?;

        let mut plan = BootPlan::new("asterism-desktop");
        plan.push(DomainFoundationPlugin);
        plan.push(HistoryPlugin);
        plan.push(DesktopSyncPlugin {
            identity: identity.clone(),
            vault: asterism_crypto::AccountVaultKey::from_bytes(*vault.avk.as_bytes()),
            store: Arc::clone(&store),
            ingestion: Arc::clone(&ingestion),
            paths: paths.clone(),
            guard: Arc::clone(&guard),
            cache_pin: Arc::clone(&cache_pin),
            settings,
            app: app.clone(),
        });
        plan.push(DesktopClipboardPlugin {
            ingestion: Arc::clone(&ingestion),
            guard: Arc::clone(&guard),
            cache_pin: Arc::clone(&cache_pin),
            app: app.clone(),
        });
        plan.push(CapturePlugin);
        plan.push(MediaPlugin);
        plan.push(DesktopActionPlugin {
            ingestion: Arc::clone(&ingestion),
            store: Arc::clone(&store),
            paths: paths.clone(),
            guard: Arc::clone(&guard),
            cache_pin: Arc::clone(&cache_pin),
        });
        let runtime = plan.mount_with(registry).map_err(|err| anyhow::anyhow!(err))?;
        tracing::info!(order = ?runtime.boot_order(), "desktop boot plan");
        let boot_order = runtime.boot_order().to_vec();
        let sync = runtime.registry.require::<SyncHandle>().map_err(|err| anyhow::anyhow!(err))?;
        let sync = (*sync).clone();

        Ok(Self {
            _runtime: runtime,
            _crash: crash,
            store,
            ingestion,
            broker: PermissionBroker::host(),
            guard,
            identity,
            paths,
            clipboard: NativeClipboard,
            vault: parking_lot::RwLock::new(vault),
            avk,
            sync,
            cache_pin,
            actions,
            boot_order,
        })
    }

    pub fn begin_capture(&self) -> CaptureSession {
        CaptureSession { scope: self._runtime.scope.fork() }
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
    let _session = state.begin_capture();
    let local_tag = asterism_crypto::local_dedup_tag(&png);
    let content = NormalizedContent::Image {
        png,
        width,
        height,
        dedup_tag: local_tag,
        flags: ContentFlags::REMOTE_ALLOWED,
        source_app: Some("asterism".into()),
    };
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
    match state.ingestion.submit_local(draft, content, false)? {
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
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.scope.dispose();
    }
}

pub fn item_to_clipboard(
    item: &ContentItem,
    store: &Store,
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

fn load_bytes(item: &ContentItem, store: &Store) -> anyhow::Result<Vec<u8>> {
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
        let content = NormalizedContent::Text {
            text: "copy again".into(),
            dedup_tag: asterism_crypto::local_dedup_tag(b"copy again"),
            flags: ContentFlags::REMOTE_ALLOWED,
            source_app: None,
        };
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
        let first = ingestion.submit_local(draft("1"), content.clone(), false).unwrap();
        let second = ingestion.submit_local(draft("2"), content, false).unwrap();
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
        let IngestionOutcome::Committed(id) = ingestion
            .submit_files(draft, vec![src.clone()], ContentFlags::empty(), None, false)
            .unwrap()
        else {
            panic!("expected commit");
        };
        let item = asterism_domain_runtime::ContentCommandService::new(&ingestion).get(id).unwrap();
        let grant = PermissionBroker::host().grant_copy(item.id(), item.kind()).unwrap();
        let written = item_to_clipboard(&item, &store, &paths, &grant).unwrap();
        let file = paths.item_cache(id).join("note.txt");
        assert_eq!(written.dedup_tag(), asterism_clipboard::files_local_dedup_tag(&[file]));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
