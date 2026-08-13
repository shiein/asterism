#![allow(clippy::too_many_arguments, clippy::collapsible_if)]

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use asterism_clipboard::{ClipboardBackend, NativeClipboard, SelfWriteGuard};
use asterism_core::content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, FileManifest, ItemMetadata, PayloadRef,
};
use asterism_crypto::AccountVaultKey;
use asterism_platform::{AppPaths, LocalIdentity};
use asterism_storage::Store;
use asterism_sync::hub_client::{HistoryDto, HubClient};
use asterism_sync::lan::{self, DiscoveredPeer};
use asterism_sync::pairing::PairingFinish;
use asterism_sync::protocol::{Envelope, ItemOffer, ItemReady, LanItem, MessageBody};
use asterism_sync::{
    DeviceCert, decode_package, decrypt_blob_chunks, encode_package, encrypt_blob_chunks, pack,
    pack_file_bundle, unpack_body, unpack_file_bundle, unpack_meta, unpack_tree,
};
use mdns_sd::ServiceEvent;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::runtime::{item_to_clipboard, persist_item};
use crate::settings::SyncSettings;

pub enum SyncCmd {
    LocalItem(Box<ContentItem>),
    ReplaceVault(AccountVaultKey),
    Reload,
}

#[derive(Clone)]
pub struct SyncHandle {
    tx: mpsc::UnboundedSender<SyncCmd>,
    pub settings: Arc<Mutex<SyncSettings>>,
}

impl SyncHandle {
    pub fn notify_local(&self, item: ContentItem) {
        if item.may_sync_remote() {
            let _ = self.tx.send(SyncCmd::LocalItem(Box::new(item)));
        }
    }

    pub fn reload(&self) {
        let _ = self.tx.send(SyncCmd::Reload);
    }

    pub fn replace_vault(&self, vault: AccountVaultKey) {
        let _ = self.tx.send(SyncCmd::ReplaceVault(vault));
    }
}

pub fn spawn(
    identity: LocalIdentity,
    vault: AccountVaultKey,
    store: Arc<Store>,
    paths: AppPaths,
    guard: Arc<SelfWriteGuard>,
    settings: SyncSettings,
    on_change: impl Fn() + Send + Sync + 'static,
) -> SyncHandle {
    let settings = Arc::new(Mutex::new(settings));
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = SyncHandle { tx, settings: Arc::clone(&settings) };
    thread::Builder::new()
        .name("asterism-sync".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(error = %err, "sync runtime");
                    return;
                }
            };
            rt.block_on(run_loop(identity, vault, store, paths, guard, settings, rx, on_change));
        })
        .ok();
    handle
}

async fn run_loop(
    identity: LocalIdentity,
    mut vault: AccountVaultKey,
    store: Arc<Store>,
    paths: AppPaths,
    guard: Arc<SelfWriteGuard>,
    settings: Arc<Mutex<SyncSettings>>,
    mut rx: mpsc::UnboundedReceiver<SyncCmd>,
    on_change: impl Fn() + Send + Sync + 'static,
) {
    let port = settings.lock().lan_port;
    let cert = match DeviceCert::load_or_create(&paths.config_dir, &identity.device_name) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "device cert");
            return;
        }
    };
    let lan = lan::LanEndpoint::announce(identity.device_id, cert.clone(), port).ok();
    let peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>> = Arc::new(Mutex::new(HashMap::new()));
    if let Some(lan) = &lan {
        if let Ok(rx_mdns) = lan.browse() {
            let peers_b = Arc::clone(&peers);
            let self_id = identity.device_id;
            thread::spawn(move || {
                while let Ok(ev) = rx_mdns.recv() {
                    if let ServiceEvent::ServiceResolved(info) = ev {
                        if let Some(peer) = lan::parse_mdns_device(&info) {
                            if peer.device_id != self_id {
                                peers_b.lock().insert(peer.device_id.to_string(), peer);
                            }
                        }
                    }
                }
            });
        }
    }

    let listener = lan::listen(cert.clone(), port).await.ok();
    let mut last_cursor: Option<String> = None;
    let mut maintenance = tokio::time::interval(Duration::from_secs(60 * 60));

    loop {
        tokio::select! {
            accepted = accept_opt(listener.as_ref(), &cert) => {
                if let Ok((mut stream, _)) = accepted {
                    match asterism_sync::lan_item::recv_lan_item(&mut stream, identity.device_id).await {
                        Ok((from, lan_item, payload)) => {
                            if let Err(err) = apply_lan(&store, &paths, &guard, from, lan_item, payload) {
                                tracing::warn!(error = %err, "lan apply");
                            } else {
                                on_change();
                            }
                        }
                        Err(err) => tracing::debug!(error = %err, "lan recv"),
                    }
                }
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    SyncCmd::LocalItem(item) => {
                        let _ = push_lan(&cert, &peers, identity.device_id, item.as_ref(), &store, &paths).await;
                        if let Err(err) = publish_one(&identity, &vault, &store, &paths, &settings, item.as_ref(), &on_change).await {
                            tracing::warn!(error = %err, "hub publish failed");
                        }
                    }
                    SyncCmd::ReplaceVault(next) => {
                        vault = next;
                        last_cursor = None;
                    }
                    SyncCmd::Reload => {
                        exchange_candidates(&identity, &settings, &peers).await;
                        retry_pending(&identity, &vault, &store, &paths, &settings, &on_change).await;
                        if let Err(err) = pull_hub(&vault, &store, &paths, &guard, &settings, &mut last_cursor, &on_change).await {
                            tracing::warn!(error = %err, "hub pull failed");
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(8)) => {
                exchange_candidates(&identity, &settings, &peers).await;
                retry_pending(&identity, &vault, &store, &paths, &settings, &on_change).await;
                if let Err(err) = pull_hub(&vault, &store, &paths, &guard, &settings, &mut last_cursor, &on_change).await {
                    tracing::debug!(error = %err, "hub pull");
                }
            }
            _ = maintenance.tick() => {
                if let Err(err) = store.gc_blobs(Duration::from_secs(24 * 60 * 60)) {
                    tracing::warn!(error = %err, "local blob GC failed");
                }
            }
        }
    }
}

async fn retry_pending(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    on_change: &impl Fn(),
) {
    if !settings.lock().hub_ready() {
        return;
    }
    let pending = match store.pending_sync(200) {
        Ok(items) => items,
        Err(err) => {
            tracing::warn!(error = %err, "load sync outbox failed");
            return;
        }
    };
    for item in pending {
        if let Err(err) =
            publish_one(identity, vault, store, paths, settings, &item, on_change).await
        {
            tracing::warn!(item_id = %item.id, error = %err, "hub retry failed");
        }
    }
}

async fn publish_one(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    item: &ContentItem,
    on_change: &impl Fn(),
) -> anyhow::Result<()> {
    if !settings.lock().hub_ready() || !item.may_sync_remote() {
        return Ok(());
    }
    store.set_status(item.id, ContentStatus::Uploading)?;
    let result = publish(identity, vault, store, paths, settings, item).await;
    let status = if result.is_ok() { ContentStatus::SyncedToHub } else { ContentStatus::Failed };
    store.set_status(item.id, status)?;
    on_change();
    result
}

async fn accept_opt(
    listener: Option<&tokio::net::TcpListener>,
    cert: &DeviceCert,
) -> anyhow::Result<(tokio_rustls::server::TlsStream<tokio::net::TcpStream>, std::net::SocketAddr)>
{
    match listener {
        Some(listener) => lan::accept_direct(listener, cert).await.map_err(|e| anyhow::anyhow!(e)),
        None => std::future::pending().await,
    }
}

async fn push_lan(
    cert: &DeviceCert,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    local: asterism_core::DeviceId,
    item: &ContentItem,
    store: &Store,
    paths: &AppPaths,
) -> anyhow::Result<()> {
    let snapshot: Vec<DiscoveredPeer> = peers.lock().values().cloned().collect();
    if snapshot.is_empty() {
        return Ok(());
    }
    let payload = load_payload(item, store, paths)?;
    let Some(payload) = payload else { return Ok(()) };
    let meta = serde_json::to_string(&item.metadata)?;
    let offer = ItemOffer {
        item_id: item.id,
        kind: item.kind.as_str().to_string(),
        logical_size: item.logical_size,
        payload_size: payload.len() as u64,
        dedup_tag: item.dedup_tag.to_vec(),
        flags: item.flags.bits(),
    };
    let env = Envelope::new(
        local,
        MessageBody::LanItem(LanItem {
            offer,
            metadata_json: meta,
            payload_len: payload.len() as u64,
        }),
    );
    for peer in snapshot {
        let Some(fp) = peer.fingerprint else { continue };
        for ep in &peer.addresses {
            match lan::connect_direct(ep, fp, cert, lan::lan_timeout()).await {
                Ok(mut stream) => {
                    if asterism_sync::lan_item::send_lan_item(&mut stream, &env, &payload)
                        .await
                        .is_ok()
                    {
                        break;
                    }
                }
                Err(_) => continue,
            }
        }
    }
    Ok(())
}

async fn exchange_candidates(
    identity: &LocalIdentity,
    settings: &Arc<Mutex<SyncSettings>>,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
) {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return;
    }
    let Ok(client) = client(&snap).await else { return };
    let Ok(mut ws) = client.connect_ws().await else { return };
    let cands = asterism_platform::local_candidates(snap.lan_port)
        .into_iter()
        .map(|c| c.endpoint())
        .collect();
    let env = Envelope::new(
        identity.device_id,
        MessageBody::LanCandidates(asterism_sync::protocol::LanCandidates { endpoints: cands }),
    );
    let _ = HubClient::send_control(&mut ws, &env).await;
    if let Ok(Some(incoming)) = HubClient::recv_control(&mut ws).await {
        if let MessageBody::LanCandidates(c) = incoming.body {
            let mut map = peers.lock();
            map.entry(incoming.device_id.to_string()).or_insert(DiscoveredPeer {
                device_id: incoming.device_id,
                port: snap.lan_port,
                fingerprint: None,
                addresses: c.endpoints,
            });
        }
    }
}

fn apply_lan(
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    from: asterism_core::DeviceId,
    lan_item: LanItem,
    payload: Vec<u8>,
) -> anyhow::Result<()> {
    let mut tag = [0u8; 32];
    if lan_item.offer.dedup_tag.len() == 32 {
        tag.copy_from_slice(&lan_item.offer.dedup_tag);
    }
    if store.contains(lan_item.offer.item_id)? {
        return Ok(());
    }
    let metadata: ItemMetadata = serde_json::from_str(&lan_item.metadata_json).unwrap_or_default();
    let kind = ContentKind::parse(&lan_item.offer.kind).unwrap_or(ContentKind::Text);
    let (item, manifest) = build_item(
        lan_item.offer.item_id,
        from,
        kind,
        lan_item.offer.flags,
        tag,
        metadata,
        payload,
        store,
        paths,
        true,
    )?;
    persist_item(store, item.clone(), manifest)?;
    if let Ok(content) = item_to_clipboard(&item, store, paths) {
        guard.remember(item.id, content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(())
}

fn build_item(
    id: asterism_core::ContentId,
    origin: asterism_core::DeviceId,
    kind: ContentKind,
    flags: u32,
    tag: [u8; 32],
    mut metadata: ItemMetadata,
    bytes: Vec<u8>,
    store: &Store,
    paths: &AppPaths,
    from_lan: bool,
) -> anyhow::Result<(ContentItem, Option<FileManifest>)> {
    let flags = ContentFlags::from_bits_truncate(flags) | ContentFlags::FROM_REMOTE;
    let now = asterism_platform::now_ms();
    match kind {
        ContentKind::Text => Ok((
            ContentItem {
                id,
                origin_device_id: origin,
                kind,
                created_at_ms: now,
                logical_size: bytes.len() as u64,
                payload_size: bytes.len() as u64,
                dedup_tag: tag,
                flags,
                status: if from_lan {
                    asterism_core::ContentStatus::DeliveredToPeer
                } else {
                    asterism_core::ContentStatus::SyncedToHub
                },
                metadata,
                payload_ref: PayloadRef::Inline { bytes: bytes::Bytes::from(bytes) },
                encrypted_metadata: bytes::Bytes::new(),
            },
            None,
        )),
        ContentKind::Image | ContentKind::Screenshot | ContentKind::Gif | ContentKind::Video => {
            let blob = store.put_blob(&bytes)?;
            Ok((
                ContentItem {
                    id,
                    origin_device_id: origin,
                    kind,
                    created_at_ms: now,
                    logical_size: bytes.len() as u64,
                    payload_size: bytes.len() as u64,
                    dedup_tag: tag,
                    flags,
                    status: asterism_core::ContentStatus::SyncedToHub,
                    metadata,
                    payload_ref: PayloadRef::Blob { blob_id: blob },
                    encrypted_metadata: bytes::Bytes::new(),
                },
                None,
            ))
        }
        ContentKind::Files => {
            let cache = paths.item_cache(id);
            let manifest = match unpack_file_bundle(&bytes, &cache) {
                Ok((manifest, _)) => manifest,
                Err(_) => {
                    let roots = unpack_tree(&bytes, &cache)?;
                    asterism_clipboard::preflight_paths(&roots)?
                }
            };
            metadata.local_cache_rel = Some(id.to_string());
            metadata.files = Some(manifest.summary());
            let manifest_id = manifest.id;
            Ok((
                ContentItem {
                    id,
                    origin_device_id: origin,
                    kind,
                    created_at_ms: now,
                    logical_size: bytes.len() as u64,
                    payload_size: bytes.len() as u64,
                    dedup_tag: tag,
                    flags,
                    status: asterism_core::ContentStatus::SyncedToHub,
                    metadata,
                    payload_ref: PayloadRef::FileManifest { manifest_id },
                    encrypted_metadata: bytes::Bytes::new(),
                },
                Some(manifest),
            ))
        }
        other => anyhow::bail!("unsupported kind {other}"),
    }
}

async fn client(settings: &SyncSettings) -> anyhow::Result<HubClient> {
    let url = settings.hub_url.clone().ok_or_else(|| anyhow::anyhow!("no hub"))?;
    let token = settings.token.clone().ok_or_else(|| anyhow::anyhow!("no token"))?;
    Ok(HubClient::new(url)?.with_token(token))
}

async fn publish(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    item: &ContentItem,
) -> anyhow::Result<()> {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return Ok(());
    }
    let payload = load_payload(item, store, paths)?;
    let meta = serde_json::to_vec(&item.metadata)?;
    let mut pkg = pack(vault, &meta, payload.as_deref())?;
    let client = client(&snap).await?;
    let mut blob_id = None;
    if pkg.body.is_none()
        && let Some(bytes) = payload
    {
        let id = client.begin_blob().await?;
        let chunks = encrypt_blob_chunks(vault, &bytes)?;
        let count = u32::try_from(chunks.len())?;
        for (index, chunk) in chunks.into_iter().enumerate() {
            client.put_chunk(&id, u32::try_from(index)?, chunk).await?;
        }
        client.commit_blob(&id, count).await?;
        pkg.chunk_count = count;
        blob_id = Some(id);
    }
    let dto = HistoryDto {
        id: hex::encode(item.id.as_bytes()),
        origin_device_id: identity.device_id,
        kind: item.kind.as_str().to_string(),
        created_at_ms: item.created_at_ms,
        logical_size: item.logical_size,
        payload_size: item.payload_size,
        dedup_tag: hex::encode(item.dedup_tag),
        flags: item.flags.bits(),
        encrypted_metadata: encode_package(&pkg)?,
        blob_id,
    };
    client.publish_history(&dto).await?;
    if let Ok(mut ws) = client.connect_ws().await {
        let offer = ItemOffer {
            item_id: item.id,
            kind: item.kind.as_str().to_string(),
            logical_size: item.logical_size,
            payload_size: item.payload_size,
            dedup_tag: item.dedup_tag.to_vec(),
            flags: item.flags.bits(),
        };
        let _ = HubClient::send_control(
            &mut ws,
            &Envelope::new(identity.device_id, MessageBody::ItemOffer(offer)),
        )
        .await;
        if let Some(blob) = &dto.blob_id {
            let _ = HubClient::send_control(
                &mut ws,
                &Envelope::new(
                    identity.device_id,
                    MessageBody::ItemReady(ItemReady { item_id: item.id, blob_id: blob.clone() }),
                ),
            )
            .await;
        }
    }
    Ok(())
}

async fn pull_hub(
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    settings: &Arc<Mutex<SyncSettings>>,
    last_cursor: &mut Option<String>,
    on_change: &impl Fn(),
) -> anyhow::Result<()> {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return Ok(());
    }
    let client = client(&snap).await?;
    let items = client.history(last_cursor.as_deref(), 50).await?;
    for dto in items {
        let next_cursor = format!("{}:{}", dto.created_at_ms, dto.id);
        apply_remote(vault, store, paths, guard, &client, dto).await?;
        *last_cursor = Some(next_cursor);
        on_change();
    }
    Ok(())
}

async fn apply_remote(
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    client: &HubClient,
    dto: HistoryDto,
) -> anyhow::Result<()> {
    let pkg = decode_package(&dto.encrypted_metadata)?;
    let meta_bytes = unpack_meta(vault, &pkg)?;
    let metadata: ItemMetadata = serde_json::from_slice(&meta_bytes).unwrap_or_default();
    let mut payload = unpack_body(vault, &pkg)?;
    if payload.is_none()
        && let Some(blob) = &dto.blob_id
    {
        if pkg.chunk_count > 0 {
            let mut chunks = Vec::with_capacity(pkg.chunk_count as usize);
            for index in 0..pkg.chunk_count {
                chunks.push(client.get_chunk(blob, index).await?);
            }
            payload = Some(decrypt_blob_chunks(vault, &chunks)?);
        } else {
            // 兼容修复前已经上传的单 AEAD 包。
            let mut packed = Vec::new();
            for i in 0..512 {
                match client.get_chunk(blob, i).await {
                    Ok(part) => packed.extend(part),
                    Err(_) => break,
                }
            }
            if let Ok(enc) = serde_json::from_slice::<asterism_crypto::EncryptedPayload>(&packed) {
                payload = Some(asterism_crypto::decrypt_metadata(vault, &enc)?);
            }
        }
    }
    let Some(bytes) = payload else {
        return Ok(());
    };
    let mut tag = [0u8; 32];
    if let Ok(decoded) = hex::decode(&dto.dedup_tag)
        && decoded.len() == 32
    {
        tag.copy_from_slice(&decoded);
    }
    let remote_id: asterism_core::ContentId = dto.id.parse()?;
    if store.contains(remote_id)? {
        return Ok(());
    }
    let kind = ContentKind::parse(&dto.kind).unwrap_or(ContentKind::Text);
    let (item, manifest) = build_item(
        remote_id,
        dto.origin_device_id,
        kind,
        dto.flags,
        tag,
        metadata,
        bytes,
        store,
        paths,
        false,
    )?;
    persist_item(store, item.clone(), manifest)?;
    if let Ok(content) = item_to_clipboard(&item, store, paths) {
        guard.remember(item.id, content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(())
}

fn load_payload(
    item: &ContentItem,
    store: &Store,
    paths: &AppPaths,
) -> anyhow::Result<Option<Vec<u8>>> {
    match &item.payload_ref {
        PayloadRef::Inline { bytes } => Ok(Some(bytes.to_vec())),
        PayloadRef::Blob { blob_id } => Ok(Some(store.get_blob(blob_id)?)),
        PayloadRef::FileManifest { manifest_id } => {
            let cache = item
                .metadata
                .local_cache_rel
                .as_ref()
                .map(|rel| paths.cache_dir.join("items").join(rel))
                .unwrap_or_else(|| paths.item_cache(item.id));
            if cache.exists() {
                let manifest = store.load_manifest(*manifest_id)?;
                Ok(Some(pack_file_bundle(&manifest, &cache)?))
            } else {
                Ok(None)
            }
        }
    }
}

pub struct HubBootstrap {
    pub code: String,
    pub vault: Option<AccountVaultKey>,
}

pub async fn bootstrap_hub(
    settings: &mut SyncSettings,
    identity: &LocalIdentity,
    config_dir: &std::path::Path,
    hub_url: String,
    pairing_code: Option<String>,
) -> anyhow::Result<HubBootstrap> {
    settings.hub_url = Some(hub_url.trim_end_matches('/').to_string());
    let client = HubClient::new(settings.hub_url.clone().unwrap())?;
    let code = match pairing_code {
        Some(code) if !code.trim().is_empty() => code.trim().to_ascii_uppercase(),
        _ => client.pairing_start().await?.code,
    };
    let finish = PairingFinish {
        code: code.clone(),
        device_id: identity.device_id,
        device_name: identity.device_name.clone(),
        platform: asterism_core::DevicePlatform::current().as_str().to_string(),
        identity_public_key: vec![1],
    };
    let session = client.pairing_finish(&finish).await?;
    settings.token = Some(session.token.clone());
    settings.save(config_dir)?;
    let vault = session
        .avk_wrapped_hex
        .as_deref()
        .map(|wrapped| unwrap_pairing_avk(&code, wrapped))
        .transpose()?;
    Ok(HubBootstrap { code, vault })
}

pub fn unwrap_pairing_avk(code: &str, wrapped_hex: &str) -> anyhow::Result<AccountVaultKey> {
    let bytes = hex::decode(wrapped_hex)?;
    let payload: asterism_crypto::EncryptedPayload = serde_json::from_slice(&bytes)?;
    let wrap_key = AccountVaultKey::from_bytes(asterism_sync::pairing::hash_code(code));
    let plain = asterism_crypto::decrypt_metadata(&wrap_key, &payload)?;
    let raw: [u8; 32] =
        plain.try_into().map_err(|_| anyhow::anyhow!("paired AVK must be 32 bytes"))?;
    Ok(AccountVaultKey::from_bytes(raw))
}

pub async fn start_pairing_code(settings: &SyncSettings) -> anyhow::Result<String> {
    let url = settings.hub_url.clone().ok_or_else(|| anyhow::anyhow!("set hub url first"))?;
    let client = HubClient::new(url)?;
    Ok(client.pairing_start().await?.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_clipboard::NormalizedContent;

    #[test]
    fn pairing_wrap_recovers_avk() {
        let code = "ABCDEFGH";
        let expected = AccountVaultKey::generate();
        let wrap_key = AccountVaultKey::from_bytes(asterism_sync::pairing::hash_code(code));
        let wrapped = asterism_crypto::encrypt_metadata(&wrap_key, expected.as_bytes()).unwrap();
        let encoded = hex::encode(serde_json::to_vec(&wrapped).unwrap());
        let actual = unwrap_pairing_avk(code, &encoded).unwrap();
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn received_file_bundle_rebuilds_native_clipboard_item() {
        let root = std::env::temp_dir()
            .join(format!("asterism-received-files-{}", asterism_core::ContentId::new()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("note.txt"), b"hello").unwrap();
        let manifest = asterism_clipboard::preflight_paths(&[source.join("note.txt")]).unwrap();
        let bundle = pack_file_bundle(&manifest, &source).unwrap();
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();

        let (item, received_manifest) = build_item(
            asterism_core::ContentId::new(),
            asterism_core::DeviceId::new(),
            ContentKind::Files,
            ContentFlags::REMOTE_ALLOWED.bits(),
            [7; 32],
            ItemMetadata::default(),
            bundle,
            &store,
            &paths,
            false,
        )
        .unwrap();
        persist_item(&store, item.clone(), received_manifest).unwrap();
        let clipboard = item_to_clipboard(&item, &store, &paths).unwrap();

        let NormalizedContent::Files { paths: files, manifest: restored, .. } = clipboard else {
            panic!("expected files clipboard item");
        };
        assert_eq!(restored, manifest);
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(&files[0]).unwrap(), b"hello");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
