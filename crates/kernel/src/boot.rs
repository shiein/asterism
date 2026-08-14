use crate::error::Result;
use crate::graph::{PluginNode, resolve_boot_order};
use crate::plugin::{Plugin, mount_plugin};
use crate::registry::ServiceRegistry;
use crate::scope::Scope;
use crate::task::TaskGroup;
use crate::{HealthBoard, MountGuard};

/// 显式启动计划。装载顺序由依赖图决定，不是源文件注册顺序。
pub struct BootPlan {
    name: &'static str,
    plugins: Vec<Box<dyn Plugin>>,
}

pub struct MountedRuntime {
    pub name: &'static str,
    pub registry: ServiceRegistry,
    pub scope: Scope,
    pub tasks: TaskGroup,
    pub health: HealthBoard,
    guards: Vec<MountGuard>,
    order: Vec<&'static str>,
}

impl BootPlan {
    pub fn new(name: &'static str) -> Self {
        Self { name, plugins: Vec::new() }
    }

    pub fn push(&mut self, plugin: impl Plugin) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn resolved_order(&self) -> Result<Vec<&'static str>> {
        let nodes: Vec<PluginNode> = self
            .plugins
            .iter()
            .map(|plugin| PluginNode { id: plugin.id(), requires: plugin.requires() })
            .collect();
        resolve_boot_order(&nodes)
    }

    pub fn mount(self) -> Result<MountedRuntime> {
        self.mount_with(ServiceRegistry::new())
    }

    /// Host 先登记 sealed Service，再按依赖图装载插件。
    /// Scope / TaskGroup / HealthBoard 在 mount 之前创建，插件可在 mount 期间使用。
    pub fn mount_with(self, registry: ServiceRegistry) -> Result<MountedRuntime> {
        let order = self.resolved_order()?;
        let mut index: std::collections::HashMap<&'static str, Box<dyn Plugin>> =
            self.plugins.into_iter().map(|plugin| (plugin.id(), plugin)).collect();
        let mut runtime = MountedRuntime {
            name: self.name,
            registry,
            scope: Scope::root(),
            tasks: TaskGroup::new(),
            health: HealthBoard::new(),
            guards: Vec::new(),
            order: order.clone(),
        };
        for id in &order {
            let plugin = index.remove(id).expect("resolved plugin");
            match mount_plugin(
                plugin.as_ref(),
                &runtime.registry,
                &runtime.scope,
                &mut runtime.tasks,
                &runtime.health,
            ) {
                Ok(guard) => runtime.guards.push(guard),
                Err(err) => {
                    drop(runtime);
                    return Err(err);
                }
            }
        }
        Ok(runtime)
    }
}

impl MountedRuntime {
    pub fn boot_order(&self) -> &[&'static str] {
        &self.order
    }

    pub fn plugin_count(&self) -> usize {
        self.guards.len()
    }
}

impl Drop for MountedRuntime {
    fn drop(&mut self) {
        self.scope.dispose();
        while let Some(guard) = self.guards.pop() {
            drop(guard);
        }
        self.registry.clear();
        let tasks = std::mem::take(&mut self.tasks);
        drop(tasks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::MountContext;
    use crate::task::OsThreadLease;
    use std::sync::Arc;

    struct Domain;
    struct Clipboard;

    impl Plugin for Domain {
        fn manifest(&self) -> crate::KernelManifest {
            crate::KernelManifest {
                id: "asterism.domain",
                requires: &[],
                permissions: &[],
                trust_tier: crate::TrustTier::SealedFoundation,
            }
        }
        fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
            assert!(!ctx.scope().is_closed());
            ctx.health().set(ctx.plugin_id(), crate::Health::Ready);
            Ok(())
        }
    }

    impl Plugin for Clipboard {
        fn manifest(&self) -> crate::KernelManifest {
            crate::KernelManifest {
                id: "asterism.clipboard",
                requires: &["asterism.domain"],
                permissions: &["clipboard.read"],
                trust_tier: crate::TrustTier::RequiredBuiltin,
            }
        }
        fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
            assert_eq!(ctx.permissions(), &["clipboard.read"]);
            Ok(())
        }
    }

    #[test]
    fn boot_plan_prints_resolved_order() {
        let mut plan = BootPlan::new("desktop");
        plan.push(Clipboard);
        plan.push(Domain);
        assert_eq!(plan.resolved_order().unwrap(), ["asterism.domain", "asterism.clipboard"]);
        let runtime = plan.mount().unwrap();
        assert_eq!(runtime.boot_order(), ["asterism.domain", "asterism.clipboard"]);
        assert_eq!(runtime.health.get("asterism.domain"), Some(crate::Health::Ready));
        assert!(!runtime.scope.is_closed());
    }

    struct ChannelWorker;

    impl Plugin for ChannelWorker {
        fn manifest(&self) -> crate::KernelManifest {
            crate::KernelManifest {
                id: "asterism.worker",
                requires: &[],
                permissions: &[],
                trust_tier: crate::TrustTier::RequiredBuiltin,
            }
        }
        fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            ctx.provide(Arc::new(tx))?;
            let thread = std::thread::spawn(move || {
                let _ = rx.recv();
            });
            ctx.adopt_thread(OsThreadLease::from_join("asterism-test-channel", thread));
            Ok(())
        }
    }

    #[test]
    fn drop_releases_registry_senders_before_joining() {
        let mut plan = BootPlan::new("shutdown");
        plan.push(ChannelWorker);
        let runtime = plan.mount().unwrap();
        drop(runtime);
    }

    struct BoomAfterWorker;

    impl Plugin for BoomAfterWorker {
        fn manifest(&self) -> crate::KernelManifest {
            crate::KernelManifest {
                id: "asterism.boom",
                requires: &["asterism.worker"],
                permissions: &[],
                trust_tier: crate::TrustTier::RequiredBuiltin,
            }
        }
        fn mount(&self, _ctx: &mut MountContext<'_>) -> Result<()> {
            Err(crate::KernelError::Mount("later plugin failed".into()))
        }
    }

    #[test]
    fn failed_mount_after_channel_worker_does_not_deadlock() {
        let mut plan = BootPlan::new("partial");
        plan.push(ChannelWorker);
        plan.push(BoomAfterWorker);
        assert!(plan.mount().is_err());
    }
}
