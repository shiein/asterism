use std::sync::Arc;
use std::sync::mpsc;

use asterism_clipboard::files::materialize_to_cache;
use asterism_clipboard::normalize::NormalizedContent;
use asterism_clipboard::{
    ClipboardEvent, NativeClipboard, SelfWriteGuard, WatcherConfig, WatcherHandle, spawn_watcher,
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
    pub avk: Arc<parking_lot::RwLock<asterism_crypto::AccountVaultKey>>,
    pub sync: SyncHandle,
    #[allow(dead_code)]
    pub watcher: WatcherHandle,
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

        let avk = Arc::new(parking_lot::RwLock::new(asterism_crypto::AccountVaultKey::from_bytes(
            *vault.avk.as_bytes(),
        )));
        let avk_watch = Arc::clone(&avk);
        let (file_tx, file_rx) = mpsc::channel::<NormalizedContent>();
        {
            let store = Arc::clone(&store_task);
            let paths = paths_task.clone();
            let sync = sync_watch.clone();
            let app = app_task.clone();
            let avk = Arc::clone(&avk_watch);
            std::thread::Builder::new()
                .name("asterism-files".into())
                .spawn(move || {
                    while let Ok(content) = file_rx.recv() {
                        persist_captured(&store, &paths, device_id, &avk, &sync, &app, content);
                    }
                })
                .ok();
        }
        let watcher = spawn_watcher(
            WatcherConfig {
                device_id,
                policy: asterism_core::CapturePolicy::default(),
                remote: asterism_core::RemotePolicy::default(),
                ..WatcherConfig::default()
            },
            Arc::clone(&guard_task),
            move |event| match event {
                ClipboardEvent::Captured(content) => {
                    if matches!(content, NormalizedContent::Files { .. }) {
                        if file_tx.send(content).is_err() {
                            tracing::warn!("file persist worker stopped");
                        }
                    } else {
                        persist_captured(
                            &store_task,
                            &paths_task,
                            device_id,
                            &avk_watch,
                            &sync_watch,
                            &app_task,
                            content,
                        );
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
            avk,
            sync,
            watcher,
            _crash: crash,
        })
    }
}

fn persist_captured(
    store: &Store,
    paths: &AppPaths,
    device_id: DeviceId,
    avk: &parking_lot::RwLock<asterism_crypto::AccountVaultKey>,
    sync: &crate::sync_engine::SyncHandle,
    app: &AppHandle,
    content: NormalizedContent,
) {
    let current = asterism_crypto::AccountVaultKey::from_bytes(*avk.read().as_bytes());
    match persist(store, paths, device_id, &current, content) {
        Ok(Some(item)) => {
            sync.notify_local(item);
            let _ = app.emit("history-changed", ());
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(error = %err, "failed to persist clipboard item"),
    }
}

fn persist(
    store: &Store,
    paths: &AppPaths,
    device_id: DeviceId,
    avk: &asterism_crypto::AccountVaultKey,
    content: NormalizedContent,
) -> anyhow::Result<Option<ContentItem>> {
    let now = asterism_platform::now_ms();
    let mut item = match content {
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
        NormalizedContent::Files { paths: sources, manifest: _, dedup_tag: _, flags, source_app } => {
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
            let mut item = NormalizedContent::Files {
                paths: sources.clone(),
                manifest: manifest.clone(),
                dedup_tag: asterism_crypto::local_dedup_tag(&fingerprint),
                flags,
                source_app,
            }
            .into_item(device_id, now);
            let cache = paths.item_cache(item.id);
            materialize_to_cache(&cache, &sources)?;
            item.metadata.local_cache_rel = Some(item.id.to_string());
            apply_hmac_dedup(&mut item, avk);
            persist_item(store, item.clone(), Some(manifest))?;
            return Ok(Some(item));
        }
    };
    apply_hmac_dedup(&mut item, avk);
    persist_item(store, item.clone(), None)?;
    Ok(Some(item))
}

fn apply_hmac_dedup(item: &mut ContentItem, avk: &asterism_crypto::AccountVaultKey) {
    item.dedup_tag = avk.dedup_tag(&item.dedup_tag);
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
                text: text.clone(),
                dedup_tag: asterism_crypto::local_dedup_tag(text.as_bytes()),
                flags: item.flags,
                source_app: item.metadata.source_app.clone(),
            })
        }
        ContentKind::Image | ContentKind::Screenshot => {
            let png = load_bytes(item, store)?;
            let (width, height) =
                item.metadata.image.as_ref().map(|i| (i.width, i.height)).unwrap_or((0, 0));
            Ok(NormalizedContent::Image {
                png: png.clone(),
                width,
                height,
                dedup_tag: asterism_crypto::local_dedup_tag(&png),
                flags: item.flags,
                source_app: item.metadata.source_app.clone(),
            })
        }
        ContentKind::Gif | ContentKind::Video => {
            let bytes = load_bytes(item, store)?;
            let cache = paths.item_cache(item.id);
            std::fs::create_dir_all(&cache)?;
            let name = if item.kind == ContentKind::Gif { "clip.gif" } else { "clip.mp4" };
            let path = cache.join(name);
            std::fs::write(&path, &bytes)?;
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
                dedup_tag: asterism_crypto::local_dedup_tag(&bytes),
                flags: item.flags | ContentFlags::FROM_REMOTE,
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

        let avk = asterism_crypto::AccountVaultKey::generate();
        let first = persist(&store, &paths, device, &avk, content.clone()).unwrap().unwrap();
        let second = persist(&store, &paths, device, &avk, content).unwrap().unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(store.history(asterism_storage::HistoryQuery::recent(10)).unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
