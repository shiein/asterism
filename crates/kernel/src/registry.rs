use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{KernelError, Result};

/// 类型化单例 Service 表。热路径用 `require::<T>()`，不做字符串查找。
#[derive(Default)]
pub struct ServiceRegistry {
    singles: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn provide<T: Send + Sync + 'static>(&mut self, service: Arc<T>) -> Result<()> {
        let id = TypeId::of::<T>();
        if self.singles.contains_key(&id) {
            return Err(KernelError::ServiceConflict);
        }
        self.singles.insert(id, service);
        Ok(())
    }

    pub fn require<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.singles
            .get(&TypeId::of::<T>())
            .and_then(|svc| svc.clone().downcast().ok())
            .ok_or(KernelError::ServiceMissing)
    }

    pub fn revoke<T: Send + Sync + 'static>(&mut self) -> bool {
        self.singles.remove(&TypeId::of::<T>()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ping(u8);

    #[test]
    fn singleton_conflict_fails() {
        let mut reg = ServiceRegistry::new();
        reg.provide(Arc::new(Ping(1))).unwrap();
        assert!(matches!(reg.provide(Arc::new(Ping(2))), Err(KernelError::ServiceConflict)));
        assert_eq!(reg.require::<Ping>().unwrap().0, 1);
    }
}
