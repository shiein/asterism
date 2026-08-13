use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("aead decrypt failed")]
    Decrypt,
    #[error("invalid key length")]
    InvalidKeyLength,
    #[error("invalid chunk")]
    InvalidChunk,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CryptoError>;
