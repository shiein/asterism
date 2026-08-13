//! Capture 层共用 FrameStream；Encoder 分离。
//!
//! ```text
//! CaptureBackend → FrameStream → Snapshot | GIF | Video | Future Remote Desktop
//! ```
//!
//! Transport 预留三类：Message / File / RealtimeMedia。V1 不实现实时媒体。

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

pub trait GifEncoder: Send {
    fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError>;
    fn finish(self: Box<Self>) -> Result<Vec<u8>, MediaError>;
}

pub trait VideoEncoder: Send {
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError>;
    fn finish(self: Box<Self>) -> Result<Vec<u8>, MediaError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSource {
    System,
    Microphone,
    Both,
    None,
}

/// 音频设备热切换：不能让 Recording Session crash。
pub trait AudioRecovery: Send {
    fn on_device_invalidated(&mut self) -> Result<(), MediaError>;
}

/// 未来远程桌面不得复用剪贴板 WebSocket 发 PNG 帧。
pub enum TransportClass {
    Message,
    File,
    RealtimeMedia,
}
