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
use asterism_platform::{AppPaths, LocalIdentity, TrustStore};
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

    let trust = Arc::new(Mutex::new(match TrustStore::load(&paths.config_dir) {
        Ok(store) => store,
        Err(err) => {
            tracing::error!(error = %err, "trust store");
            return;
        }
    }));
    let listener = lan::listen(cert.clone(), port).await.ok();
    let mut last_cursor = store.hub_cursor().ok().flatten();
    let mut failed_remote: Vec<asterism_sync::hub_client::HistoryDto> = Vec::new();
    let mut maintenance = tokio::time::interval(Duration::from_secs(60 * 60));
    let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();
    let net_watch = asterism_platform::spawn_change_watch(Duration::from_secs(3));
    std::thread::Builder::new()
        .name("asterism-net-bridge".into())
        .spawn(move || {
            while net_watch.recv().is_ok() {
                if net_tx.send(()).is_err() {
                    break;
                }
            }
        })
        .ok();

    loop {
        tokio::select! {
            accepted = accept_opt(listener.as_ref(), &cert, &trust) => {
                if let Ok((mut stream, _)) = accepted {
                    match asterism_sync::lan_item::recv_lan_item(&mut stream, identity.device_id, true).await {
                        Ok((from, lan_item, payload)) => {
                            if !trust.lock().contains_device(from) {
                                tracing::warn!(%from, "rejected lan item from untrusted device");
                            } else if let Err(err) = apply_lan(&store, &paths, &guard, from, lan_item, payload, settings.lock().auto_receive) {
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
                        if settings.lock().auto_sync {
                            let _ = push_lan(&cert, &peers, &trust, identity.device_id, item.as_ref(), &store, &paths).await;
                        }
                        if let Err(err) = publish_one(&identity, &vault, &store, &paths, &settings, item.as_ref(), &on_change).await {
                            tracing::warn!(error = %err, "hub publish failed");
                        }
                    }
                    SyncCmd::ReplaceVault(next) => {
                        vault = next;
                    }
                    SyncCmd::Reload => {
                        refresh_trust(&identity, &settings, &trust, &paths).await;
                        exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
                        retry_pending(&identity, &vault, &store, &paths, &settings, &on_change).await;
                        if let Err(err) = pull_hub(&identity, &vault, &store, &paths, &guard, &settings, &mut last_cursor, &mut failed_remote, &on_change).await {
                            tracing::warn!(error = %err, "hub pull failed");
                        }
                    }
                }
            }
            _ = net_rx.recv() => {
                tracing::info!("network change; refreshing candidates");
                exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
            }
            _ = tokio::time::sleep(Duration::from_secs(8)) => {
                refresh_trust(&identity, &settings, &trust, &paths).await;
                exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
                retry_pending(&identity, &vault, &store, &paths, &settings, &on_change).await;
                if let Err(err) = pull_hub(&identity, &vault, &store, &paths, &guard, &settings, &mut last_cursor, &mut failed_remote, &on_change).await {
                    tracing::debug!(error = %err, "hub pull");
                }
            }
            _ = maintenance.tick() => {
                if let Err(err) = store.gc_blobs(Duration::from_secs(24 * 60 * 60)) {
                    tracing::warn!(error = %err, "local blob GC failed");
                }
                if let Err(err) = store.sweep_orphan_blobs() {
                    tracing::warn!(error = %err, "local orphan blob sweep failed");
                }
                let pins = store.cache_pins().unwrap_or_default();
                if let Err(err) = asterism_storage::cleanup::evict_item_cache(
                    &paths.cache_dir,
                    Duration::from_secs(7 * 24 * 60 * 60),
                    2 * 1024 * 1024 * 1024,
                    &pins,
                ) {
                    tracing::warn!(error = %err, "item cache evict failed");
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
    trust: &Arc<Mutex<TrustStore>>,
) -> anyhow::Result<(tokio_rustls::server::TlsStream<tokio::net::TcpStream>, std::net::SocketAddr)>
{
    match listener {
        Some(listener) => {
            let fps = trust.lock().fingerprints();
            lan::accept_direct(listener, cert, &fps).await.map_err(|e| anyhow::anyhow!(e))
        }
        None => std::future::pending().await,
    }
}

async fn refresh_trust(
    identity: &LocalIdentity,
    settings: &Arc<Mutex<SyncSettings>>,
    trust: &Arc<Mutex<TrustStore>>,
    paths: &AppPaths,
) {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return;
    }
    let Ok(client) = client(&snap) else { return };
    let Ok(devices) = client.devices().await else { return };
    persist_hub_pin(settings, paths, &client);
    let mut store = trust.lock();
    for device in devices {
        if device.id == identity.device_id {
            continue;
        }
        if device.revoked {
            if let Err(err) = store.remove(device.id) {
                tracing::warn!(error = %err, "drop revoked peer");
            }
            continue;
        }
        if let Some(fp) = device.cert_fingerprint
            && let Err(err) = store.add(device.id, fp, device.name)
        {
            tracing::warn!(error = %err, "persist trusted peer");
        }
    }
}

async fn push_lan(
    cert: &DeviceCert,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    trust: &Arc<Mutex<TrustStore>>,
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
        if !trust.lock().is_trusted(peer.device_id, fp) {
            continue;
        }
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
    cert: &DeviceCert,
    settings: &Arc<Mutex<SyncSettings>>,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    paths: &AppPaths,
) {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return;
    }
    let Ok(client) = client(&snap) else { return };
    let Ok(mut ws) = client.connect_ws().await else { return };
    persist_hub_pin(settings, paths, &client);
    let cands = asterism_platform::local_candidates(snap.lan_port)
        .into_iter()
        .map(|c| c.endpoint())
        .collect();
    let env = Envelope::new(
        identity.device_id,
        MessageBody::LanCandidates(asterism_sync::protocol::LanCandidates {
            endpoints: cands,
            fingerprint_hex: Some(cert.fingerprint_hex()),
        }),
    );
    if HubClient::send_control(&mut ws, &env).await.is_err() {
        return;
    }
    let incoming = match tokio::time::timeout(
        Duration::from_secs(2),
        HubClient::recv_control(&mut ws),
    )
    .await
    {
        Ok(Ok(Some(incoming))) => incoming,
        _ => return,
    };
    if let MessageBody::LanCandidates(c) = incoming.body {
        let fingerprint = c.fingerprint_hex.as_deref().and_then(parse_fp);
        let mut map = peers.lock();
        map.insert(
            incoming.device_id.to_string(),
            DiscoveredPeer {
                device_id: incoming.device_id,
                port: snap.lan_port,
                fingerprint,
                addresses: c.endpoints,
            },
        );
    }
}

fn parse_fp(hex_str: &str) -> Option<[u8; 32]> {
    let raw = hex::decode(hex_str).ok()?;
    (raw.len() == 32).then(|| {
        let mut a = [0u8; 32];
        a.copy_from_slice(&raw);
        a
    })
}

fn apply_lan(
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    from: asterism_core::DeviceId,
    lan_item: LanItem,
    payload: Vec<u8>,
    auto_receive: bool,
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
        None,
        Some(lan_item.offer.logical_size),
        Some(lan_item.offer.payload_size),
    )?;
    persist_item(store, item.clone(), manifest)?;
    if auto_receive && let Ok(content) = item_to_clipboard(&item, store, paths) {
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
    created_at_ms: Option<i64>,
    logical_size: Option<u64>,
    payload_size: Option<u64>,
) -> anyhow::Result<(ContentItem, Option<FileManifest>)> {
    let flags = ContentFlags::from_bits_truncate(flags) | ContentFlags::FROM_REMOTE;
    let created_at_ms = created_at_ms.unwrap_or_else(asterism_platform::now_ms);
    let logical_size = logical_size.filter(|n| *n > 0).unwrap_or(bytes.len() as u64);
    let payload_size = payload_size.filter(|n| *n > 0).unwrap_or(bytes.len() as u64);
    match kind {
        ContentKind::Text => Ok((
            ContentItem {
                id,
                origin_device_id: origin,
                kind,
                created_at_ms,
                logical_size,
                payload_size,
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
                    created_at_ms,
                    logical_size,
                    payload_size,
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
                    created_at_ms,
                    logical_size,
                    payload_size,
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

fn client(settings: &SyncSettings) -> anyhow::Result<HubClient> {
    hub_client_from_settings(settings)
}

pub fn hub_client_from_settings(settings: &SyncSettings) -> anyhow::Result<HubClient> {
    let url = settings.hub_url.clone().ok_or_else(|| anyhow::anyhow!("no hub"))?;
    let mut client = HubClient::with_pin(url, settings.hub_cert_sha256.as_deref())?;
    if let Some(token) = &settings.token {
        client = client.with_token(token.clone());
    }
    Ok(client)
}

fn persist_hub_pin(settings: &Arc<Mutex<SyncSettings>>, paths: &AppPaths, client: &HubClient) {
    let Some(fp) = client.observed_cert_sha256() else { return };
    let mut snap = settings.lock();
    if snap.hub_cert_sha256.is_some() {
        return;
    }
    snap.hub_cert_sha256 = Some(fp);
    if let Err(err) = snap.save(&paths.config_dir) {
        tracing::warn!(error = %err, "persist hub cert pin");
    }
}

pub fn persist_hub_pin_settings(
    settings: &mut SyncSettings,
    config_dir: &std::path::Path,
    client: &HubClient,
) {
    if settings.hub_cert_sha256.is_some() {
        return;
    }
    if let Some(fp) = client.observed_cert_sha256() {
        settings.hub_cert_sha256 = Some(fp);
        if let Err(err) = settings.save(config_dir) {
            tracing::warn!(error = %err, "persist hub cert pin");
        }
    }
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
    if matches!(item.payload_ref, PayloadRef::FileManifest { .. }) && payload.is_none() {
        anyhow::bail!("file cache missing for {}; not publishing metadata-only item", item.id);
    }
    let meta = serde_json::to_vec(&item.metadata)?;
    let mut pkg = pack(vault, &meta, payload.as_deref())?;
    let client = client(&snap)?;
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
    persist_hub_pin(settings, paths, &client);
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
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    settings: &Arc<Mutex<SyncSettings>>,
    last_cursor: &mut Option<String>,
    failed_remote: &mut Vec<asterism_sync::hub_client::HistoryDto>,
    on_change: &impl Fn(),
) -> anyhow::Result<()> {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return Ok(());
    }
    let client = client(&snap)?;
    let mut retry = std::mem::take(failed_remote);
    let mut newest: Option<ContentItem> = None;
    for dto in retry.drain(..) {
        match apply_remote(vault, store, paths, guard, &client, dto.clone(), false).await {
            Ok(Some(item)) => newest = Some(item),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(id = %dto.id, error = %err, "retry remote item failed");
                failed_remote.push(dto);
            }
        }
    }
    let items = client.history(last_cursor.as_deref(), 50).await?;
    persist_hub_pin(settings, paths, &client);
    for dto in items {
        let next_cursor = format!("{}:{}", dto.created_at_ms, dto.id);
        match apply_remote(vault, store, paths, guard, &client, dto.clone(), false).await {
            Ok(Some(item)) => {
                newest = Some(item);
                persist_cursor(store, last_cursor, next_cursor);
                on_change();
            }
            Ok(None) => {
                persist_cursor(store, last_cursor, next_cursor);
            }
            Err(err) => {
                tracing::warn!(id = %dto.id, error = %err, "remote item failed; will retry");
                failed_remote.push(dto);
                break;
            }
        }
    }
    if snap.auto_receive
        && let Some(item) = newest
        && item.origin_device_id != identity.device_id
        && let Ok(content) = item_to_clipboard(&item, store, paths)
    {
        guard.remember(item.id, content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(())
}

fn persist_cursor(store: &Store, last_cursor: &mut Option<String>, next: String) {
    *last_cursor = Some(next.clone());
    if let Err(err) = store.set_hub_cursor(&next) {
        tracing::warn!(error = %err, "persist hub cursor");
    }
}

async fn apply_remote(
    vault: &AccountVaultKey,
    store: &Store,
    paths: &AppPaths,
    _guard: &SelfWriteGuard,
    client: &HubClient,
    dto: HistoryDto,
    write_clipboard: bool,
) -> anyhow::Result<Option<ContentItem>> {
    let remote_id: asterism_core::ContentId = dto.id.parse()?;
    if store.contains(remote_id)? {
        return Ok(None);
    }
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
        anyhow::bail!("remote item {} missing payload", dto.id);
    };
    let mut tag = [0u8; 32];
    if let Ok(decoded) = hex::decode(&dto.dedup_tag)
        && decoded.len() == 32
    {
        tag.copy_from_slice(&decoded);
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
        Some(dto.created_at_ms),
        Some(dto.logical_size),
        Some(dto.payload_size),
    )?;
    persist_item(store, item.clone(), manifest)?;
    if write_clipboard && let Ok(content) = item_to_clipboard(&item, store, paths) {
        _guard.remember(item.id, content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(Some(item))
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
    let cert = DeviceCert::load_or_create(config_dir, &identity.device_name)?;
    let supplied = pairing_code.filter(|c| !c.trim().is_empty());
    let (code, salt_hex) = match supplied {
        Some(code) if looks_like_pairing_code(&code) => {
            (asterism_sync::pairing::normalize_code(&code), None)
        }
        Some(bootstrap) => {
            let client = HubClient::with_pin(
                settings.hub_url.clone().unwrap(),
                settings.hub_cert_sha256.as_deref(),
            )?
            .with_bootstrap(bootstrap.trim().to_string());
            let offer = client.pairing_start().await?;
            persist_hub_pin_settings(settings, config_dir, &client);
            (offer.code, Some(offer.kdf_salt_hex))
        }
        None => anyhow::bail!(
            "first device needs the hub bootstrap secret; later devices need a pairing code"
        ),
    };
    let finish = PairingFinish {
        code: code.clone(),
        device_id: identity.device_id,
        device_name: identity.device_name.clone(),
        platform: asterism_core::DevicePlatform::current().as_str().to_string(),
        identity_public_key: vec![1],
        cert_fingerprint: cert.fingerprint_hex(),
    };
    let client = HubClient::with_pin(
        settings.hub_url.clone().unwrap(),
        settings.hub_cert_sha256.as_deref(),
    )?;
    let session = client.pairing_finish(&finish).await?;
    persist_hub_pin_settings(settings, config_dir, &client);
    settings.token = Some(session.token.clone());
    persist_trusted_devices(config_dir, identity.device_id, &session.trusted_devices)?;
    settings.save(config_dir)?;
    let vault = match (
        session.avk_wrapped_hex.as_deref(),
        session.kdf_salt_hex.as_deref().or(salt_hex.as_deref()),
    ) {
        (Some(wrapped), Some(salt)) => Some(unwrap_pairing_avk(&code, salt, wrapped)?),
        (Some(_), None) => anyhow::bail!("paired AVK wrap is missing KDF salt"),
        _ => None,
    };
    Ok(HubBootstrap { code, vault })
}

fn looks_like_pairing_code(code: &str) -> bool {
    let normalized = asterism_sync::pairing::normalize_code(code);
    normalized.len() == asterism_sync::pairing::PAIRING_CODE_LEN
        && normalized.chars().all(|c| c.is_ascii_alphanumeric())
}

fn persist_trusted_devices(
    config_dir: &std::path::Path,
    self_id: asterism_core::DeviceId,
    devices: &[asterism_sync::hub_client::TrustedDeviceDto],
) -> anyhow::Result<()> {
    let mut trust = TrustStore::load(config_dir)?;
    for device in devices {
        if device.device_id == self_id {
            continue;
        }
        if let Some(fp) = &device.cert_fingerprint {
            trust.add(device.device_id, fp.clone(), device.name.clone())?;
        }
    }
    Ok(())
}

pub fn unwrap_pairing_avk(
    code: &str,
    salt_hex: &str,
    wrapped_hex: &str,
) -> anyhow::Result<AccountVaultKey> {
    let bytes = hex::decode(wrapped_hex)?;
    let payload: asterism_crypto::EncryptedPayload = serde_json::from_slice(&bytes)?;
    let salt = asterism_sync::pairing::parse_salt_hex(salt_hex)
        .ok_or_else(|| anyhow::anyhow!("invalid pairing KDF salt"))?;
    let wrap_key =
        AccountVaultKey::from_bytes(asterism_sync::pairing::derive_wrap_key(code, &salt));
    let plain = asterism_crypto::decrypt_metadata(&wrap_key, &payload)?;
    let raw: [u8; 32] =
        plain.try_into().map_err(|_| anyhow::anyhow!("paired AVK must be 32 bytes"))?;
    Ok(AccountVaultKey::from_bytes(raw))
}

pub async fn start_pairing_code(
    settings: &mut SyncSettings,
    config_dir: &std::path::Path,
) -> anyhow::Result<String> {
    let url = settings.hub_url.clone().ok_or_else(|| anyhow::anyhow!("set hub url first"))?;
    let token =
        settings.token.clone().ok_or_else(|| anyhow::anyhow!("connect this device first"))?;
    let client = HubClient::with_pin(url, settings.hub_cert_sha256.as_deref())?.with_token(token);
    let offer = client.pairing_start().await?;
    persist_hub_pin_settings(settings, config_dir, &client);
    settings.pending_pair_code = Some(offer.code.clone());
    settings.pending_pair_salt = Some(offer.kdf_salt_hex.clone());
    settings.save(config_dir)?;
    Ok(offer.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_clipboard::NormalizedContent;

    #[test]
    fn pairing_wrap_recovers_avk() {
        let code = "ABCDEFGHJKLMNPQRSTUV";
        let salt = [9u8; 16];
        let expected = AccountVaultKey::generate();
        let wrap_key =
            AccountVaultKey::from_bytes(asterism_sync::pairing::derive_wrap_key(code, &salt));
        let wrapped = asterism_crypto::encrypt_metadata(&wrap_key, expected.as_bytes()).unwrap();
        let encoded = hex::encode(serde_json::to_vec(&wrapped).unwrap());
        let actual = unwrap_pairing_avk(code, &hex::encode(salt), &encoded).unwrap();
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
            None,
            None,
            None,
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

    #[test]
    fn missing_file_cache_is_not_treated_as_empty_payload() {
        let root = std::env::temp_dir()
            .join(format!("asterism-missing-cache-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let item = ContentItem {
            id: asterism_core::ContentId::new(),
            origin_device_id: asterism_core::DeviceId::new(),
            kind: ContentKind::Files,
            created_at_ms: 1,
            logical_size: 4,
            payload_size: 4,
            dedup_tag: [1; 32],
            flags: ContentFlags::REMOTE_ALLOWED,
            status: asterism_core::ContentStatus::Local,
            metadata: ItemMetadata::default(),
            payload_ref: PayloadRef::FileManifest { manifest_id: asterism_core::ManifestId::new() },
            encrypted_metadata: bytes::Bytes::new(),
        };
        let loaded = load_payload(&item, &store, &paths).unwrap();
        assert!(loaded.is_none());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
