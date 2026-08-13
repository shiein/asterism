use std::sync::Arc;

use asterism_clipboard::files::materialize_to_cache;
use asterism_clipboard::normalize::NormalizedContent;
use asterism_clipboard::{
    ClipboardEvent, NativeClipboard, SelfWriteGuard, WatcherConfig, spawn_watcher,
};
use asterism_core::content::{ContentFlags, ContentItem, ContentKind, PayloadRef};
use asterism_core::id::DeviceId;
use asterism_platform::hardening::CrashGuard;
use asterism_platform::{AppPaths, LocalIdentity, LocalVault};
use asterism_storage::Store;
use tauri::{AppHandle, Emitter};

use crate::settings::SyncSettings;
use crate::sync_engine::{self, SyncHandle};

pub struct DesktopState {
    pub store: Arc<Store>,
    pub guard: Arc<SelfWriteGuard>,
    pub identity: LocalIdentity,
    pub paths: AppPaths,
    pub clipboard: NativeClipboard,
    pub vault: parking_lot::RwLock<LocalVault>,
    pub sync: SyncHandle,
    _crash: CrashGuard,
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

        let settings = SyncSettings::load(&paths.config_dir);
        let store_task = Arc::clone(&store);
        let guard_task = Arc::clone(&guard);
        let paths_task = paths.clone();
        let device_id = identity.device_id;
        let app_task = app.clone();
        let app_sync = app.clone();
        let sync = sync_engine::spawn(
            identity.clone(),
            asterism_crypto::AccountVaultKey::from_bytes(*vault.avk.as_bytes()),
            Arc::clone(&store),
            paths.clone(),
            Arc::clone(&guard),
            settings,
            move || {
                let _ = app_sync.emit("history-changed", ());
            },
        );
        let sync_watch = sync.clone();

        spawn_watcher(
            WatcherConfig {
                device_id,
                policy: asterism_core::CapturePolicy::default(),
                ..WatcherConfig::default()
            },
            Arc::clone(&guard_task),
            move |event| match event {
                ClipboardEvent::Captured(content) => {
                    match persist(&store_task, &paths_task, device_id, content) {
                        Ok(Some(item)) => {
                            sync_watch.notify_local(item);
                            let _ = app_task.emit("history-changed", ());
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(error = %err, "failed to persist clipboard item")
                        }
                    }
                }
                ClipboardEvent::Ignored => {}
                ClipboardEvent::Error(message) => {
                    tracing::warn!(%message, "clipboard watcher error");
                }
            },
        );

        Ok(Self {
            store,
            guard,
            identity,
            paths,
            clipboard: NativeClipboard,
            vault: parking_lot::RwLock::new(vault),
            sync,
            _crash: crash,
        })
    }
}

fn persist(
    store: &Store,
    paths: &AppPaths,
    device_id: DeviceId,
    content: NormalizedContent,
) -> anyhow::Result<Option<ContentItem>> {
    let now = asterism_platform::now_ms();
    let item = match content {
        NormalizedContent::Text { .. } => content.into_item(device_id, now),
        NormalizedContent::Image { png, width, height, dedup_tag, flags, source_app } => {
            let blob_id = store.put_blob(&png)?;
            let mut item = NormalizedContent::Image {
                png: Vec::new(),
                width,
                height,
                dedup_tag,
                flags,
                source_app,
            }
            .into_item(device_id, now);
            item.payload_ref = PayloadRef::Blob { blob_id };
            item.logical_size = png.len() as u64;
            item.payload_size = png.len() as u64;
            item
        }
        NormalizedContent::Files { paths: sources, manifest, dedup_tag, flags, source_app } => {
            let mut item = NormalizedContent::Files {
                paths: sources.clone(),
                manifest: manifest.clone(),
                dedup_tag,
                flags,
                source_app,
            }
            .into_item(device_id, now);
            let cache = paths.item_cache(item.id);
            materialize_to_cache(&cache, &sources)?;
            item.metadata.local_cache_rel = Some(item.id.to_string());
            persist_item(store, item.clone(), Some(manifest))?;
            return Ok(Some(item));
        }
    };
    persist_item(store, item.clone(), None)?;
    Ok(Some(item))
}

pub fn persist_item(
    store: &Store,
    item: ContentItem,
    manifest: Option<asterism_core::FileManifest>,
) -> anyhow::Result<()> {
    store.insert(item, manifest)?;
    Ok(())
}

pub fn item_to_clipboard(
    item: &ContentItem,
    store: &Store,
    paths: &AppPaths,
) -> anyhow::Result<NormalizedContent> {
    match item.kind {
        ContentKind::Text => {
            let text = match &item.payload_ref {
                PayloadRef::Inline { bytes } => String::from_utf8_lossy(bytes).into_owned(),
                PayloadRef::Blob { blob_id } => {
                    String::from_utf8_lossy(&store.get_blob(blob_id)?).into_owned()
                }
                PayloadRef::FileManifest { .. } => anyhow::bail!("text item has file payload"),
            };
            Ok(NormalizedContent::Text {
                text,
                dedup_tag: item.dedup_tag,
                flags: item.flags,
                source_app: item.metadata.source_app.clone(),
            })
        }
        ContentKind::Image | ContentKind::Screenshot => {
            let png = load_bytes(item, store)?;
            let (width, height) =
                item.metadata.image.as_ref().map(|i| (i.width, i.height)).unwrap_or((0, 0));
            Ok(NormalizedContent::Image {
                png,
                width,
                height,
                dedup_tag: item.dedup_tag,
                flags: item.flags,
                source_app: item.metadata.source_app.clone(),
            })
        }
        ContentKind::Files => {
            let cache = item
                .metadata
                .local_cache_rel
                .as_ref()
                .map(|rel| paths.cache_dir.join("items").join(rel))
                .unwrap_or_else(|| paths.item_cache(item.id));
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
                paths: file_paths,
                manifest: if let PayloadRef::FileManifest { manifest_id } = item.payload_ref {
                    store.load_manifest(manifest_id)?
                } else {
                    anyhow::bail!("files item missing manifest")
                },
                dedup_tag: item.dedup_tag,
                flags: item.flags | ContentFlags::FROM_REMOTE,
                source_app: item.metadata.source_app.clone(),
            })
        }
        other => anyhow::bail!("cannot copy kind {other}"),
    }
}

fn load_bytes(item: &ContentItem, store: &Store) -> anyhow::Result<Vec<u8>> {
    match &item.payload_ref {
        PayloadRef::Inline { bytes } => Ok(bytes.to_vec()),
        PayloadRef::Blob { blob_id } => Ok(store.get_blob(blob_id)?),
        PayloadRef::FileManifest { .. } => anyhow::bail!("expected blob payload"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let device = DeviceId::new();
        let content = NormalizedContent::Text {
            text: "copy again".into(),
            dedup_tag: asterism_crypto::local_dedup_tag(b"copy again"),
            flags: ContentFlags::REMOTE_ALLOWED,
            source_app: None,
        };

        let first = persist(&store, &paths, device, content.clone()).unwrap().unwrap();
        let second = persist(&store, &paths, device, content).unwrap().unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(store.history(asterism_storage::HistoryQuery::recent(10)).unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
