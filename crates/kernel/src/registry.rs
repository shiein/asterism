use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::{KernelError, Result};

struct ServiceEntry {
    service: Arc<dyn Any + Send + Sync>,
    required_permission: Option<&'static str>,
    provided_by: Option<&'static str>,
}

struct Inner {
    singles: HashMap<TypeId, ServiceEntry>,
}

/// 类型化单例 Service 表。热路径用 `require::<T>()`，不做字符串查找。
/// 内部共享，便于 MountGuard 在 drop 时撤销。
#[derive(Clone)]
pub struct ServiceRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(Inner { singles: HashMap::new() })) }
    }

    pub fn provide<T: Send + Sync + 'static>(&self, service: Arc<T>) -> Result<()> {
        self.insert::<T>(service, None, None)
    }

    /// Host 登记需声明权限才能 `require` 的密封能力。
    pub fn provide_gated<T: Send + Sync + 'static>(
        &self,
        service: Arc<T>,
        permission: &'static str,
    ) -> Result<()> {
        self.insert::<T>(service, Some(permission), None)
    }

    pub(crate) fn provide_from<T: Send + Sync + 'static>(
        &self,
        service: Arc<T>,
        plugin_id: &'static str,
    ) -> Result<()> {
        self.insert::<T>(service, None, Some(plugin_id))
    }

    fn insert<T: Send + Sync + 'static>(
        &self,
        service: Arc<T>,
        required_permission: Option<&'static str>,
        provided_by: Option<&'static str>,
    ) -> Result<()> {
        let mut inner = self.inner.lock().expect("service registry");
        let id = TypeId::of::<T>();
        if inner.singles.contains_key(&id) {
            return Err(KernelError::ServiceConflict);
        }
        inner.singles.insert(id, ServiceEntry { service, required_permission, provided_by });
        Ok(())
    }

    /// Host / 装载完成后的无门闩查询。
    pub fn require<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.lookup::<T>(None)
    }

    pub(crate) fn require_for<T: Send + Sync + 'static>(
        &self,
        plugin_id: &'static str,
        permissions: &'static [&'static str],
        requires: &'static [&'static str],
    ) -> Result<Arc<T>> {
        self.lookup::<T>(Some(Caller { plugin_id, permissions, requires }))
    }

    fn lookup<T: Send + Sync + 'static>(&self, caller: Option<Caller<'_>>) -> Result<Arc<T>> {
        let inner = self.inner.lock().expect("service registry");
        let entry = inner.singles.get(&TypeId::of::<T>()).ok_or(KernelError::ServiceMissing)?;
        if let Some(caller) = caller {
            if let Some(perm) = entry.required_permission
                && !caller.permissions.contains(&perm)
            {
                return Err(KernelError::PermissionDenied(perm));
            }
            if let Some(provider) = entry.provided_by
                && provider != caller.plugin_id
                && !caller.requires.contains(&provider)
            {
                return Err(KernelError::MissingDependency(
                    caller.plugin_id.to_string(),
                    provider.to_string(),
                ));
            }
        }
        entry.service.clone().downcast().map_err(|_| KernelError::ServiceMissing)
    }

    pub fn revoke<T: Send + Sync + 'static>(&self) -> bool {
        self.inner.lock().expect("service registry").singles.remove(&TypeId::of::<T>()).is_some()
    }

    pub fn clear(&self) {
        self.inner.lock().expect("service registry").singles.clear();
    }
}

struct Caller<'a> {
    plugin_id: &'static str,
    permissions: &'static [&'static str],
    requires: &'a [&'static str],
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ping(u8);
    struct Vault;

    #[test]
    fn singleton_conflict_fails() {
        let reg = ServiceRegistry::new();
        reg.provide(Arc::new(Ping(1))).unwrap();
        assert!(matches!(reg.provide(Arc::new(Ping(2))), Err(KernelError::ServiceConflict)));
        assert_eq!(reg.require::<Ping>().unwrap().0, 1);
    }

    #[test]
    fn gated_service_requires_declared_permission() {
        let reg = ServiceRegistry::new();
        reg.provide_gated(Arc::new(Vault), "credential.account").unwrap();
        assert!(matches!(
            reg.require_for::<Vault>("asterism.actions", &["content.read"], &[]),
            Err(KernelError::PermissionDenied("credential.account"))
        ));
        assert!(reg.require_for::<Vault>("asterism.sync", &["credential.account"], &[]).is_ok());
    }

    #[test]
    fn plugin_service_requires_manifest_dependency() {
        let reg = ServiceRegistry::new();
        reg.provide_from(Arc::new(Ping(3)), "asterism.sync").unwrap();
        assert!(matches!(
            reg.require_for::<Ping>("asterism.actions", &[], &[]),
            Err(KernelError::MissingDependency(_, _))
        ));
        assert_eq!(
            reg.require_for::<Ping>("asterism.actions", &[], &["asterism.sync"]).unwrap().0,
            3
        );
    }
}
