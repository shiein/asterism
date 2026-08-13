use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("not connected")]
    NotConnected,
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("timeout")]
    Timeout,
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;
