use std::path::PathBuf;

use asterism_capture::backend::CaptureBackend;
use asterism_capture::{AnnotationScene, OverlaySession, XcapBackend, export_png};
use asterism_clipboard::ClipboardBackend;
use asterism_core::action::ActionId;
use asterism_core::content::{ContentFlags, ContentKind};
use asterism_core::id::ContentId;
use asterism_storage::HistoryQuery;
use serde::Serialize;
use tauri::State;

use crate::actions;
use crate::runtime::{DesktopState, item_to_clipboard};
use crate::settings::SyncSettings;
use crate::sync_engine;

#[derive(Debug, thiserror::Error)]
pub enum CmdError {
    #[error("{0}")]
    Any(String),
}

impl From<anyhow::Error> for CmdError {
    fn from(err: anyhow::Error) -> Self {
        Self::Any(err.to_string())
    }
}

impl Serialize for CmdError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemDto {
    pub id: String,
    pub kind: String,
    pub created_at_ms: i64,
    pub preview: Option<String>,
    pub favorite: bool,
    pub source_app: Option<String>,
    pub logical_size: u64,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub file_count: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDto {
    pub device_id: String,
    pub account_id: String,
    pub device_name: String,
}

#[tauri::command]
pub fn list_history(
    state: State<'_, DesktopState>,
    query: Option<String>,
    kind: Option<String>,
    favorite_only: bool,
    limit: Option<u32>,
) -> Result<Vec<HistoryItemDto>, CmdError> {
    let kind = kind
        .map(|k| ContentKind::parse(&k))
        .transpose()
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let items = state
        .store
        .history(HistoryQuery {
            kind,
            favorite_only,
            query,
            limit: limit.unwrap_or(80),
            before_ms: None,
        })
        .map_err(|e| CmdError::Any(e.to_string()))?;
    Ok(items.into_iter().map(to_dto).collect())
}

#[tauri::command]
pub fn set_favorite(
    state: State<'_, DesktopState>,
    id: String,
    favorite: bool,
) -> Result<(), CmdError> {
    let id = id.parse::<ContentId>().map_err(|e| CmdError::Any(e.to_string()))?;
    state.store.set_favorite(id, favorite).map_err(|e| CmdError::Any(e.to_string()))
}

#[tauri::command]
pub fn delete_item(state: State<'_, DesktopState>, id: String) -> Result<(), CmdError> {
    let id = id.parse::<ContentId>().map_err(|e| CmdError::Any(e.to_string()))?;
    state.store.delete(id).map_err(|e| CmdError::Any(e.to_string()))?;
    state.sync.notify_deleted(id);
    Ok(())
}

#[tauri::command]
pub fn copy_item(state: State<'_, DesktopState>, id: String) -> Result<(), CmdError> {
    let id = id.parse::<ContentId>().map_err(|e| CmdError::Any(e.to_string()))?;
    let item = state.store.get(id).map_err(|e| CmdError::Any(e.to_string()))?;
    let content = item_to_clipboard(&item, &state.store, &state.paths)?;
    state.guard.remember(item.id, content.dedup_tag());
    state.clipboard.write(&content).map_err(|e| CmdError::Any(e.to_string()))
}

#[tauri::command]
pub fn get_identity(state: State<'_, DesktopState>) -> IdentityDto {
    IdentityDto {
        device_id: state.identity.device_id.to_string(),
        account_id: state.identity.account_id.to_string(),
        device_name: state.identity.device_name.clone(),
    }
}

#[tauri::command]
pub fn execute_action(
    state: State<'_, DesktopState>,
    action: String,
    id: String,
    save_path: Option<String>,
) -> Result<String, CmdError> {
    let action: ActionId =
        serde_json::from_str(&format!("\"{action}\"")).map_err(|e| CmdError::Any(e.to_string()))?;
    let id = id.parse::<ContentId>().map_err(|e| CmdError::Any(e.to_string()))?;
    let result = actions::execute(&state, action, id, save_path.map(PathBuf::from))?;
    Ok(format!("{result:?}"))
}

#[tauri::command]
pub fn recovery_key(state: State<'_, DesktopState>) -> String {
    state.vault.read().recovery_hex()
}

#[tauri::command]
pub async fn capture_fullscreen(state: State<'_, DesktopState>) -> Result<String, CmdError> {
    let (png, width, height) = tauri::async_runtime::spawn_blocking(|| {
        let backend = XcapBackend;
        backend.permission_preflight().map_err(|e| e.to_string())?;
        let monitors = backend.list_monitors().map_err(|e| e.to_string())?;
        let monitor = asterism_capture::preferred_monitor(&monitors)
            .ok_or_else(|| "no monitor".to_string())?;
        let frame = backend.capture_display(monitor).map_err(|e| e.to_string())?;
        let png = export_png(frame.width, frame.height, &frame.bgra, &AnnotationScene::default())?;
        Ok::<_, String>((png, frame.width, frame.height))
    })
    .await
    .map_err(|e| CmdError::Any(e.to_string()))?
    .map_err(CmdError::Any)?;
    insert_screenshot(&state, png, width, height)
}

pub fn insert_screenshot(
    state: &DesktopState,
    png: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<String, CmdError> {
    let local_tag = asterism_crypto::local_dedup_tag(&png);
    let blob = state.store.put_blob(&png).map_err(|e| CmdError::Any(e.to_string()))?;
    let mut item = asterism_clipboard::NormalizedContent::Image {
        png: Vec::new(),
        width,
        height,
        dedup_tag: state.vault.read().avk.dedup_tag(&asterism_crypto::blake3_bytes(&png)),
        flags: asterism_core::ContentFlags::REMOTE_ALLOWED,
        source_app: Some("asterism".into()),
    }
    .into_item(state.identity.device_id, asterism_platform::now_ms());
    item.kind = asterism_core::ContentKind::Screenshot;
    item.payload_ref = asterism_core::PayloadRef::Blob { blob_id: blob };
    item.logical_size = png.len() as u64;
    item.payload_size = png.len() as u64;
    let id = state.store.insert(item.clone(), None).map_err(|e| CmdError::Any(e.to_string()))?;
    let written = asterism_clipboard::NormalizedContent::Image {
        png,
        width,
        height,
        dedup_tag: local_tag,
        flags: item.flags,
        source_app: item.metadata.source_app.clone(),
    };
    state.guard.remember(item.id, local_tag);
    if let Err(err) = state.clipboard.write(&written) {
        tracing::warn!(error = %err, "failed to write screenshot to clipboard");
    }
    state.sync.notify_local(item);
    Ok(id.to_string())
}

#[tauri::command]
pub async fn capture_region(state: State<'_, DesktopState>) -> Result<String, CmdError> {
    let (png, w, h) = tauri::async_runtime::spawn_blocking(|| {
        let backend = XcapBackend;
        backend.permission_preflight().map_err(|e| e.to_string())?;
        let monitors = backend.list_monitors().map_err(|e| e.to_string())?;
        let monitor = asterism_capture::preferred_monitor(&monitors)
            .ok_or_else(|| "no monitor".to_string())?;
        let frame = backend.capture_display(monitor).map_err(|e| e.to_string())?;
        let selection =
            crate::overlay_cli::select_region_subprocess(&frame).map_err(|e| e.to_string())?;
        let Some(selection) = selection else {
            return Err("cancelled".into());
        };
        let session = OverlaySession { frame, selection: Some(selection) };
        let (w, h, bgra) = session.crop_bgra().ok_or_else(|| "empty selection".to_string())?;
        let png = export_png(w, h, &bgra, &AnnotationScene::default())?;
        Ok::<_, String>((png, w, h))
    })
    .await
    .map_err(|e| CmdError::Any(e.to_string()))?
    .map_err(CmdError::Any)?;
    insert_screenshot(&state, png, w, h)
}

#[tauri::command]
pub fn get_sync_settings(state: State<'_, DesktopState>) -> SyncSettings {
    state.sync.settings.lock().clone()
}

#[tauri::command]
pub fn save_sync_settings(
    state: State<'_, DesktopState>,
    settings: SyncSettings,
) -> Result<(), CmdError> {
    let current = SyncSettings::load(&state.paths.config_dir);
    let mut settings = settings;
    if settings.hub_cert_sha256.is_none() {
        settings.hub_cert_sha256 = current.hub_cert_sha256;
    }
    if settings.pending_pair_salt.is_none() {
        settings.pending_pair_salt = current.pending_pair_salt;
        settings.pending_pair_code = current.pending_pair_code.or(settings.pending_pair_code);
    }
    settings.save(&state.paths.config_dir).map_err(|e| CmdError::Any(e.to_string()))?;
    *state.sync.settings.lock() = settings;
    state.sync.reload();
    Ok(())
}

#[tauri::command]
pub async fn connect_hub(
    state: State<'_, DesktopState>,
    url: String,
    pairing_code: Option<String>,
) -> Result<String, CmdError> {
    let mut settings = state.sync.settings.lock().clone();
    let bootstrap = sync_engine::bootstrap_hub(
        &mut settings,
        &state.identity,
        &state.paths.config_dir,
        url,
        pairing_code,
    )
    .await
    .map_err(|e| CmdError::Any(e.to_string()))?;
    if let Some(vault) = bootstrap.vault {
        persist_and_activate_vault(&state, vault)?;
    }
    *state.sync.settings.lock() = settings;
    state.sync.reload();
    Ok(bootstrap.code)
}

#[tauri::command]
pub async fn hub_pairing_code(state: State<'_, DesktopState>) -> Result<String, CmdError> {
    let mut settings = state.sync.settings.lock().clone();
    let code = sync_engine::start_pairing_code(&mut settings, &state.paths.config_dir)
        .await
        .map_err(|e| CmdError::Any(e.to_string()))?;
    *state.sync.settings.lock() = settings;
    Ok(code)
}

#[tauri::command]
pub async fn hub_devices(
    state: State<'_, DesktopState>,
) -> Result<Vec<asterism_sync::hub_client::DeviceDto>, CmdError> {
    let settings = state.sync.settings.lock().clone();
    if settings.token.is_none() {
        return Err(CmdError::Any("no token".into()));
    }
    let client = sync_engine::hub_client_from_settings(&settings)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let devices = client.devices().await.map_err(|e| CmdError::Any(e.to_string()))?;
    let mut snap = settings;
    sync_engine::persist_hub_pin_settings(&mut snap, &state.paths.config_dir, &client);
    if state.sync.settings.lock().hub_cert_sha256.is_none() {
        state.sync.settings.lock().hub_cert_sha256 = snap.hub_cert_sha256;
    }
    Ok(devices)
}

#[tauri::command]
pub fn import_recovery(state: State<'_, DesktopState>, hex_key: String) -> Result<(), CmdError> {
    let key = asterism_crypto::RecoveryKey::decode_hex(&hex_key)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let vault = asterism_crypto::AccountVaultKey::from_bytes(*key.avk().as_bytes());
    persist_and_activate_vault(&state, vault)
}

#[tauri::command]
pub fn enable_autostart() -> Result<String, CmdError> {
    let exe = std::env::current_exe().map_err(|e| CmdError::Any(e.to_string()))?;
    let path = asterism_platform::hardening::write_autostart_plist("dev.asterism.desktop", &exe)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn publish_pairing_avk(
    state: State<'_, DesktopState>,
    code: String,
) -> Result<(), CmdError> {
    let settings = state.sync.settings.lock().clone();
    let url = settings.hub_url.clone().ok_or_else(|| CmdError::Any("no hub".into()))?;
    let token = settings.token.clone().ok_or_else(|| CmdError::Any("no token".into()))?;
    let salt_hex = settings
        .pending_pair_salt
        .clone()
        .ok_or_else(|| CmdError::Any("generate a pairing code first".into()))?;
    let salt = asterism_sync::pairing::parse_salt_hex(&salt_hex)
        .ok_or_else(|| CmdError::Any("invalid pairing salt".into()))?;
    let wrap_key = asterism_crypto::AccountVaultKey::from_bytes(
        asterism_sync::pairing::derive_wrap_key(&code, &salt),
    );
    let wrapped = {
        let vault = state.vault.read();
        asterism_crypto::encrypt_metadata(&wrap_key, vault.avk.as_bytes())
            .map_err(|e| CmdError::Any(e.to_string()))?
    };
    let client = asterism_sync::HubClient::with_pin(url, settings.hub_cert_sha256.as_deref())
        .map_err(|e| CmdError::Any(e.to_string()))?
        .with_token(token);
    client
        .deposit_avk(&code, &hex::encode(serde_json::to_vec(&wrapped).unwrap_or_default()))
        .await
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let mut snap = settings;
    sync_engine::persist_hub_pin_settings(&mut snap, &state.paths.config_dir, &client);
    if state.sync.settings.lock().hub_cert_sha256.is_none() {
        state.sync.settings.lock().hub_cert_sha256 = snap.hub_cert_sha256;
    }
    Ok(())
}

fn persist_and_activate_vault(
    state: &DesktopState,
    vault: asterism_crypto::AccountVaultKey,
) -> Result<(), CmdError> {
    let local_vault = asterism_platform::LocalVault {
        avk: asterism_crypto::AccountVaultKey::from_bytes(*vault.as_bytes()),
    };
    local_vault.save(&state.paths.config_dir).map_err(|e| CmdError::Any(e.to_string()))?;
    *state.vault.write() = local_vault;
    *state.avk.write() = asterism_crypto::AccountVaultKey::from_bytes(*vault.as_bytes());
    state.sync.replace_vault(vault);
    Ok(())
}

fn to_dto(item: asterism_core::ContentItem) -> HistoryItemDto {
    HistoryItemDto {
        id: item.id.to_string(),
        kind: item.kind.as_str().to_string(),
        created_at_ms: item.created_at_ms,
        preview: item.metadata.text_preview,
        favorite: item.flags.contains(ContentFlags::FAVORITE),
        source_app: item.metadata.source_app,
        logical_size: item.logical_size,
        image_width: item.metadata.image.as_ref().map(|i| i.width),
        image_height: item.metadata.image.as_ref().map(|i| i.height),
        file_count: item.metadata.files.as_ref().map(|f| f.file_count),
    }
}
