use std::sync::Arc;

use asterism_kernel::{Health, KernelManifest, MountContext, Plugin, Result};
use asterism_plugin_api::{PluginManifest, TrustTier};

/// Domain 已就绪的标记服务。Host 在 mount 前提供 Ingestion。
pub struct DomainRuntime;

/// History 查询/命令能力已就绪。
pub struct HistoryApi;

/// Capture session 可由 Host Scope fork。
pub struct CaptureApi;

/// GIF/Video 媒体 ingest 已就绪。
pub struct MediaApi;

fn ready(ctx: &mut MountContext<'_>, plugin_id: &'static str) {
    ctx.health().set(plugin_id, Health::Ready);
}

pub struct DomainFoundationPlugin;

impl DomainFoundationPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.domain",
        trust_tier: TrustTier::SealedFoundation,
        requires: &[],
        permissions: &[],
    };
}

impl Plugin for DomainFoundationPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        ctx.provide(Arc::new(DomainRuntime))?;
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}

pub struct HistoryPlugin;

impl HistoryPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.history",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.domain"],
        permissions: &["history.query", "content.favorite", "content.delete"],
    };
}

impl Plugin for HistoryPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let _ = ctx.require::<DomainRuntime>()?;
        ctx.provide(Arc::new(HistoryApi))?;
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}

pub struct ClipboardPlugin;

impl ClipboardPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.clipboard",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.domain", "asterism.sync"],
        permissions: &["clipboard.read", "clipboard.write"],
    };
}

impl Plugin for ClipboardPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}

pub struct SyncPlugin;

impl SyncPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.sync",
        trust_tier: TrustTier::RequiredBuiltin,
        requires: &["asterism.domain"],
        permissions: &["content.read"],
    };
}

impl Plugin for SyncPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}

pub struct CapturePlugin;

impl CapturePlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.capture",
        trust_tier: TrustTier::OptionalBuiltin,
        requires: &["asterism.domain"],
        permissions: &["capture.screen", "clipboard.write"],
    };
}

impl Plugin for CapturePlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let _ = ctx.require::<DomainRuntime>()?;
        ctx.provide(Arc::new(CaptureApi))?;
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}

pub struct MediaPlugin;

impl MediaPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "asterism.media",
        trust_tier: TrustTier::OptionalBuiltin,
        requires: &["asterism.domain", "asterism.capture"],
        permissions: &["capture.screen"],
    };
}

impl Plugin for MediaPlugin {
    fn manifest(&self) -> KernelManifest {
        Self::MANIFEST.kernel()
    }

    fn mount(&self, ctx: &mut MountContext<'_>) -> Result<()> {
        let _ = ctx.require::<CaptureApi>()?;
        ctx.provide(Arc::new(MediaApi))?;
        ready(ctx, Self::MANIFEST.id);
        Ok(())
    }
}
