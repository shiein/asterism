use asterism_clipboard::ClipboardBackend;
use asterism_core::content::{ContentFlags, ContentKind};
use asterism_core::id::ContentId;
use asterism_storage::HistoryQuery;
use serde::Serialize;
use tauri::State;

use crate::runtime::{DesktopState, item_to_clipboard};

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
    state.store.delete(id).map_err(|e| CmdError::Any(e.to_string()))
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
