use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::content::ContentItem;
use crate::id::{ContentId, DeviceId};

/// V1 内置 Action。未来 OCR / AI 走同一 Registry，不写进截图模块。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    Copy,
    Save,
    SendToDevice,
    Favorite,
    Delete,
}

#[derive(Clone, Debug)]
pub struct ActionContext {
    pub local_device_id: DeviceId,
    pub target_device_id: Option<DeviceId>,
    pub save_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionResult {
    Copied { id: ContentId },
    Saved { path: PathBuf },
    Sent { id: ContentId, device_id: DeviceId },
    Favorited { id: ContentId, favorite: bool },
    Deleted { id: ContentId },
}

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("action not supported for this item")]
    Unsupported,
    #[error("missing target device")]
    MissingTarget,
    #[error("missing save path")]
    MissingSavePath,
    #[error("{0}")]
    Failed(String),
}

#[async_trait]
pub trait ContentAction: Send + Sync {
    fn id(&self) -> ActionId;
    fn supports(&self, item: &ContentItem) -> bool;
    async fn execute(
        &self,
        ctx: &ActionContext,
        item: &ContentItem,
    ) -> Result<ActionResult, ActionError>;
}

/// 截图 Toolbar 与 History 右键菜单共用。
#[derive(Default)]
pub struct ActionRegistry {
    actions: Vec<Box<dyn ContentAction>>,
}

impl ActionRegistry {
    pub fn register(&mut self, action: Box<dyn ContentAction>) {
        self.actions.push(action);
    }

    pub fn supported(&self, item: &ContentItem) -> Vec<ActionId> {
        self.actions.iter().filter(|a| a.supports(item)).map(|a| a.id()).collect()
    }

    pub fn get(&self, id: ActionId) -> Option<&dyn ContentAction> {
        self.actions.iter().find(|a| a.id() == id).map(|a| a.as_ref())
    }
}
