//! Capture 层共用 FrameStream；Encoder 分离。

pub mod audio;
pub mod gifenc;
pub mod video;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("encoder unavailable")]
    Unavailable,
    #[error("audio source interrupted")]
    AudioInterrupted,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub timestamp_us: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub trait FrameStream: Send {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSource {
    System,
    Microphone,
    Both,
    None,
}

/// 未来远程桌面不得复用剪贴板 WebSocket 发 PNG 帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportClass {
    Message,
    File,
    RealtimeMedia,
}
