use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Ready,
    Unavailable(String),
}

#[derive(Default)]
pub struct HealthBoard {
    states: Mutex<HashMap<String, Health>>,
}

impl HealthBoard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, plugin_id: &str, health: Health) {
        self.states.lock().expect("health").insert(plugin_id.to_string(), health);
    }

    pub fn get(&self, plugin_id: &str) -> Option<Health> {
        self.states.lock().expect("health").get(plugin_id).cloned()
    }
}
