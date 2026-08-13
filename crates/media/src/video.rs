use crate::{MediaError, VideoFrame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordingPlan {
    pub fps: u32,
    pub frames: u32,
}

impl RecordingPlan {
    pub fn new(seconds: u32, fps: u32) -> Result<Self, MediaError> {
        let fps = fps.clamp(10, 60);
        let frames = seconds
            .max(1)
            .checked_mul(fps)
            .ok_or_else(|| MediaError::Failed("recording duration is too long".into()))?;
        Ok(Self { fps, frames })
    }

    pub fn timestamp_us(self, frame_index: u32) -> u64 {
        u64::from(frame_index) * 1_000_000 / u64::from(self.fps)
    }

    pub fn deadline(self, started: std::time::Instant, frame_index: u32) -> std::time::Instant {
        started + std::time::Duration::from_micros(self.timestamp_us(frame_index))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_plan_does_not_truncate_after_twenty_seconds() {
        let plan = RecordingPlan::new(120, 60).unwrap();
        assert_eq!(plan.frames, 7_200);
        assert_eq!(plan.timestamp_us(60), 1_000_000);
    }

    #[test]
    fn recording_plan_rejects_overflow() {
        assert!(RecordingPlan::new(u32::MAX, 60).is_err());
    }
}
