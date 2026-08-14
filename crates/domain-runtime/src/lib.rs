//! Sealed Domain Runtime：Ingestion 是唯一 Content 提交路径。

pub mod command;
pub mod ingest;
pub mod plugins;
pub mod ports;
pub mod query;

pub use command::ContentCommandService;
pub use ingest::{Ingestion, RemoteItemBody, RemoteItemSpec};
pub use plugins::{
    CaptureApi, CapturePlugin, ClipboardPlugin, DomainFoundationPlugin, DomainRuntime, HistoryApi,
    HistoryPlugin, MediaApi, MediaPlugin, SyncPlugin,
};
pub use ports::{ContentLookup, DomainReadStore, DomainStore};
pub use query::ContentQueryService;

use asterism_kernel::BootPlan;

pub fn desktop_boot_plan() -> BootPlan {
    let mut plan = BootPlan::new("asterism-desktop");
    plan.push(DomainFoundationPlugin);
    plan.push(HistoryPlugin);
    plan.push(ClipboardPlugin);
    plan.push(SyncPlugin);
    plan.push(CapturePlugin);
    plan.push(MediaPlugin);
    plan
}

pub fn hub_boot_plan() -> BootPlan {
    let mut plan = BootPlan::new("asterism-hub");
    plan.push(DomainFoundationPlugin);
    plan.push(HistoryPlugin);
    plan.push(SyncPlugin);
    plan
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_plan_puts_domain_first() {
        let order = super::desktop_boot_plan().resolved_order().unwrap();
        assert_eq!(order[0], "asterism.domain");
        assert!(order.contains(&"asterism.clipboard"));
        assert!(order.contains(&"asterism.capture"));
    }

    #[test]
    fn hub_plan_excludes_desktop_plugins() {
        let order = super::hub_boot_plan().resolved_order().unwrap();
        assert!(!order.contains(&"asterism.clipboard"));
        assert!(!order.contains(&"asterism.capture"));
    }
}
