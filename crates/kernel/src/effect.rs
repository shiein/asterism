/// 一次注册的逆操作。Guard 按逆序执行。
pub struct EffectGuard {
    undo: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl EffectGuard {
    pub fn new(undo: impl FnOnce() + Send + 'static) -> Self {
        Self { undo: std::sync::Mutex::new(Some(Box::new(undo))) }
    }

    pub fn dismiss(&mut self) {
        if let Ok(mut undo) = self.undo.lock() {
            *undo = None;
        }
    }
}

impl Drop for EffectGuard {
    fn drop(&mut self) {
        if let Ok(mut undo) = self.undo.lock()
            && let Some(undo) = undo.take()
        {
            undo();
        }
    }
}

/// mount 成功后持有全部 effect；销毁时逆序撤销。
pub struct MountGuard {
    plugin_id: String,
    effects: Vec<EffectGuard>,
}

impl MountGuard {
    pub fn new(plugin_id: impl Into<String>, effects: Vec<EffectGuard>) -> Self {
        Self { plugin_id: plugin_id.into(), effects }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        while let Some(effect) = self.effects.pop() {
            drop(effect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mount_guard_undoes_in_reverse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        {
            let a = {
                let log = Arc::clone(&log);
                EffectGuard::new(move || log.lock().unwrap().push("a"))
            };
            let b = {
                let log = Arc::clone(&log);
                EffectGuard::new(move || log.lock().unwrap().push("b"))
            };
            let _guard = MountGuard::new("asterism.test", vec![a, b]);
        }
        assert_eq!(*log.lock().unwrap(), ["b", "a"]);
    }
}
