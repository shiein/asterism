use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use asterism_clipboard::{
    CapturedClipboard, ClipboardBackend, ClipboardEvent, NativeClipboard, SelfWriteGuard,
    WatcherConfig, spawn_watcher,
};
use asterism_core::action::{ActionError, ActionId, ActionResult};
use asterism_core::builtin_actions;
use asterism_core::{ContentDraft, ContentId, IngestionOutcome};
use asterism_domain_runtime::{HistoryApi, Ingestion};
use asterism_kernel::{Health, KernelManifest, MountContext, OsThreadLease, Plugin, Result};
use asterism_platform::{AppPaths, LocalIdentity};
use asterism_plugin_api::{ActionKey, ActionRegistry, PermissionBroker, PluginManifest, TrustTier};
use asterism_storage::Store;
use parking_lot::{Mutex, RwLock};
use tauri::{AppHandle, Emitter};

use crate::runtime::item_to_clipboard;
use crate::settings::SyncSettings;
use crate::sync_engine::{self, SyncHandle};

pub struct DesktopSyncPlugin {
    pub identity: LocalIdentity,
    pub vault: asterism_crypto::AccountVaultKey,
    pub store: Arc<Store>,
    pub ingestion: Arc<Ingestion>,
    pub paths: asterism_platform::AppPaths,
    pub guard: Arc<SelfWriteGuard>,
    pub cache_pin: Arc<RwLock<Option<String>>>,
    pub settings: SyncSettings,
    pub app: AppHandle,
}

impl DesktopSyncPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.sync",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.domain"],
        permissions: &["content.read"],
    };
}

impl Plugin for DesktopSyncPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let app = self.app.clone();
        let (handle, thread) = sync_engine::spawn(
            self.identity.clone(),
            self.vault.clone(),
            Arc::clone(&self.store),
            Arc::clone(&self.ingestion),
            self.paths.clone(),
            Arc::clone(&self.guard),
            Arc::clone(&self.cache_pin),
            self.settings.clone(),
            move || {
                let _ = app.emit("history-changed", ());
            },
        )
        .map_err(|err| asterism_kernel::KernelError::Mount(err.to_string()))?;
        ctx.adopt_thread(OsThreadLease::from_join("asterism-sync", thread));
        ctx.provide(Arc::new(handle))?;
        ctx.health().set(Self::MANIFEST.id, Health::Ready);
        Ok(())
    }
}

pub struct DesktopClipboardPlugin {
    pub ingestion: Arc<Ingestion>,
    pub guard: Arc<SelfWriteGuard>,
    pub cache_pin: Arc<RwLock<Option<String>>>,
    pub app: AppHandle,
}

impl DesktopClipboardPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.clipboard",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.domain", "asterism.sync"],
        permissions: &["clipboard.read", "clipboard.write"],
    };
}

impl Plugin for DesktopClipboardPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let sync = ctx.require::<SyncHandle>()?;
        let ingestion = Arc::clone(&self.ingestion);
        let app = self.app.clone();
        let cache_pin = Arc::clone(&self.cache_pin);
        let (file_tx, file_rx) = mpsc::channel::<CapturedClipboard>();
        let file_lease = {
            let ingestion = Arc::clone(&ingestion);
            let sync = sync.clone();
            let app = app.clone();
            let cache_pin = Arc::clone(&cache_pin);
            let guard = Arc::clone(&self.guard);
            let thread = std::thread::Builder::new()
                .name("asterism-files".into())
                .spawn(move || {
                    while let Ok(captured) = file_rx.recv() {
                        submit_capture(&ingestion, &guard, &sync, &app, &cache_pin, captured);
                    }
                })
                .map_err(|err| asterism_kernel::KernelError::Mount(err.to_string()))?;
            OsThreadLease::from_join("asterism-files", thread)
        };
        let file_tx = Arc::new(Mutex::new(Some(file_tx)));
        let file_tx_watch = Arc::clone(&file_tx);
        let ingest_watch = Arc::clone(&ingestion);
        let guard = Arc::clone(&self.guard);
        let (watcher, watcher_thread) = spawn_watcher(
            WatcherConfig {
                device_id: ingestion.device_id(),
                policy: asterism_core::CapturePolicy::default(),
                remote: asterism_core::RemotePolicy::default(),
                ..WatcherConfig::default()
            },
            Arc::clone(&self.guard),
            move |event| match event {
                ClipboardEvent::Captured { captured, .. } => {
                    if !captured.files.is_empty() {
                        if let Some(tx) = file_tx_watch.lock().as_ref()
                            && tx.send(captured).is_err()
                        {
                            tracing::warn!("file persist worker stopped");
                        }
                    } else {
                        submit_capture(&ingest_watch, &guard, &sync, &app, &cache_pin, captured);
                    }
                }
                ClipboardEvent::Ignored => {}
                ClipboardEvent::Error(message) => {
                    tracing::warn!(%message, "clipboard watcher error");
                }
            },
        );
        let watcher_lease = OsThreadLease::from_join("asterism-clipboard", watcher_thread);
        ctx.adopt_thread(file_lease);
        ctx.adopt_thread(watcher_lease);
        ctx.on_drop(move || {
            drop(watcher);
            *file_tx.lock() = None;
        });
        ctx.health().set(Self::MANIFEST.id, Health::Ready);
        Ok(())
    }
}

pub struct DesktopActionPlugin {
    pub ingestion: Arc<Ingestion>,
    pub store: Arc<Store>,
    pub paths: AppPaths,
    pub guard: Arc<SelfWriteGuard>,
    pub cache_pin: Arc<RwLock<Option<String>>>,
}

impl DesktopActionPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.actions",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.history", "asterism.sync"],
        permissions: &["content.read", "content.favorite", "content.delete", "clipboard.write"],
    };
}

impl Plugin for DesktopActionPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let _ = ctx.require::<HistoryApi>()?;
        let sync = ctx.require::<SyncHandle>()?;
        let registry = ctx.require::<ActionRegistry>()?;
        let broker = PermissionBroker::for_plugin(ctx.permissions());
        let ingestion = Arc::clone(&self.ingestion);
        let store = Arc::clone(&self.store);
        let paths = self.paths.clone();
        let guard = Arc::clone(&self.guard);
        let cache_pin = Arc::clone(&self.cache_pin);

        {
            let ingestion = Arc::clone(&ingestion);
            let store = Arc::clone(&store);
            let paths = paths.clone();
            let guard = Arc::clone(&guard);
            let cache_pin = Arc::clone(&cache_pin);
            let broker = broker.clone();
            registry.register(ActionKey::COPY, move |item_id, _| {
                copy_action(&ingestion, &store, &paths, &guard, &cache_pin, &broker, item_id)
            });
        }
        {
            let ingestion = Arc::clone(&ingestion);
            let broker = broker.clone();
            registry.register(ActionKey::FAVORITE, move |item_id, _| {
                favorite_action(&ingestion, &broker, item_id)
            });
        }
        {
            let ingestion = Arc::clone(&ingestion);
            let broker = broker.clone();
            let sync = (*sync).clone();
            registry.register(ActionKey::DELETE, move |item_id, _| {
                delete_action(&ingestion, &broker, &sync, item_id)
            });
        }
        {
            let store = Arc::clone(&store);
            let paths = paths.clone();
            let ingestion = Arc::clone(&ingestion);
            registry.register(ActionKey::SAVE, move |item_id, save_path| {
                save_action(&ingestion, &store, &paths, item_id, save_path)
            });
        }
        registry.register(ActionKey::SEND, |_, _| {
            Err(ActionError::Failed("send uses sync session".into()))
        });
        ctx.health().set(Self::MANIFEST.id, Health::Ready);
        Ok(())
    }
}

fn map_err(err: impl std::fmt::Display) -> ActionError {
    ActionError::Failed(err.to_string())
}

fn copy_action(
    ingestion: &Ingestion,
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    cache_pin: &RwLock<Option<String>>,
    broker: &PermissionBroker,
    item_id: ContentId,
) -> std::result::Result<ActionResult, ActionError> {
    let item = asterism_domain_runtime::ContentCommandService::new(ingestion)
        .get(item_id)
        .map_err(map_err)?;
    if !builtin_actions::supports(ActionId::Copy, &item) {
        return Err(ActionError::Unsupported);
    }
    let grant = broker.grant_copy(item.id(), item.kind()).ok_or(ActionError::Unsupported)?;
    let content = item_to_clipboard(&item, store, paths, &grant).map_err(map_err)?;
    guard.remember(item.id(), content.dedup_tag());
    ClipboardBackend::write(&NativeClipboard, &content).map_err(map_err)?;
    *cache_pin.write() = item.metadata().local_cache_rel.clone().or_else(|| {
        matches!(item.kind(), asterism_core::ContentKind::Gif | asterism_core::ContentKind::Video)
            .then(|| item.id().to_string())
    });
    Ok(builtin_actions::copied(&item))
}

fn favorite_action(
    ingestion: &Ingestion,
    broker: &PermissionBroker,
    item_id: ContentId,
) -> std::result::Result<ActionResult, ActionError> {
    let item = asterism_domain_runtime::ContentCommandService::new(ingestion)
        .get(item_id)
        .map_err(map_err)?;
    let next = !item.flags().contains(asterism_core::ContentFlags::FAVORITE);
    let command = broker.grant_command(item.id(), true, false).ok_or(ActionError::Unsupported)?;
    asterism_domain_runtime::ContentCommandService::new(ingestion)
        .set_favorite(&command, next)
        .map_err(map_err)?;
    Ok(builtin_actions::favorited(&item, next))
}

fn delete_action(
    ingestion: &Ingestion,
    broker: &PermissionBroker,
    sync: &SyncHandle,
    item_id: ContentId,
) -> std::result::Result<ActionResult, ActionError> {
    let item = asterism_domain_runtime::ContentCommandService::new(ingestion)
        .get(item_id)
        .map_err(map_err)?;
    let command = broker.grant_command(item.id(), false, true).ok_or(ActionError::Unsupported)?;
    asterism_domain_runtime::ContentCommandService::new(ingestion)
        .delete(&command)
        .map_err(map_err)?;
    sync.notify_deleted(item.id());
    Ok(builtin_actions::deleted(&item))
}

fn save_action(
    ingestion: &Ingestion,
    store: &Store,
    paths: &AppPaths,
    item_id: ContentId,
    save_path: Option<PathBuf>,
) -> std::result::Result<ActionResult, ActionError> {
    let item = asterism_domain_runtime::ContentCommandService::new(ingestion)
        .get(item_id)
        .map_err(map_err)?;
    if !builtin_actions::supports(ActionId::Save, &item) {
        return Err(ActionError::Unsupported);
    }
    let path = builtin_actions::require_save_path(save_path.as_ref())?;
    match item.payload_ref() {
        asterism_core::content::PayloadRef::Inline { bytes } => {
            std::fs::write(&path, bytes).map_err(|err| ActionError::Failed(err.to_string()))?;
        }
        asterism_core::content::PayloadRef::Blob { blob_id } => {
            std::fs::write(&path, store.get_blob(blob_id).map_err(map_err)?)
                .map_err(|err| ActionError::Failed(err.to_string()))?;
        }
        asterism_core::content::PayloadRef::FileManifest { .. } => {
            let cache = paths.item_cache(item.id());
            if cache.exists() {
                copy_dir(&cache, &path).map_err(|err| ActionError::Failed(err.to_string()))?;
            }
        }
    }
    Ok(ActionResult::Saved { path })
}

fn copy_dir(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn submit_capture(
    ingestion: &Ingestion,
    guard: &SelfWriteGuard,
    sync: &SyncHandle,
    app: &AppHandle,
    cache_pin: &RwLock<Option<String>>,
    captured: CapturedClipboard,
) {
    let pin_cache = !captured.files.is_empty();
    let draft = ContentDraft {
        producer_plugin_id: "asterism.clipboard".into(),
        source_device_id: ingestion.device_id(),
        source_event_id: captured.change_token.to_string(),
        change_token: Some(captured.change_token),
        kind_hint: if pin_cache {
            asterism_core::ContentKind::Files
        } else if captured.image.is_some() {
            asterism_core::ContentKind::Image
        } else {
            asterism_core::ContentKind::Text
        },
        source_app: captured.source_app.clone(),
        parent_content_id: None,
        kind_override: None,
        mime_hint: None,
    };
    match ingestion.submit_capture(draft, captured, guard) {
        Ok(IngestionOutcome::Committed(id)) => {
            if pin_cache {
                *cache_pin.write() = Some(id.to_string());
            }
            sync.drain_outbox();
            let _ = app.emit("history-changed", ());
        }
        Ok(IngestionOutcome::Ignored(_) | IngestionOutcome::RejectedPolicy) => {}
        Err(err) => tracing::warn!(error = %err, "failed to ingest clipboard item"),
    }
}
