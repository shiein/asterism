use thiserror::Error;

use crate::error::SyncError;
use crate::protocol::Envelope;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("not connected")]
    NotConnected,
    #[error("handshake failed")]
    Handshake,
    #[error("timeout")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

impl From<SyncError> for TransportError {
    fn from(err: SyncError) -> Self {
        match err {
            SyncError::NotConnected => Self::NotConnected,
            SyncError::Timeout => Self::Timeout,
            SyncError::Handshake(_) => Self::Handshake,
            other => Self::Failed(other.to_string()),
        }
    }
}

/// LAN：TCP + TLS。Direct 必须再次验证设备身份和证书指纹。
pub trait DirectTransport: Send + Sync {
    fn send(&self, env: &Envelope) -> Result<(), TransportError>;
}

/// Hub：WSS 控制面 + HTTPS 分块 Blob。
pub trait HubTransport: Send + Sync {
    fn send_control(&self, env: &Envelope) -> Result<(), TransportError>;
}

/// 路由降级：mDNS → Hub candidate → Direct fail → Relay。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    LanDirect,
    HubRelay,
}
