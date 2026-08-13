use crate::{MediaError, VideoFrame};

/// 硬件编码优先：Windows Media Foundation / macOS VideoToolbox。
/// 本模块定义会话状态；平台 encoder 在 cfg 模块接入。
pub struct VideoSession {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
}

impl VideoSession {
    pub fn h264_default(width: u32, height: u32) -> Self {
        Self { width, height, fps: 30, frames: 0 }
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        if frame.width != self.width || frame.height != self.height {
            return Err(MediaError::Failed("frame size changed".into()));
        }
        self.frames += 1;
        Ok(())
    }
}

#[cfg(windows)]
pub mod windows_mf {
    //! WGC → D3D11 → Media Foundation H.264
}

#[cfg(target_os = "macos")]
pub mod macos_vt {
    pub use crate::macos::MacOsRecording;
}
