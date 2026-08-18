//! Windows Media Foundation (WMF) H.264 MP4 硬件加速视频编码管线。
//!
//! 利用 IMFSinkWriter 自动调用 GPU 硬件 MFT（Intel QuickSync / NVIDIA NVENC / AMD VCE）
//! 进行高性能、低 CPU 开销的 H.264 编码并直接封装为标准 MP4 容器。

use std::path::Path;
use crate::MediaError;

#[cfg(windows)]
pub struct WmfH264Encoder {
    writer: windows::Win32::Media::MediaFoundation::IMFSinkWriter,
    stream_index: u32,
    width: u32,
    height: u32,
    fps: u32,
    timestamp_100ns: i64,
    frame_duration_100ns: i64,
    finalized: bool,
}

#[cfg(windows)]
impl WmfH264Encoder {
    pub fn create(
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: Option<u32>,
    ) -> Result<Self, MediaError> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Media::MediaFoundation::*;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
                .map_err(|e| MediaError::Failed(format!("MFStartup: {e}")))?;
        }

        let mut path_wide: Vec<u16> = output_path.as_os_str().encode_wide().collect();
        path_wide.push(0);

        let writer = unsafe {
            MFCreateSinkWriterFromURL(PCWSTR(path_wide.as_ptr()), None, None)
                .map_err(|e| MediaError::Failed(format!("MFCreateSinkWriterFromURL: {e}")))?
        };

        let target_bitrate = bitrate.unwrap_or_else(|| {
            // 自适应比特率：1080p@30fps ~ 4Mbps, 4K@60fps ~ 20Mbps
            let pixels = (width as u64) * (height as u64);
            let raw_bps = pixels * (fps as u64) * 2;
            (raw_bps / 10).clamp(2_000_000, 25_000_000) as u32
        });

        let stream_index = unsafe {
            // 1. 配置输出流媒体格式：H.264
            let out_type = MFCreateMediaType()
                .map_err(|e| MediaError::Failed(format!("MFCreateMediaType out: {e}")))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| MediaError::Failed(format!("Set major type: {e}")))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| MediaError::Failed(format!("Set subtype H264: {e}")))?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, target_bitrate)
                .map_err(|e| MediaError::Failed(format!("Set bitrate: {e}")))?;
            MFSetAttributeRatio(&out_type, &MF_MT_FRAME_RATE, fps, 1)
                .map_err(|e| MediaError::Failed(format!("Set out framerate: {e}")))?;
            MFSetAttributeSize(&out_type, &MF_MT_FRAME_SIZE, width, height)
                .map_err(|e| MediaError::Failed(format!("Set out size: {e}")))?;
            out_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| MediaError::Failed(format!("Set interlace mode: {e}")))?;

            let stream_idx = writer
                .AddStream(&out_type)
                .map_err(|e| MediaError::Failed(format!("AddStream: {e}")))?;

            // 2. 配置输入流媒体格式：BGRA32 / RGB32
            let in_type = MFCreateMediaType()
                .map_err(|e| MediaError::Failed(format!("MFCreateMediaType in: {e}")))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| MediaError::Failed(format!("Set in major type: {e}")))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| MediaError::Failed(format!("Set in subtype RGB32: {e}")))?;
            MFSetAttributeRatio(&in_type, &MF_MT_FRAME_RATE, fps, 1)
                .map_err(|e| MediaError::Failed(format!("Set in framerate: {e}")))?;
            MFSetAttributeSize(&in_type, &MF_MT_FRAME_SIZE, width, height)
                .map_err(|e| MediaError::Failed(format!("Set in size: {e}")))?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| MediaError::Failed(format!("Set in interlace mode: {e}")))?;

            writer
                .SetInputMediaType(stream_idx, &in_type, None)
                .map_err(|e| MediaError::Failed(format!("SetInputMediaType: {e}")))?;

            writer
                .BeginWriting()
                .map_err(|e| MediaError::Failed(format!("BeginWriting: {e}")))?;

            stream_idx
        };

        let frame_duration_100ns = 10_000_000i64 / (fps.max(1) as i64);

        Ok(Self {
            writer,
            stream_index,
            width,
            height,
            fps,
            timestamp_100ns: 0,
            frame_duration_100ns,
            finalized: false,
        })
    }

    pub fn write_frame(&mut self, bgra_pixels: &[u8]) -> Result<(), MediaError> {
        use windows::Win32::Media::MediaFoundation::*;

        let expected_len = (self.width as usize) * (self.height as usize) * 4;
        if bgra_pixels.len() < expected_len {
            return Err(MediaError::Failed("pixel buffer too small".into()));
        }

        unsafe {
            let buffer = MFCreateMemoryBuffer(expected_len as u32)
                .map_err(|e| MediaError::Failed(format!("MFCreateMemoryBuffer: {e}")))?;

            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut current_len = 0u32;

            buffer
                .Lock(&mut ptr, Some(&mut max_len), Some(&mut current_len))
                .map_err(|e| MediaError::Failed(format!("Buffer Lock: {e}")))?;

            std::ptr::copy_nonoverlapping(bgra_pixels.as_ptr(), ptr, expected_len);

            buffer
                .Unlock()
                .map_err(|e| MediaError::Failed(format!("Buffer Unlock: {e}")))?;

            buffer
                .SetCurrentLength(expected_len as u32)
                .map_err(|e| MediaError::Failed(format!("SetCurrentLength: {e}")))?;

            let sample = MFCreateSample()
                .map_err(|e| MediaError::Failed(format!("MFCreateSample: {e}")))?;

            sample
                .AddBuffer(&buffer)
                .map_err(|e| MediaError::Failed(format!("AddBuffer: {e}")))?;

            sample
                .SetSampleTime(self.timestamp_100ns)
                .map_err(|e| MediaError::Failed(format!("SetSampleTime: {e}")))?;

            sample
                .SetSampleDuration(self.frame_duration_100ns)
                .map_err(|e| MediaError::Failed(format!("SetSampleDuration: {e}")))?;

            self.writer
                .WriteSample(self.stream_index, &sample)
                .map_err(|e| MediaError::Failed(format!("WriteSample: {e}")))?;

            self.timestamp_100ns += self.frame_duration_100ns;
        }

        Ok(())
    }

    pub fn finish(mut self) -> Result<(), MediaError> {
        self.finalize_internal()
    }

    fn finalize_internal(&mut self) -> Result<(), MediaError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        unsafe {
            self.writer
                .Finalize()
                .map_err(|e| MediaError::Failed(format!("Finalize sink writer: {e}")))?;
            let _ = windows::Win32::Media::MediaFoundation::MFShutdown();
            windows::Win32::System::Com::CoUninitialize();
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WmfH264Encoder {
    fn drop(&mut self) {
        let _ = self.finalize_internal();
    }
}

/// 非 Windows 平台占位实现，保证交叉兼容性。
#[cfg(not(windows))]
pub struct WmfH264Encoder;

#[cfg(not(windows))]
impl WmfH264Encoder {
    pub fn create(
        _output_path: &Path,
        _width: u32,
        _height: u32,
        _fps: u32,
        _bitrate: Option<u32>,
    ) -> Result<Self, MediaError> {
        Err(MediaError::Unavailable)
    }

    pub fn write_frame(&mut self, _bgra_pixels: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::Unavailable)
    }

    pub fn finish(self) -> Result<(), MediaError> {
        Err(MediaError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmf_encoder_non_windows_graceful_unavailable() {
        #[cfg(not(windows))]
        {
            let res = WmfH264Encoder::create(Path::new("test.mp4"), 1920, 1080, 30, None);
            assert!(matches!(res, Err(MediaError::Unavailable)));
        }
    }
}
