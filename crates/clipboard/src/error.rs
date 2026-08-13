use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("core: {0}")]
    Core(#[from] asterism_core::CoreError),
    #[error("empty clipboard")]
    Empty,
    #[error("unsupported clipboard payload")]
    Unsupported,
    #[error("platform: {0}")]
    Platform(String),
    #[error("enumeration aborted: too many entries")]
    TooManyEntries,
    #[error("clipboard payload exceeds local size limit")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, ClipboardError>;
