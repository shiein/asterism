use thiserror::Error;

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
