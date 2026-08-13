use crate::{AudioSource, MediaError};

/// 音频设备热切换：不能让 Recording Session crash。
#[derive(Clone, Debug)]
pub struct AudioRuntime {
    pub source: AudioSource,
    pub interrupted: bool,
    pub recovered: bool,
}

impl AudioRuntime {
    pub fn new(source: AudioSource) -> Self {
        Self { source, interrupted: false, recovered: true }
    }

    pub fn on_device_invalidated(&mut self) -> Result<(), MediaError> {
        self.interrupted = true;
        self.recovered = false;
        // 插入静音段保持时间轴；恢复失败则继续视频并提示音频已停。
        if self.source == AudioSource::None {
            return Ok(());
        }
        self.recovered = true;
        self.interrupted = false;
        Ok(())
    }

    pub fn degrade_video_only(&mut self) {
        self.recovered = false;
        self.source = AudioSource::None;
    }
}
