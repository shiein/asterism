use std::fs;
use std::path::PathBuf;

use asterism_clipboard::ClipboardBackend;
use asterism_core::action::{ActionId, ActionResult};
use asterism_core::builtin_actions;
use asterism_core::content::{ContentItem, PayloadRef};
use asterism_core::id::ContentId;

use crate::runtime::{DesktopState, item_to_clipboard};

pub fn execute(
    state: &DesktopState,
    id: ActionId,
    item_id: ContentId,
    save_path: Option<PathBuf>,
) -> anyhow::Result<ActionResult> {
    let item = state.store.get(item_id)?;
    if !builtin_actions::supports(id, &item) {
        anyhow::bail!("action not supported");
    }
    match id {
        ActionId::Copy => {
            let content = item_to_clipboard(&item, &state.store, &state.paths)?;
            state.guard.remember(item.id, content.dedup_tag());
            state.clipboard.write(&content)?;
            Ok(builtin_actions::copied(&item))
        }
        ActionId::Favorite => {
            let next = !item.flags.contains(asterism_core::ContentFlags::FAVORITE);
            state.store.set_favorite(item.id, next)?;
            Ok(builtin_actions::favorited(&item, next))
        }
        ActionId::Delete => {
            state.store.delete(item.id)?;
            Ok(builtin_actions::deleted(&item))
        }
        ActionId::Save => {
            let path = builtin_actions::require_save_path(save_path.as_ref())?;
            save_item(&item, state, &path)?;
            Ok(ActionResult::Saved { path })
        }
        ActionId::SendToDevice => anyhow::bail!("send uses sync session"),
    }
}

fn save_item(item: &ContentItem, state: &DesktopState, path: &PathBuf) -> anyhow::Result<()> {
    match &item.payload_ref {
        PayloadRef::Inline { bytes } => {
            fs::write(path, bytes)?;
        }
        PayloadRef::Blob { blob_id } => {
            fs::write(path, state.store.get_blob(blob_id)?)?;
        }
        PayloadRef::FileManifest { .. } => {
            let cache = state.paths.item_cache(item.id);
            if cache.exists() {
                copy_dir(&cache, path)?;
            }
        }
    }
    Ok(())
}

fn copy_dir(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
