use std::sync::Arc;
use std::thread;

use asterism_clipboard::{ClipboardBackend, NativeClipboard, SelfWriteGuard};
use asterism_core::content::{ContentFlags, ContentItem, ContentKind, ItemMetadata, PayloadRef};
use asterism_crypto::AccountVaultKey;
use asterism_platform::{AppPaths, LocalIdentity};
use asterism_storage::Store;
use asterism_sync::hub_client::{HistoryDto, HubClient};
use asterism_sync::pairing::PairingFinish;
use asterism_sync::protocol::{Envelope, ItemOffer, ItemReady, MessageBody};
use asterism_sync::{DeviceCert, decode_package, encode_package, pack, unpack_body, unpack_meta};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::runtime::{item_to_clipboard, persist_item};
use crate::settings::SyncSettings;

pub enum SyncCmd {
    LocalItem(Box<ContentItem>),
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
    vault: AccountVaultKey,
    store: Arc<Store>,
    paths: AppPaths,
    guard: Arc<SelfWriteGuard>,
    settings: Arc<Mutex<SyncSettings>>,
    mut rx: mpsc::UnboundedReceiver<SyncCmd>,
    on_change: impl Fn() + Send + Sync + 'static,
) {
    let cert = DeviceCert::generate(&identity.device_name).ok();
    if let Some(cert) = &cert
        && let Ok(lan) =
            asterism_sync::lan::LanEndpoint::announce(identity.device_id, cert.clone(), {
                settings.lock().lan_port
            })
    {
        tracing::info!(port = lan.port, "lan announce");
        std::mem::forget(lan);
    }

    let mut last_cursor: Option<String> = None;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    SyncCmd::LocalItem(item) => {
                        if let Err(err) = publish(&identity, &vault, &store, &settings, item.as_ref()).await {
                            tracing::warn!(error = %err, "publish failed");
                        }
                    }
                    SyncCmd::Reload => {
                        if let Err(err) = pull_hub(&vault, &store, &paths, &guard, &settings, &mut last_cursor, &on_change).await {
                            tracing::warn!(error = %err, "hub pull failed");
                        }
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(8)) => {
                if let Err(err) = pull_hub(&vault, &store, &paths, &guard, &settings, &mut last_cursor, &on_change).await {
                    tracing::debug!(error = %err, "hub pull");
                }
            }
        }
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
    settings: &Arc<Mutex<SyncSettings>>,
    item: &ContentItem,
) -> anyhow::Result<()> {
    let snap = settings.lock().clone();
    if !snap.hub_ready() {
        return Ok(());
    }
    let payload = load_payload(item, store)?;
    let meta = serde_json::to_vec(&item.metadata)?;
    let pkg = pack(vault, &meta, payload.as_deref())?;
    let client = client(&snap).await?;
    let mut blob_id = None;
    if pkg.body.is_none()
        && let Some(bytes) = payload
    {
        let id = client.begin_blob().await?;
        let enc = asterism_crypto::encrypt_metadata(vault, &bytes)?;
        let packed = serde_json::to_vec(&enc)?;
        client.put_chunk(&id, 0, packed).await?;
        client.commit_blob(&id, 1).await?;
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
        *last_cursor = Some(dto.created_at_ms.to_string());
        apply_remote(vault, store, paths, guard, &client, dto).await?;
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
        let raw = client.get_chunk(blob, 0).await?;
        if let Ok(enc) = serde_json::from_slice::<asterism_crypto::EncryptedPayload>(&raw) {
            payload = Some(asterism_crypto::decrypt_metadata(vault, &enc)?);
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
    if store.find_by_dedup(&tag)?.is_some() {
        return Ok(());
    }
    let kind = ContentKind::parse(&dto.kind).unwrap_or(ContentKind::Text);
    let item = match kind {
        ContentKind::Text => ContentItem {
            id: asterism_core::ContentId::new(),
            origin_device_id: dto.origin_device_id,
            kind,
            created_at_ms: dto.created_at_ms,
            logical_size: dto.logical_size,
            payload_size: bytes.len() as u64,
            dedup_tag: tag,
            flags: ContentFlags::from_bits_truncate(dto.flags) | ContentFlags::FROM_REMOTE,
            status: asterism_core::ContentStatus::SyncedToHub,
            metadata,
            payload_ref: PayloadRef::Inline { bytes: bytes::Bytes::from(bytes.clone()) },
            encrypted_metadata: bytes::Bytes::new(),
        },
        ContentKind::Image | ContentKind::Screenshot | ContentKind::Gif => {
            let blob = store.put_blob(&bytes)?;
            ContentItem {
                id: asterism_core::ContentId::new(),
                origin_device_id: dto.origin_device_id,
                kind,
                created_at_ms: dto.created_at_ms,
                logical_size: bytes.len() as u64,
                payload_size: bytes.len() as u64,
                dedup_tag: tag,
                flags: ContentFlags::from_bits_truncate(dto.flags) | ContentFlags::FROM_REMOTE,
                status: asterism_core::ContentStatus::SyncedToHub,
                metadata,
                payload_ref: PayloadRef::Blob { blob_id: blob },
                encrypted_metadata: bytes::Bytes::new(),
            }
        }
        _ => return Ok(()),
    };
    persist_item(store, item.clone(), None)?;
    if let Ok(content) = item_to_clipboard(&item, store, paths) {
        guard.remember(item.id, content.dedup_tag());
        let _ = NativeClipboard.write(&content);
    }
    Ok(())
}

fn load_payload(item: &ContentItem, store: &Store) -> anyhow::Result<Option<Vec<u8>>> {
    match &item.payload_ref {
        PayloadRef::Inline { bytes } => Ok(Some(bytes.to_vec())),
        PayloadRef::Blob { blob_id } => Ok(Some(store.get_blob(blob_id)?)),
        PayloadRef::FileManifest { .. } => Ok(None),
    }
}

pub async fn bootstrap_hub(
    settings: &mut SyncSettings,
    identity: &LocalIdentity,
    config_dir: &std::path::Path,
    hub_url: String,
) -> anyhow::Result<String> {
    settings.hub_url = Some(hub_url.trim_end_matches('/').to_string());
    let client = HubClient::new(settings.hub_url.clone().unwrap())?;
    let offer = client.pairing_start().await?;
    let finish = PairingFinish {
        code: offer.code.clone(),
        device_id: identity.device_id,
        device_name: identity.device_name.clone(),
        platform: asterism_core::DevicePlatform::current().as_str().to_string(),
        identity_public_key: vec![1],
    };
    let session = client.pairing_finish(&finish).await?;
    settings.token = Some(session.token.clone());
    settings.save(config_dir)?;
    Ok(offer.code)
}

pub async fn start_pairing_code(settings: &SyncSettings) -> anyhow::Result<String> {
    let url = settings.hub_url.clone().ok_or_else(|| anyhow::anyhow!("set hub url first"))?;
    let client = HubClient::new(url)?;
    Ok(client.pairing_start().await?.code)
}
