#![allow(clippy::too_many_arguments, clippy::collapsible_if)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use asterism_clipboard::{ClipboardBackend, NativeClipboard, SelfWriteGuard};
use asterism_core::content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, FileManifest, ItemMetadata, PayloadRef,
};
use asterism_crypto::AccountVaultKey;
use asterism_domain_runtime::{DomainStore, Ingestion, RemoteItemBody, RemoteItemSpec};
use asterism_platform::{AppPaths, LocalIdentity, TrustStore};
#[cfg(test)]
use asterism_storage::Store;
use asterism_sync::hub_client::{HistoryDto, HubClient};
use asterism_sync::lan::{self, DiscoveredPeer};
use asterism_sync::pairing::PairingFinish;
use asterism_sync::protocol::{Envelope, ItemOffer, ItemReady, LanItem, MessageBody};
use asterism_sync::{
    BlobChunkDecryptor, BlobChunkEncryptor, DeviceCert, decode_package, encode_package, pack,
    pack_file_bundle_to_writer, unpack_body, unpack_file_bundle, unpack_file_bundle_reader,
    unpack_meta, unpack_tree,
};
use mdns_sd::ServiceEvent;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::runtime::item_to_clipboard;
use crate::settings::SyncSettings;
use asterism_plugin_api::ContentReadGrant;

pub enum SyncCmd {
    ReplaceVault(AccountVaultKey),
    Reload,
    DeleteRemote(asterism_core::ContentId),
    DrainOutbox,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanPeerDto {
    pub device_id: String,
    pub name: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    pub is_trusted: bool,
}

#[derive(Clone)]
pub struct SyncHandle {
    tx: mpsc::UnboundedSender<SyncCmd>,
    pub settings: Arc<Mutex<SyncSettings>>,
    pub peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    pub trust: Arc<Mutex<TrustStore>>,
    pub local_fingerprint: String,
}

impl SyncHandle {
    pub fn reload(&self) {
        let _ = self.tx.send(SyncCmd::Reload);
    }

    pub fn replace_vault(&self, vault: AccountVaultKey) {
        let _ = self.tx.send(SyncCmd::ReplaceVault(vault));
    }

    pub fn notify_deleted(&self, id: asterism_core::ContentId) {
        let _ = self.tx.send(SyncCmd::DeleteRemote(id));
    }

    pub fn drain_outbox(&self) {
        let _ = self.tx.send(SyncCmd::DrainOutbox);
    }

    pub fn get_lan_peers(&self) -> Vec<LanPeerDto> {
        let peers = self.peers.lock().clone();
        let trust = self.trust.lock();
        peers
            .into_values()
            .map(|p| {
                let is_trusted =
                    p.fingerprint.map(|fp| trust.is_trusted(p.device_id, fp)).unwrap_or(false);
                let fp_hex = p.fingerprint.map(hex::encode).unwrap_or_default();
                let short_id = p.device_id.to_string();
                let short_name = format!("Peer-{}", &short_id[..8.min(short_id.len())]);
                LanPeerDto {
                    device_id: short_id,
                    name: short_name,
                    addresses: p.addresses,
                    port: p.port,
                    fingerprint: fp_hex,
                    is_trusted,
                }
            })
            .collect()
    }

    pub fn trust_peer(
        &self,
        device_id_str: &str,
        fingerprint_hex: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        let device_id: asterism_core::id::DeviceId =
            device_id_str.parse().map_err(|e: asterism_core::CoreError| anyhow::anyhow!(e))?;
        self.trust.lock().add(device_id, fingerprint_hex.to_string(), name.to_string())?;
        self.reload();
        Ok(())
    }

    pub fn untrust_peer(&self, device_id_str: &str) -> anyhow::Result<()> {
        let device_id: asterism_core::id::DeviceId =
            device_id_str.parse().map_err(|e: asterism_core::CoreError| anyhow::anyhow!(e))?;
        self.trust.lock().remove(device_id)?;
        self.reload();
        Ok(())
    }
}

pub fn spawn(
    identity: LocalIdentity,
    vault: AccountVaultKey,
    store: Arc<DomainStore>,
    ingestion: Arc<Ingestion>,
    paths: AppPaths,
    guard: Arc<SelfWriteGuard>,
    cache_pin: Arc<parking_lot::RwLock<Option<String>>>,
    settings: SyncSettings,
    on_change: impl Fn() + Send + Sync + 'static,
) -> anyhow::Result<(SyncHandle, thread::JoinHandle<()>)> {
    let settings = Arc::new(Mutex::new(settings));
    let (tx, rx) = mpsc::unbounded_channel();
    let cert = DeviceCert::load_or_create(&paths.config_dir, &identity.device_name)
        .map_err(|e| anyhow::anyhow!(e))?;
    let local_fingerprint = cert.fingerprint_hex();
    let peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>> = Arc::new(Mutex::new(HashMap::new()));
    let trust =
        Arc::new(Mutex::new(TrustStore::load(&paths.config_dir).map_err(|e| anyhow::anyhow!(e))?));
    let handle = SyncHandle {
        tx,
        settings: Arc::clone(&settings),
        peers: Arc::clone(&peers),
        trust: Arc::clone(&trust),
        local_fingerprint,
    };
    let peers_loop = Arc::clone(&peers);
    let trust_loop = Arc::clone(&trust);
    let join = thread::Builder::new().name("asterism-sync".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(err) => {
                tracing::error!(error = %err, "sync runtime");
                return;
            }
        };
        rt.block_on(run_loop(
            identity, vault, store, ingestion, paths, guard, cache_pin, settings, cert, peers_loop,
            trust_loop, rx, on_change,
        ));
    })?;
    Ok((handle, join))
}

async fn run_loop(
    identity: LocalIdentity,
    mut vault: AccountVaultKey,
    store: Arc<DomainStore>,
    ingestion: Arc<Ingestion>,
    paths: AppPaths,
    guard: Arc<SelfWriteGuard>,
    cache_pin: Arc<parking_lot::RwLock<Option<String>>>,
    settings: Arc<Mutex<SyncSettings>>,
    cert: DeviceCert,
    peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    trust: Arc<Mutex<TrustStore>>,
    mut rx: mpsc::UnboundedReceiver<SyncCmd>,
    on_change: impl Fn() + Send + Sync + 'static,
) {
    let port = settings.lock().lan_port;
    let lan = lan::LanEndpoint::announce(identity.device_id, cert.clone(), port).ok();
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
    let mut last_cursor = store.hub_cursor().ok().flatten();
    let mut failed_remote = load_failed_remote(&store);
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
                if let Ok((mut stream, _, peer_fp)) = accepted {
                    match asterism_sync::lan_item::recv_lan_item(&mut stream, identity.device_id, true).await {
                        Ok((from, lan_item, payload)) => {
                            let trusted = peer_fp
                                .is_some_and(|fp| trust.lock().is_trusted(from, fp));
                            if !trusted {
                                tracing::warn!(%from, "rejected lan item from untrusted device");
                            } else if let Err(err) = apply_lan(&ingestion, &store, &paths, &guard, &cache_pin, from, lan_item, payload, settings.lock().auto_receive) {
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
                    SyncCmd::ReplaceVault(next) => {
                        vault = next;
                    }
                    SyncCmd::DeleteRemote(_id) => {
                        if let Err(err) = flush_tombstones(&store, &settings).await {
                            tracing::warn!(error = %err, "hub delete failed");
                        }
                    }
                    SyncCmd::DrainOutbox => {
                        consume_committed_outbox(
                            &identity, &vault, &store, &paths, &settings, &cert, &peers, &trust,
                            &on_change,
                        )
                        .await;
                    }
                    SyncCmd::Reload => {
                        consume_committed_outbox(
                            &identity, &vault, &store, &paths, &settings, &cert, &peers, &trust,
                            &on_change,
                        )
                        .await;
                        if let Err(err) = flush_tombstones(&store, &settings).await {
                            tracing::warn!(error = %err, "hub delete retry");
                        }
                        refresh_trust(&identity, &settings, &trust, &paths).await;
                        exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
                        backfill_unsynced(&store);
                        consume_committed_outbox(
                            &identity, &vault, &store, &paths, &settings, &cert, &peers, &trust,
                            &on_change,
                        )
                        .await;
                        if let Err(err) = pull_hub(&identity, &vault, &ingestion, &store, &paths, &guard, &cache_pin, &settings, &mut last_cursor, &mut failed_remote, &on_change).await {
                            tracing::debug!(error = %err, "hub pull failed");
                        }
                    }
                }
            }
            _ = net_rx.recv() => {
                tracing::info!("network change; refreshing candidates");
                exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
            }
            _ = tokio::time::sleep(Duration::from_secs(8)) => {
                consume_committed_outbox(
                    &identity, &vault, &store, &paths, &settings, &cert, &peers, &trust, &on_change,
                )
                .await;
                refresh_trust(&identity, &settings, &trust, &paths).await;
                exchange_candidates(&identity, &cert, &settings, &peers, &paths).await;
                if let Err(err) = flush_tombstones(&store, &settings).await {
                    tracing::debug!(error = %err, "hub delete retry");
                }
                if let Err(err) = pull_hub(&identity, &vault, &ingestion, &store, &paths, &guard, &cache_pin, &settings, &mut last_cursor, &mut failed_remote, &on_change).await {
                    tracing::debug!(error = %err, "hub pull");
                }
            }
            _ = maintenance.tick() => {
                if let Err(err) = store.gc_blobs(Duration::from_secs(24 * 60 * 60)) {
                    tracing::warn!(error = %err, "local blob GC failed");
                }
                if let Err(err) = store.gc_outbox(Duration::from_secs(7 * 24 * 60 * 60)) {
                    tracing::warn!(error = %err, "local outbox GC failed");
                }
                if let Err(err) = store.sweep_orphan_blobs() {
                    tracing::warn!(error = %err, "local orphan blob sweep failed");
                }
                let mut pins = store.cache_pins().unwrap_or_default();
                if let Some(pin) = cache_pin.read().clone()
                    && !pins.contains(&pin)
                {
                    pins.push(pin);
                }
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

async fn consume_committed_outbox(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &DomainStore,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    cert: &DeviceCert,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    trust: &Arc<Mutex<TrustStore>>,
    on_change: &impl Fn(),
) {
    consume_lan_outbox(identity, store, paths, settings, cert, peers, trust).await;
    consume_hub_outbox(identity, vault, store, paths, settings, on_change).await;
}

async fn consume_lan_outbox(
    identity: &LocalIdentity,
    store: &DomainStore,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    cert: &DeviceCert,
    peers: &Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    trust: &Arc<Mutex<TrustStore>>,
) {
    let pending = match store.pending_outbox_for(
        asterism_storage::EVENT_COMMITTED,
        asterism_storage::CONSUMER_LAN,
        50,
    ) {
        Ok(events) => events,
        Err(err) => {
            tracing::warn!(error = %err, "load lan outbox");
            return;
        }
    };
    for event in pending {
        let from_remote = event.payload().map(|payload| payload.from_remote).unwrap_or(false);
        if from_remote {
            let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_LAN);
            continue;
        }
        let item = match store.get(event.aggregate_id) {
            Ok(item) => item,
            Err(asterism_storage::StorageError::NotFound) => {
                let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_LAN);
                continue;
            }
            Err(err) => {
                tracing::warn!(error = %err, "load lan item");
                let _ = store.retry_outbox_consumer(
                    event.id,
                    asterism_storage::CONSUMER_LAN,
                    Duration::from_secs(5),
                );
                continue;
            }
        };
        if item.may_sync_remote() && settings.lock().auto_sync {
            let _ = push_lan(cert, peers, trust, identity.device_id, &item, store, paths).await;
        }
        let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_LAN);
    }
}

async fn consume_hub_outbox(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &DomainStore,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    on_change: &impl Fn(),
) {
    let pending = match store.pending_outbox_for(
        asterism_storage::EVENT_COMMITTED,
        asterism_storage::CONSUMER_HUB,
        50,
    ) {
        Ok(events) => events,
        Err(err) => {
            tracing::warn!(error = %err, "load hub outbox");
            return;
        }
    };
    let snap = settings.lock().clone();
    for event in pending {
        let from_remote = event.payload().map(|payload| payload.from_remote).unwrap_or(false);
        if from_remote {
            let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_HUB);
            continue;
        }
        if !snap.hub_url.as_ref().is_some_and(|url| !url.is_empty()) {
            continue;
        }
        let item = match store.get(event.aggregate_id) {
            Ok(item) => item,
            Err(asterism_storage::StorageError::NotFound) => {
                let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_HUB);
                continue;
            }
            Err(err) => {
                tracing::warn!(error = %err, "load hub item");
                let _ = store.retry_outbox_consumer(
                    event.id,
                    asterism_storage::CONSUMER_HUB,
                    Duration::from_secs(5),
                );
                continue;
            }
        };
        if !item.may_sync_remote() {
            let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_HUB);
            continue;
        }
        if !snap.hub_ready() {
            let _ = store.retry_outbox_consumer(
                event.id,
                asterism_storage::CONSUMER_HUB,
                Duration::from_secs(8),
            );
            continue;
        }
        match publish_one(identity, vault, store, paths, settings, &item, on_change).await {
            Ok(()) => {
                let _ = store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_HUB);
            }
            Err(err) => {
                tracing::warn!(error = %err, "hub publish from outbox failed");
                let _ = store.retry_outbox_consumer(
                    event.id,
                    asterism_storage::CONSUMER_HUB,
                    Duration::from_secs(8),
                );
            }
        }
    }
}

fn backfill_unsynced(store: &DomainStore) {
    let pending = match store.pending_sync(200) {
        Ok(items) => items,
        Err(err) => {
            tracing::warn!(error = %err, "load pending sync for outbox backfill");
            return;
        }
    };
    for item in pending {
        if let Err(err) = store.ensure_committed(&item) {
            tracing::warn!(error = %err, "backfill committed outbox");
        }
    }
}

async fn publish_one(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    store: &DomainStore,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    item: &ContentItem,
    on_change: &impl Fn(),
) -> anyhow::Result<()> {
    if !settings.lock().hub_ready() || !item.may_sync_remote() {
        return Ok(());
    }
    store.set_status(item.id(), ContentStatus::Uploading)?;
    let result = publish(identity, vault, store, paths, settings, item).await;
    let status = match &result {
        Ok(true) => ContentStatus::SyncedToHub,
        Ok(false) => ContentStatus::Local,
        Err(_) => ContentStatus::Failed,
    };
    store.set_status(item.id(), status)?;
    on_change();
    result.map(|_| ())
}

async fn accept_opt(
    listener: Option<&tokio::net::TcpListener>,
    cert: &DeviceCert,
    trust: &Arc<Mutex<TrustStore>>,
) -> anyhow::Result<(
    tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    std::net::SocketAddr,
    Option<[u8; 32]>,
)> {
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
    store: &DomainStore,
    paths: &AppPaths,
) -> anyhow::Result<()> {
    let snapshot: Vec<DiscoveredPeer> = peers.lock().values().cloned().collect();
    if snapshot.is_empty() {
        return Ok(());
    }
    let payload = load_payload(item, store, paths)?;
    let payload = payload.into_bytes()?;
    let meta = serde_json::to_string(&item.metadata())?;
    let offer = ItemOffer {
        item_id: item.id(),
        kind: item.kind().as_str().to_string(),
        logical_size: item.logical_size(),
        payload_size: payload.len() as u64,
        dedup_tag: item.dedup_tag().to_vec(),
        flags: item.flags().bits(),
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
    ingestion: &Ingestion,
    store: &DomainStore,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    cache_pin: &parking_lot::RwLock<Option<String>>,
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
    let Some((item, _)) = persist_item(
        ingestion,
        lan_item.offer.item_id,
        from,
        kind,
        lan_item.offer.flags,
        tag,
        metadata,
        payload,
        paths,
        true,
        None,
        Some(lan_item.offer.logical_size),
        Some(lan_item.offer.payload_size),
    )?
    else {
        return Ok(());
    };
    if auto_receive
        && let Ok(content) = item_to_clipboard(&item, store, paths, &host_read_grant(item.id()))
    {
        guard.remember(item.id(), content.dedup_tag());
        let _ = NativeClipboard.write(&content);
        *cache_pin.write() = item.metadata().local_cache_rel.clone().or_else(|| {
            matches!(item.kind(), ContentKind::Gif | ContentKind::Video)
                .then(|| item.id().to_string())
        });
    }
    Ok(())
}

fn persist_item(
    ingestion: &Ingestion,
    id: asterism_core::ContentId,
    origin: asterism_core::DeviceId,
    kind: ContentKind,
    flags: u32,
    tag: [u8; 32],
    metadata: ItemMetadata,
    bytes: Vec<u8>,
    paths: &AppPaths,
    from_lan: bool,
    created_at_ms: Option<i64>,
    logical_size: Option<u64>,
    payload_size: Option<u64>,
) -> anyhow::Result<Option<(ContentItem, Option<FileManifest>)>> {
    let spec = RemoteItemSpec {
        id,
        origin,
        kind,
        flags: ContentFlags::from_bits_truncate(flags),
        tag,
        metadata,
        from_lan,
        created_at_ms,
        logical_size,
        payload_size,
    };
    let body = match kind {
        ContentKind::Files => {
            let cache = paths.item_cache(id);
            let manifest = match unpack_file_bundle(&bytes, &cache) {
                Ok((manifest, _)) => manifest,
                Err(_) => {
                    let roots = unpack_tree(&bytes, &cache)?;
                    asterism_clipboard::preflight_paths(&roots)?
                }
            };
            RemoteItemBody::Files(manifest)
        }
        _ => RemoteItemBody::Bytes(bytes),
    };
    ingestion.persist_remote_item(spec, body)
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
    store: &DomainStore,
    paths: &AppPaths,
    settings: &Arc<Mutex<SyncSettings>>,
    item: &ContentItem,
) -> anyhow::Result<bool> {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return Ok(false);
    }
    let payload = load_payload(item, store, paths)?;
    let meta = serde_json::to_vec(&item.metadata())?;
    let mut pkg = pack(vault, &meta, payload.small_bytes())?;
    let client = client(&snap)?;
    let mut blob_id = None;
    if pkg.body.is_none() {
        let id = client.begin_blob().await?;
        let count = upload_payload_chunks(&client, &id, vault, &payload).await?;
        client.commit_blob(&id, count).await?;
        pkg.chunk_count = count;
        blob_id = Some(id);
    }
    let dto = HistoryDto {
        id: hex::encode(item.id().as_bytes()),
        origin_device_id: identity.device_id,
        kind: item.kind().as_str().to_string(),
        created_at_ms: item.created_at_ms(),
        logical_size: item.logical_size(),
        payload_size: item.payload_size(),
        dedup_tag: hex::encode(item.dedup_tag()),
        flags: item.flags().bits(),
        encrypted_metadata: encode_package(&pkg)?,
        blob_id,
    };
    client.publish_history(&dto).await?;
    persist_hub_pin(settings, paths, &client);
    if let Ok(mut ws) = client.connect_ws().await {
        let offer = ItemOffer {
            item_id: item.id(),
            kind: item.kind().as_str().to_string(),
            logical_size: item.logical_size(),
            payload_size: item.payload_size(),
            dedup_tag: item.dedup_tag().to_vec(),
            flags: item.flags().bits(),
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
                    MessageBody::ItemReady(ItemReady { item_id: item.id(), blob_id: blob.clone() }),
                ),
            )
            .await;
        }
    }
    Ok(true)
}

async fn pull_hub(
    identity: &LocalIdentity,
    vault: &AccountVaultKey,
    ingestion: &Ingestion,
    store: &DomainStore,
    paths: &AppPaths,
    guard: &SelfWriteGuard,
    cache_pin: &parking_lot::RwLock<Option<String>>,
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
        match apply_remote(vault, ingestion, store, paths, guard, &client, dto.clone(), false).await
        {
            Ok(Some(item)) => newest = Some(item),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(id = %dto.id, error = %err, "retry remote item failed");
                remember_failed(failed_remote, dto);
            }
        }
    }
    save_failed_remote(store, failed_remote);
    persist_hub_pin(settings, paths, &client);
    let items = client.history(last_cursor.as_deref(), 50).await?;
    for dto in items {
        let next_cursor = format!("{}:{}", dto.created_at_ms, dto.id);
        match apply_remote(vault, ingestion, store, paths, guard, &client, dto.clone(), false).await
        {
            Ok(Some(item)) => {
                newest = Some(item);
                on_change();
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(id = %dto.id, error = %err, "remote item failed; isolated for retry");
                remember_failed(failed_remote, dto);
                // 先持久化失败队列，再推进 cursor，避免进程退出后永久跳过该项。
                save_failed_remote(store, failed_remote);
            }
        }
        // 单条损坏或暂时不可取的记录不能卡住同页后续记录。
        persist_cursor(store, last_cursor, next_cursor);
    }
    if snap.auto_receive
        && let Some(item) = newest
        && item.origin_device_id() != identity.device_id
        && let Ok(content) = item_to_clipboard(&item, store, paths, &host_read_grant(item.id()))
    {
        guard.remember(item.id(), content.dedup_tag());
        let _ = NativeClipboard.write(&content);
        *cache_pin.write() = item.metadata().local_cache_rel.clone().or_else(|| {
            matches!(item.kind(), ContentKind::Gif | ContentKind::Video)
                .then(|| item.id().to_string())
        });
    }
    Ok(())
}

const TOMBSTONE_SCOPE: &str = "hub_tombstones";

#[cfg(test)]
fn remember_tombstone(store: &DomainStore, id: asterism_core::ContentId) {
    let hex_id = hex::encode(id.as_bytes());
    let mut ids = load_tombstones(store);
    if !ids.iter().any(|existing| existing == &hex_id) {
        ids.push(hex_id);
        save_tombstones(store, &ids);
    }
}

fn load_tombstones(store: &DomainStore) -> Vec<String> {
    store
        .kv_get(TOMBSTONE_SCOPE)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_tombstones(store: &DomainStore, ids: &[String]) {
    let encoded = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
    if let Err(err) = store.kv_set(TOMBSTONE_SCOPE, &encoded) {
        tracing::warn!(error = %err, "persist hub tombstones");
    }
}

async fn flush_tombstones(
    store: &DomainStore,
    settings: &Arc<Mutex<SyncSettings>>,
) -> anyhow::Result<()> {
    let snap = settings.lock().clone();
    if !snap.hub_url.as_ref().is_some_and(|url| !url.is_empty()) {
        return Ok(());
    }
    if !snap.hub_ready() {
        return Ok(());
    }
    let client = client(&snap)?;
    let mut remain = Vec::new();
    for id in load_tombstones(store) {
        if let Err(err) = client.delete_history(&id).await {
            tracing::warn!(id, error = %err, "hub delete item failed");
            remain.push(id);
        }
    }
    save_tombstones(store, &remain);

    let pending = store
        .pending_outbox_for(
            asterism_storage::EVENT_DELETED,
            asterism_storage::CONSUMER_HUB_DELETE,
            50,
        )
        .unwrap_or_default();
    let mut failed = !remain.is_empty();
    for event in pending {
        let hex_id = hex::encode(event.aggregate_id.as_bytes());
        match client.delete_history(&hex_id).await {
            Ok(()) => {
                if let Err(err) =
                    store.ack_outbox_consumer(event.id, asterism_storage::CONSUMER_HUB_DELETE)
                {
                    tracing::warn!(error = %err, "ack deleted outbox");
                    failed = true;
                }
            }
            Err(err) => {
                tracing::warn!(id = hex_id, error = %err, "hub delete item failed");
                let _ = store.retry_outbox_consumer(
                    event.id,
                    asterism_storage::CONSUMER_HUB_DELETE,
                    Duration::from_secs(8),
                );
                failed = true;
            }
        }
    }
    if failed { Err(anyhow::anyhow!("hub still has pending deletes")) } else { Ok(()) }
}

fn remember_failed(failed: &mut Vec<HistoryDto>, dto: HistoryDto) {
    if failed.iter().any(|existing| existing.id == dto.id) {
        return;
    }
    failed.push(dto);
}

const FAILED_REMOTE_SCOPE: &str = "hub_failed_remote";

fn load_failed_remote(store: &DomainStore) -> Vec<HistoryDto> {
    store
        .kv_get(FAILED_REMOTE_SCOPE)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_failed_remote(store: &DomainStore, failed: &[HistoryDto]) {
    match serde_json::to_string(failed) {
        Ok(raw) => {
            if let Err(err) = store.kv_set(FAILED_REMOTE_SCOPE, &raw) {
                tracing::warn!(error = %err, "persist failed remote retry queue");
            }
        }
        Err(err) => tracing::warn!(error = %err, "encode failed remote retry queue"),
    }
}

fn persist_cursor(store: &DomainStore, last_cursor: &mut Option<String>, next: String) {
    *last_cursor = Some(next.clone());
    if let Err(err) = store.set_hub_cursor(&next) {
        tracing::warn!(error = %err, "persist hub cursor");
    }
}

enum RemotePayload {
    Bytes(Vec<u8>),
    StagedFile(TemporaryFile),
}

impl RemotePayload {
    fn write_all(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        match self {
            Self::Bytes(target) => target.extend_from_slice(bytes),
            Self::StagedFile(target) => target.file.write_all(bytes)?,
        }
        Ok(())
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        if let Self::StagedFile(target) = self {
            target.file.flush()?;
            target.file.sync_all()?;
        }
        Ok(())
    }
}

struct TemporaryFile {
    path: std::path::PathBuf,
    file: std::fs::File,
}

impl TemporaryFile {
    fn create(path: std::path::PathBuf) -> anyhow::Result<Self> {
        let file = std::fs::File::create(&path)?;
        Ok(Self { path, file })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn persist_file_item_from_archive(
    ingestion: &Ingestion,
    id: asterism_core::ContentId,
    origin: asterism_core::DeviceId,
    flags: u32,
    tag: [u8; 32],
    metadata: ItemMetadata,
    archive: &std::path::Path,
    paths: &AppPaths,
    created_at_ms: i64,
    logical_size: u64,
    payload_size: u64,
) -> anyhow::Result<Option<(ContentItem, Option<FileManifest>)>> {
    let cache = paths.item_cache(id);
    let (manifest, _) = unpack_file_bundle_reader(std::fs::File::open(archive)?, &cache)?;
    ingestion.persist_remote_item(
        RemoteItemSpec {
            id,
            origin,
            kind: ContentKind::Files,
            flags: ContentFlags::from_bits_truncate(flags),
            tag,
            metadata,
            from_lan: false,
            created_at_ms: Some(created_at_ms),
            logical_size: Some(logical_size),
            payload_size: Some(payload_size),
        },
        RemoteItemBody::Files(manifest),
    )
}

async fn apply_remote(
    vault: &AccountVaultKey,
    ingestion: &Ingestion,
    store: &DomainStore,
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
    let kind = ContentKind::parse(&dto.kind).unwrap_or(ContentKind::Text);
    let mut payload = unpack_body(vault, &pkg)?.map(RemotePayload::Bytes);
    if payload.is_none()
        && let Some(blob) = &dto.blob_id
    {
        if pkg.chunk_count > 0 {
            let mut decryptor = BlobChunkDecryptor::default();
            let mut target = if kind == ContentKind::Files {
                let staging = paths.cache_dir.join("sync-staging");
                std::fs::create_dir_all(&staging)?;
                RemotePayload::StagedFile(TemporaryFile::create(
                    staging.join(format!("{}.download", dto.id)),
                )?)
            } else {
                RemotePayload::Bytes(Vec::with_capacity(dto.payload_size as usize))
            };
            for index in 0..pkg.chunk_count {
                let encoded = client.get_chunk(blob, index).await?;
                target.write_all(&decryptor.decrypt(vault, index, &encoded)?)?;
            }
            decryptor.finish(vault)?;
            target.flush()?;
            payload = Some(target);
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
                payload =
                    Some(RemotePayload::Bytes(asterism_crypto::decrypt_metadata(vault, &enc)?));
            }
        }
    }
    let Some(payload) = payload else {
        anyhow::bail!("remote item {} missing payload", dto.id);
    };
    let mut tag = [0u8; 32];
    if let Ok(decoded) = hex::decode(&dto.dedup_tag)
        && decoded.len() == 32
    {
        tag.copy_from_slice(&decoded);
    }
    let persisted = match payload {
        RemotePayload::Bytes(bytes) => persist_item(
            ingestion,
            remote_id,
            dto.origin_device_id,
            kind,
            dto.flags,
            tag,
            metadata,
            bytes,
            paths,
            false,
            Some(dto.created_at_ms),
            Some(dto.logical_size),
            Some(dto.payload_size),
        )?,
        RemotePayload::StagedFile(file) => persist_file_item_from_archive(
            ingestion,
            remote_id,
            dto.origin_device_id,
            dto.flags,
            tag,
            metadata,
            file.path(),
            paths,
            dto.created_at_ms,
            dto.logical_size,
            dto.payload_size,
        )?,
    };
    let Some((item, _)) = persisted else {
        return Ok(None);
    };
    if write_clipboard
        && let Ok(content) = item_to_clipboard(&item, store, paths, &host_read_grant(item.id()))
    {
        _guard.remember(item.id(), content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(Some(item))
}

fn host_read_grant(id: asterism_core::ContentId) -> ContentReadGrant {
    asterism_plugin_api::PermissionBroker::host()
        .grant_host_transfer(id)
        .expect("host broker issues transfer grant")
}

enum PayloadSource {
    Bytes(Vec<u8>),
    StagedFile(std::path::PathBuf),
}

impl PayloadSource {
    fn small_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) if bytes.len() <= 256 * 1024 => Some(bytes),
            _ => None,
        }
    }

    fn open(&self) -> anyhow::Result<Box<dyn Read + '_>> {
        match self {
            Self::Bytes(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.as_slice()))),
            Self::StagedFile(path) => Ok(Box::new(std::fs::File::open(path)?)),
        }
    }

    fn into_bytes(mut self) -> anyhow::Result<Vec<u8>> {
        match &mut self {
            Self::Bytes(bytes) => Ok(std::mem::take(bytes)),
            Self::StagedFile(path) => Ok(std::fs::read(path)?),
        }
    }
}

impl Drop for PayloadSource {
    fn drop(&mut self) {
        if let Self::StagedFile(path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn upload_payload_chunks(
    client: &HubClient,
    blob_id: &str,
    vault: &AccountVaultKey,
    payload: &PayloadSource,
) -> anyhow::Result<u32> {
    let hash = match payload {
        PayloadSource::Bytes(bytes) => asterism_crypto::blake3_bytes(bytes),
        PayloadSource::StagedFile(path) => {
            asterism_crypto::blake3_reader(std::fs::File::open(path)?)?
        }
    };
    let encryptor = BlobChunkEncryptor::new(vault, hash)?;
    let mut reader = payload.open()?;
    let mut buffer = vec![0u8; asterism_crypto::CHUNK_SIZE];
    let mut count = 0u32;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        client.put_chunk(blob_id, count, encryptor.encrypt(count, &buffer[..read])?).await?;
        count = count.checked_add(1).ok_or_else(|| anyhow::anyhow!("blob has too many chunks"))?;
    }
    Ok(count)
}

fn load_payload(
    item: &ContentItem,
    store: &DomainStore,
    paths: &AppPaths,
) -> anyhow::Result<PayloadSource> {
    match &item.payload_ref() {
        PayloadRef::Inline { bytes } => Ok(PayloadSource::Bytes(bytes.to_vec())),
        PayloadRef::Blob { blob_id } => Ok(PayloadSource::Bytes(store.get_blob(blob_id)?)),
        PayloadRef::FileManifest { manifest_id } => {
            let cache = item
                .metadata()
                .local_cache_rel
                .as_ref()
                .map(|rel| paths.cache_dir.join("items").join(rel))
                .unwrap_or_else(|| paths.item_cache(item.id()));
            if cache.exists() {
                let manifest = store.load_manifest(*manifest_id)?;
                let staging = paths.cache_dir.join("sync-staging");
                std::fs::create_dir_all(&staging)?;
                let path = staging.join(format!("{}.asb", item.id()));
                let mut file = std::fs::File::create(&path)?;
                pack_file_bundle_to_writer(&manifest, &cache, &mut file)?;
                file.sync_all()?;
                Ok(PayloadSource::StagedFile(path))
            } else {
                anyhow::bail!(
                    "file cache missing for {}; not publishing metadata-only item",
                    item.id()
                )
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

    fn domain(store: &Arc<Store>) -> Arc<DomainStore> {
        DomainStore::wrap(Arc::clone(store))
    }

    #[test]
    fn persist_new_treats_existing_id_as_not_new() {
        let root = std::env::temp_dir()
            .join(format!("asterism-persist-new-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let avk = Arc::new(parking_lot::RwLock::new(AccountVaultKey::generate()));
        let ingestion =
            Ingestion::new(Arc::clone(&store), paths.clone(), asterism_core::DeviceId::new(), avk);
        let spec = RemoteItemSpec {
            id: asterism_core::ContentId::new(),
            origin: asterism_core::DeviceId::new(),
            kind: ContentKind::Text,
            flags: ContentFlags::empty(),
            tag: [2; 32],
            metadata: ItemMetadata::default(),
            from_lan: false,
            created_at_ms: Some(1),
            logical_size: Some(1),
            payload_size: Some(1),
        };
        assert!(
            ingestion.persist_remote(spec.clone(), RemoteItemBody::Bytes(b"x".to_vec())).unwrap()
        );
        assert!(!ingestion.persist_remote(spec, RemoteItemBody::Bytes(b"x".to_vec())).unwrap());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_delete_writes_outbox_not_kv_tombstone() {
        let root = std::env::temp_dir()
            .join(format!("asterism-delete-outbox-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let avk = Arc::new(parking_lot::RwLock::new(AccountVaultKey::generate()));
        let ingestion =
            Ingestion::new(Arc::clone(&store), paths.clone(), asterism_core::DeviceId::new(), avk);
        let spec = RemoteItemSpec {
            id: asterism_core::ContentId::new(),
            origin: asterism_core::DeviceId::new(),
            kind: ContentKind::Text,
            flags: ContentFlags::empty(),
            tag: [3; 32],
            metadata: ItemMetadata::default(),
            from_lan: false,
            created_at_ms: Some(1),
            logical_size: Some(1),
            payload_size: Some(1),
        };
        let item_id = spec.id;
        assert!(ingestion.persist_remote(spec, RemoteItemBody::Bytes(b"x".to_vec())).unwrap());
        let item = store.get(item_id).unwrap();
        store.delete(item.id()).unwrap();
        assert!(load_tombstones(&domain(&store)).is_empty());
        let types: Vec<_> =
            store.pending_outbox(10).unwrap().into_iter().map(|event| event.event_type).collect();
        assert!(types.iter().any(|ty| ty == asterism_storage::EVENT_COMMITTED));
        assert!(types.iter().any(|ty| ty == asterism_storage::EVENT_DELETED));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tombstones_round_trip_and_dedup() {
        let root = std::env::temp_dir()
            .join(format!("asterism-tombstone-{}", asterism_core::ContentId::new()));
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let id = asterism_core::ContentId::new();
        remember_tombstone(&domain(&store), id);
        remember_tombstone(&domain(&store), id);
        let listed = load_tombstones(&domain(&store));
        assert_eq!(listed, vec![hex::encode(id.as_bytes())]);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remember_failed_dedups_by_item_id() {
        let dto = HistoryDto {
            id: "aa".into(),
            origin_device_id: asterism_core::DeviceId::new(),
            kind: "TEXT".into(),
            created_at_ms: 1,
            logical_size: 1,
            payload_size: 1,
            dedup_tag: String::new(),
            flags: 0,
            encrypted_metadata: String::new(),
            blob_id: None,
        };
        let mut failed = Vec::new();
        remember_failed(&mut failed, dto.clone());
        remember_failed(&mut failed, dto);
        assert_eq!(failed.len(), 1);
    }

    #[test]
    fn failed_remote_retry_queue_survives_restart() {
        let root = std::env::temp_dir()
            .join(format!("asterism-failed-remote-{}", asterism_core::ContentId::new()));
        let store = Store::open(&root).unwrap();
        let dto = HistoryDto {
            id: "aa".into(),
            origin_device_id: asterism_core::DeviceId::new(),
            kind: "TEXT".into(),
            created_at_ms: 1,
            logical_size: 1,
            payload_size: 1,
            dedup_tag: String::new(),
            flags: 0,
            encrypted_metadata: "ciphertext".into(),
            blob_id: None,
        };
        save_failed_remote(&domain(&store), std::slice::from_ref(&dto));
        drop(store);
        let reopened = Store::open(&root).unwrap();
        assert_eq!(load_failed_remote(&domain(&reopened))[0].id, dto.id);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

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
        let bundle = asterism_sync::pack_file_bundle(&manifest, &source).unwrap();
        let paths = AppPaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            config_dir: root.join("config"),
        };
        paths.ensure().unwrap();
        let store = Store::open(&paths.data_dir).unwrap();
        let avk = Arc::new(parking_lot::RwLock::new(AccountVaultKey::generate()));
        let ingestion =
            Ingestion::new(Arc::clone(&store), paths.clone(), asterism_core::DeviceId::new(), avk);

        let Some((item, _)) = persist_item(
            &ingestion,
            asterism_core::ContentId::new(),
            asterism_core::DeviceId::new(),
            ContentKind::Files,
            ContentFlags::REMOTE_ALLOWED.bits(),
            [7; 32],
            ItemMetadata::default(),
            bundle,
            &paths,
            false,
            None,
            None,
            None,
        )
        .unwrap() else {
            panic!("expected new remote file item");
        };
        let clipboard =
            item_to_clipboard(&item, &domain(&store), &paths, &host_read_grant(item.id())).unwrap();

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
        let avk = Arc::new(parking_lot::RwLock::new(AccountVaultKey::generate()));
        let ingestion =
            Ingestion::new(Arc::clone(&store), paths.clone(), asterism_core::DeviceId::new(), avk);
        let (item, _) = ingestion
            .assemble_remote(
                RemoteItemSpec {
                    id: asterism_core::ContentId::new(),
                    origin: asterism_core::DeviceId::new(),
                    kind: ContentKind::Files,
                    flags: ContentFlags::empty(),
                    tag: [1; 32],
                    metadata: ItemMetadata::default(),
                    from_lan: false,
                    created_at_ms: Some(1),
                    logical_size: Some(4),
                    payload_size: Some(4),
                },
                RemoteItemBody::Files(FileManifest {
                    id: asterism_core::ManifestId::new(),
                    root_name: "missing".into(),
                    entries: Vec::new(),
                    unsupported: Vec::new(),
                }),
            )
            .unwrap();
        assert!(load_payload(&item, &domain(&store), &paths).is_err());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
