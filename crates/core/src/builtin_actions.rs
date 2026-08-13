//! V1 内置 Action 的纯决策部分。真正写剪贴板 / 落盘由 Desktop 执行器完成。

use crate::action::{ActionError, ActionId, ActionResult};
use crate::content::{ContentItem, ContentKind};
use crate::id::DeviceId;

pub fn supports(id: ActionId, item: &ContentItem) -> bool {
    match id {
        ActionId::Copy | ActionId::Favorite | ActionId::Delete => true,
        ActionId::Save => matches!(
            item.kind,
            ContentKind::Text
                | ContentKind::Image
                | ContentKind::Screenshot
                | ContentKind::Files
                | ContentKind::Gif
                | ContentKind::Video
        ),
        ActionId::SendToDevice => item.may_sync_remote(),
    }
}

pub fn require_target(target: Option<DeviceId>) -> Result<DeviceId, ActionError> {
    target.ok_or(ActionError::MissingTarget)
}

pub fn require_save_path(
    ctx_path: Option<&std::path::PathBuf>,
) -> Result<std::path::PathBuf, ActionError> {
    ctx_path.cloned().ok_or(ActionError::MissingSavePath)
}

pub fn copied(item: &ContentItem) -> ActionResult {
    ActionResult::Copied { id: item.id }
}

pub fn deleted(item: &ContentItem) -> ActionResult {
    ActionResult::Deleted { id: item.id }
}

pub fn favorited(item: &ContentItem, favorite: bool) -> ActionResult {
    ActionResult::Favorited { id: item.id, favorite }
}
