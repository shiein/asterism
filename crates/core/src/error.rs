use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid content kind: {0}")]
    InvalidContentKind(String),
    #[error("invalid device platform: {0}")]
    InvalidPlatform(String),
    #[error("invalid uuid")]
    InvalidUuid,
    #[error("invalid hex: {0}")]
    InvalidHex(String),
    #[error("path traversal rejected: {0}")]
    PathTraversal(String),
    #[error("policy rejected: {0}")]
    PolicyRejected(&'static str),
    #[error("unsupported reserved kind: {0}")]
    ReservedKind(&'static str),
}

pub type Result<T> = std::result::Result<T, CoreError>;
