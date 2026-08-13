use std::time::Duration;

use asterism_core::id::DeviceId;
use asterism_core::policy::RemotePolicy;

use crate::error::Result;
use crate::lan::{self, LanEndpoint};
use crate::protocol::{Envelope, ItemOffer, MessageBody};
use crate::router::select_route;
use crate::transport::Route;

/// 同步会话：先 Normalize/Dedup/Policy，再选 LAN Direct 或 Hub Relay。
pub struct SyncSession {
    pub local_device: DeviceId,
    pub policy: RemotePolicy,
    pub lan: Option<LanEndpoint>,
    pub hub_ready: bool,
}

impl SyncSession {
    pub fn route(&self) -> Option<Route> {
        select_route(self.lan.is_some(), self.hub_ready)
    }

    pub fn may_offer(&self, offer: &ItemOffer) -> Result<()> {
        let kind = asterism_core::ContentKind::parse(&offer.kind)
            .map_err(|e| crate::error::SyncError::Protocol(e.to_string()))?;
        self.policy
            .check_preflight(kind, 1, offer.logical_size)
            .map_err(|e| crate::error::SyncError::Failed(e.to_string()))
    }

    pub fn offer_envelope(&self, offer: ItemOffer) -> Envelope {
        Envelope::new(self.local_device, MessageBody::ItemOffer(offer))
    }
}

pub fn connect_timeout() -> Duration {
    lan::lan_timeout()
}
