use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use asterism_core::action::{ActionError, ActionResult};
use asterism_core::id::ContentId;

use crate::ids::ActionKey;

pub type ActionFn =
    Arc<dyn Fn(ContentId, Option<PathBuf>) -> Result<ActionResult, ActionError> + Send + Sync>;

/// 插件在 mount 时登记；dispatch 只查表，不再按 ActionKey 分支。
#[derive(Default)]
pub struct ActionRegistry {
    handlers: Mutex<HashMap<ActionKey, ActionFn>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        key: ActionKey,
        handler: impl Fn(ContentId, Option<PathBuf>) -> Result<ActionResult, ActionError>
        + Send
        + Sync
        + 'static,
    ) -> Result<(), ActionError> {
        let mut handlers = self.handlers.lock().expect("action registry");
        if handlers.contains_key(&key) {
            return Err(ActionError::Failed(format!("action {} already registered", key.as_str())));
        }
        handlers.insert(key, Arc::new(handler));
        Ok(())
    }

    pub fn unregister(&self, key: ActionKey) -> bool {
        self.handlers.lock().expect("action registry").remove(&key).is_some()
    }

    pub fn execute(
        &self,
        key: ActionKey,
        item_id: ContentId,
        save_path: Option<PathBuf>,
    ) -> Result<ActionResult, ActionError> {
        let handler = self
            .handlers
            .lock()
            .expect("action registry")
            .get(&key)
            .cloned()
            .ok_or_else(|| ActionError::Failed(format!("unknown action {}", key.as_str())))?;
        handler(item_id, save_path)
    }

    pub fn contains(&self, key: ActionKey) -> bool {
        self.handlers.lock().expect("action registry").contains_key(&key)
    }

    pub fn descriptors(&self) -> Vec<ActionDescriptor> {
        let mut keys: Vec<_> =
            self.handlers.lock().expect("action registry").keys().copied().collect();
        keys.sort_by_key(|key| key.as_str());
        keys.into_iter().map(ActionDescriptor::from_key).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionDescriptor {
    pub key: ActionKey,
    pub title: &'static str,
}

impl ActionDescriptor {
    pub fn from_key(key: ActionKey) -> Self {
        let title = match key {
            ActionKey::COPY => "Copy",
            ActionKey::SAVE => "Save",
            ActionKey::DELETE => "Delete",
            ActionKey::FAVORITE => "Favorite",
            ActionKey::SEND => "Send",
            ActionKey::CAPTURE_FULLSCREEN => "Screenshot",
            ActionKey::CAPTURE_REGION => "Region",
            other => other.as_str(),
        };
        Self { key, title }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_core::action::ActionResult;

    #[test]
    fn unknown_action_fails() {
        let registry = ActionRegistry::new();
        assert!(registry.execute(ActionKey::COPY, ContentId::new(), None).is_err());
    }

    #[test]
    fn registered_action_runs() {
        let registry = ActionRegistry::new();
        registry.register(ActionKey::COPY, |id, _| Ok(ActionResult::Copied { id })).unwrap();
        let id = ContentId::new();
        assert_eq!(
            registry.execute(ActionKey::COPY, id, None).unwrap(),
            ActionResult::Copied { id }
        );
        assert_eq!(registry.descriptors()[0].key, ActionKey::COPY);
        assert_eq!(registry.descriptors()[0].title, "Copy");
    }

    #[test]
    fn duplicate_action_is_rejected_and_unregister_allows_remount() {
        let registry = ActionRegistry::new();
        registry.register(ActionKey::SAVE, |_, _| Err(ActionError::Unsupported)).unwrap();
        assert!(registry.register(ActionKey::SAVE, |_, _| Err(ActionError::Unsupported)).is_err());
        assert!(registry.unregister(ActionKey::SAVE));
        registry
            .register(ActionKey::SAVE, |_, path| {
                Ok(ActionResult::Saved { path: path.unwrap_or_default() })
            })
            .unwrap();
    }
}
