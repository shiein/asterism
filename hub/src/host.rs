use asterism_domain_runtime::{DomainFoundationPlugin, HistoryPlugin};
use asterism_kernel::{BootPlan, Health, KernelManifest, MountContext, Plugin, Result};
use asterism_plugin_api::{PluginManifest, TrustTier};

use crate::routes::HubRouter;

macro_rules! hub_plugin {
    ($name:ident, $id:expr, $requires:expr, $routes:expr) => {
        pub struct $name;
        impl $name {
            pub const MANIFEST: PluginManifest = PluginManifest {
                id: $id,
                trust_tier: TrustTier::RequiredBuiltin,
                requires: $requires,
                permissions: &[],
            };
        }
        impl Plugin for $name {
            fn manifest(&self) -> KernelManifest {
                Self::MANIFEST.kernel()
            }
            fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
                ctx.require::<HubRouter>()?.contribute(Self::MANIFEST.id, $routes);
                ctx.health().set(Self::MANIFEST.id, Health::Ready);
                Ok(())
            }
        }
    };
}

hub_plugin!(AuthPlugin, "asterism.hub.auth", &["asterism.domain"], crate::routes::auth_routes);
hub_plugin!(
    DevicePlugin,
    "asterism.hub.device",
    &["asterism.hub.auth"],
    crate::routes::device_routes
);
hub_plugin!(
    HistoryApiPlugin,
    "asterism.hub.history",
    &["asterism.domain"],
    crate::routes::history_routes
);
hub_plugin!(BlobPlugin, "asterism.hub.blob", &["asterism.hub.history"], crate::routes::blob_routes);
hub_plugin!(RelayPlugin, "asterism.hub.relay", &["asterism.hub.auth"], crate::routes::relay_routes);
hub_plugin!(WebPlugin, "asterism.hub.web", &["asterism.domain"], crate::routes::web_routes);
hub_plugin!(
    MaintenancePlugin,
    "asterism.hub.maintenance",
    &["asterism.domain"],
    crate::routes::maintenance_routes
);

pub fn hub_boot_plan() -> BootPlan {
    let mut plan = BootPlan::new("asterism-hub");
    plan.push(DomainFoundationPlugin);
    plan.push(HistoryPlugin);
    plan.push(AuthPlugin);
    plan.push(DevicePlugin);
    plan.push(HistoryApiPlugin);
    plan.push(BlobPlugin);
    plan.push(RelayPlugin);
    plan.push(WebPlugin);
    plan.push(MaintenancePlugin);
    plan
}
