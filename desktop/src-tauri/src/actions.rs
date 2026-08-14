use std::path::PathBuf;

use asterism_core::action::ActionResult;
use asterism_core::id::ContentId;
use asterism_plugin_api::ActionKey;

use crate::runtime::DesktopState;

pub fn execute(
    state: &DesktopState,
    key: ActionKey,
    item_id: ContentId,
    save_path: Option<PathBuf>,
) -> anyhow::Result<ActionResult> {
    state.actions.execute(key, item_id, save_path).map_err(|err| anyhow::anyhow!("{err}"))
}
