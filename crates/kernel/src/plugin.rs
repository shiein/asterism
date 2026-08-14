use std::sync::Arc;

use crate::effect::{EffectGuard, MountGuard};
use crate::error::{KernelError, Result};
use crate::health::HealthBoard;
use crate::registry::ServiceRegistry;
use crate::scope::Scope;
use crate::task::{OsThreadLease, TaskGroup};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustTier {
    SealedFoundation,
    RequiredBuiltin,
    OptionalBuiltin,
}

#[derive(Clone, Copy, Debug)]
pub struct KernelManifest {
    pub id: &'static str,
    pub requires: &'static [&'static str],
    pub permissions: &'static [&'static str],
    pub trust_tier: TrustTier,
}

/// 仅能从 `MountContext` 取出的已声明权限。插件不能自行构造。
#[derive(Clone, Copy, Debug)]
pub struct DeclaredPermissions {
    permissions: &'static [&'static str],
}

impl DeclaredPermissions {
    fn from_manifest(permissions: &'static [&'static str]) -> Self {
        Self { permissions }
    }

    pub fn as_slice(self) -> &'static [&'static str] {
        self.permissions
    }
}

pub trait Plugin: Send + Sync + 'static {
    fn manifest(&self) -> KernelManifest;
    fn id(&self) -> &'static str {
        self.manifest().id
    }
    fn requires(&self) -> &'static [&'static str] {
        self.manifest().requires
    }
    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()>;
}

pub struct MountContext<'a> {
    plugin_id: &'static str,
    manifest: KernelManifest,
    registry: &'a ServiceRegistry,
    scope: &'a Scope,
    tasks: &'a mut TaskGroup,
    health: &'a HealthBoard,
    effects: Vec<EffectGuard>,
}

impl<'a> MountContext<'a> {
    fn new(
        manifest: KernelManifest,
        registry: &'a ServiceRegistry,
        scope: &'a Scope,
        tasks: &'a mut TaskGroup,
        health: &'a HealthBoard,
    ) -> Self {
        Self {
            plugin_id: manifest.id,
            manifest,
            registry,
            scope,
            tasks,
            health,
            effects: Vec::new(),
        }
    }

    pub fn plugin_id(&self) -> &'static str {
        self.plugin_id
    }

    pub fn manifest(&self) -> KernelManifest {
        self.manifest
    }

    pub fn permissions(&self) -> &'static [&'static str] {
        self.manifest.permissions
    }

    pub fn declared_permissions(&self) -> DeclaredPermissions {
        DeclaredPermissions::from_manifest(self.manifest.permissions)
    }

    pub fn trust_tier(&self) -> TrustTier {
        self.manifest.trust_tier
    }

    pub fn scope(&self) -> &Scope {
        self.scope
    }

    pub fn tasks(&mut self) -> &mut TaskGroup {
        self.tasks
    }

    pub fn health(&self) -> &HealthBoard {
        self.health
    }

    pub fn adopt_thread(&mut self, lease: OsThreadLease) {
        self.tasks.adopt(lease);
    }

    pub fn provide<T: Send + Sync + 'static>(&mut self, service: Arc<T>) -> Result<()> {
        self.registry.provide_from(service, self.plugin_id)?;
        let registry = self.registry.clone();
        self.effects.push(EffectGuard::new(move || {
            registry.revoke::<T>();
        }));
        Ok(())
    }

    pub fn require<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.registry.require_for::<T>(
            self.plugin_id,
            self.manifest.permissions,
            self.manifest.requires,
        )
    }

    pub fn on_drop(&mut self, undo: impl FnOnce() + Send + 'static) {
        self.effects.push(EffectGuard::new(undo));
    }
}

/// 装载插件：失败则逆序撤销已登记 Service 与 effect。
/// 成功后 effect（含 Service 撤销）留在 MountGuard，销毁时统一撤销。
pub fn mount_plugin(
    plugin: &dyn Plugin,
    registry: &ServiceRegistry,
    scope: &Scope,
    tasks: &mut TaskGroup,
    health: &HealthBoard,
) -> Result<MountGuard> {
    let manifest = plugin.manifest();
    if manifest.id != plugin.id() {
        return Err(KernelError::InvalidId(format!(
            "manifest id {} != plugin.id {}",
            manifest.id,
            plugin.id()
        )));
    }
    let mut ctx = MountContext::new(manifest, registry, scope, tasks, health);
    match plugin.mount(&mut ctx) {
        Ok(()) => Ok(MountGuard::new(plugin.id(), std::mem::take(&mut ctx.effects))),
        Err(err) => {
            drop(std::mem::take(&mut ctx.effects));
            Err(KernelError::Mount(err.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Ping;
    struct Good;
    struct Boom;

    impl Plugin for Good {
        fn manifest(&self) -> KernelManifest {
            KernelManifest {
                id: "asterism.good",
                requires: &[],
                permissions: &[],
                trust_tier: TrustTier::RequiredBuiltin,
            }
        }
        fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
            ctx.provide(Arc::new(Ping))?;
            Ok(())
        }
    }

    impl Plugin for Boom {
        fn manifest(&self) -> KernelManifest {
            KernelManifest {
                id: "asterism.boom",
                requires: &[],
                permissions: &[],
                trust_tier: TrustTier::RequiredBuiltin,
            }
        }
        fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
            ctx.provide(Arc::new(Ping))?;
            Err(KernelError::Mount("later step failed".into()))
        }
    }

    #[test]
    fn successful_mount_revokes_service_when_guard_drops() {
        let registry = ServiceRegistry::new();
        let scope = crate::scope::Scope::root();
        let mut tasks = crate::task::TaskGroup::new();
        let health = crate::HealthBoard::new();
        let guard = mount_plugin(&Good, &registry, &scope, &mut tasks, &health).unwrap();
        assert!(registry.require::<Ping>().is_ok());
        drop(guard);
        assert!(registry.require::<Ping>().is_err());
    }

    #[test]
    fn failed_mount_revokes_provided_service() {
        let registry = ServiceRegistry::new();
        let scope = crate::scope::Scope::root();
        let mut tasks = crate::task::TaskGroup::new();
        let health = crate::HealthBoard::new();
        assert!(mount_plugin(&Boom, &registry, &scope, &mut tasks, &health).is_err());
        assert!(registry.require::<Ping>().is_err());
    }

    #[test]
    fn failed_mount_runs_on_drop_undo() {
        let undone = Arc::new(AtomicUsize::new(0));
        struct Partial {
            undone: Arc<AtomicUsize>,
        }
        impl Plugin for Partial {
            fn manifest(&self) -> KernelManifest {
                KernelManifest {
                    id: "asterism.partial",
                    requires: &[],
                    permissions: &[],
                    trust_tier: TrustTier::RequiredBuiltin,
                }
            }
            fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
                let undone = Arc::clone(&self.undone);
                ctx.on_drop(move || {
                    undone.fetch_add(1, Ordering::SeqCst);
                });
                Err(KernelError::Mount("later step failed".into()))
            }
        }
        let registry = ServiceRegistry::new();
        let plugin = Partial { undone: Arc::clone(&undone) };
        let scope = crate::scope::Scope::root();
        let mut tasks = crate::task::TaskGroup::new();
        let health = crate::HealthBoard::new();
        assert!(mount_plugin(&plugin, &registry, &scope, &mut tasks, &health).is_err());
        assert_eq!(undone.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn remount_after_guard_drop_succeeds() {
        let registry = ServiceRegistry::new();
        let scope = crate::scope::Scope::root();
        let mut tasks = crate::task::TaskGroup::new();
        let health = crate::HealthBoard::new();
        let first = mount_plugin(&Good, &registry, &scope, &mut tasks, &health).unwrap();
        drop(first);
        let second = mount_plugin(&Good, &registry, &scope, &mut tasks, &health).unwrap();
        assert!(registry.require::<Ping>().is_ok());
        drop(second);
        assert!(registry.require::<Ping>().is_err());
    }
}
