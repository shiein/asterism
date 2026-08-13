use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("core: {0}")]
    Core(#[from] asterism_core::CoreError),
    #[error("writer stopped")]
    WriterStopped,
    #[error("not found")]
    NotFound,
    #[error("blob missing: {0}")]
    MissingBlob(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;
