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
    ) {
        self.handlers.lock().expect("action registry").insert(key, Arc::new(handler));
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
        registry.register(ActionKey::COPY, |id, _| Ok(ActionResult::Copied { id }));
        let id = ContentId::new();
        assert_eq!(
            registry.execute(ActionKey::COPY, id, None).unwrap(),
            ActionResult::Copied { id }
        );
    }
}
