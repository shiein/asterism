use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;

use crate::{AudioSource, MediaError, VideoFrame};

#[repr(C)]
struct AsterismMacRecorder {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn asterism_macos_screen_access_ok() -> c_int;
    fn asterism_macos_request_screen_access() -> c_int;
    fn asterism_macos_mic_access_ok() -> c_int;
    fn asterism_macos_request_mic_access();
    fn asterism_macos_recorder_start(
        output_path: *const c_char,
        width: c_int,
        height: c_int,
        fps: c_int,
        audio_mode: c_int,
        err: *mut c_char,
        errlen: c_int,
    ) -> *mut AsterismMacRecorder;
    fn asterism_macos_recorder_push_bgra(
        rec: *mut AsterismMacRecorder,
        bgra: *const u8,
        width: c_int,
        height: c_int,
        pts_us: i64,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
    fn asterism_macos_recorder_finish(
        rec: *mut AsterismMacRecorder,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
}

pub fn screen_access_ok() -> bool {
    unsafe { asterism_macos_screen_access_ok() == 1 }
}

pub fn request_screen_access() -> bool {
    unsafe { asterism_macos_request_screen_access() == 1 }
}

pub fn mic_access_ok() -> bool {
    unsafe { asterism_macos_mic_access_ok() == 1 }
}

pub fn request_mic_access() {
    unsafe { asterism_macos_request_mic_access() }
}

fn audio_mode(src: AudioSource) -> c_int {
    match src {
        AudioSource::None => 0,
        AudioSource::Microphone => 1,
        AudioSource::System => 2,
        AudioSource::Both => 3,
    }
}

fn take_err(buf: &[c_char]) -> String {
    let c = unsafe { CStr::from_ptr(buf.as_ptr()) };
    c.to_string_lossy().into_owned()
}

/// macOS 正式路径：AVAssetWriter H.264 + AVAudioRecorder 麦克风 + ScreenCaptureKit 系统音频。
pub struct MacOsRecording {
    rec: *mut AsterismMacRecorder,
    pub path: PathBuf,
}

unsafe impl Send for MacOsRecording {}

impl MacOsRecording {
    pub fn start(
        width: u32,
        height: u32,
        fps: u32,
        audio: AudioSource,
    ) -> Result<Self, MediaError> {
        if !screen_access_ok() && !request_screen_access() {
            return Err(MediaError::Failed("screen recording permission denied".into()));
        }
        if matches!(audio, AudioSource::Microphone | AudioSource::Both) && !mic_access_ok() {
            request_mic_access();
        }
        let dir = std::env::temp_dir().join(format!("asterism-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| MediaError::Failed(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = dir.join(format!("{}.mp4", uuid_lite()));
        let c_path = CString::new(path.to_string_lossy().as_ref())
            .map_err(|e| MediaError::Failed(e.to_string()))?;
        let mut err = [0 as c_char; 256];
        let rec = unsafe {
            asterism_macos_recorder_start(
                c_path.as_ptr(),
                width as c_int,
                height as c_int,
                fps as c_int,
                audio_mode(audio),
                err.as_mut_ptr(),
                err.len() as c_int,
            )
        };
        if rec.is_null() {
            return Err(MediaError::Failed(take_err(&err)));
        }
        Ok(Self { rec, path })
    }

    pub fn push(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        let mut err = [0 as c_char; 256];
        let rc = unsafe {
            asterism_macos_recorder_push_bgra(
                self.rec,
                frame.bgra.as_ptr(),
                frame.width as c_int,
                frame.height as c_int,
                frame.timestamp_us as i64,
                err.as_mut_ptr(),
                err.len() as c_int,
            )
        };
        if rc != 0 {
            return Err(MediaError::Failed(take_err(&err)));
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Vec<u8>, MediaError> {
        let rec = std::mem::replace(&mut self.rec, std::ptr::null_mut());
        let mut err = [0 as c_char; 256];
        let rc =
            unsafe { asterism_macos_recorder_finish(rec, err.as_mut_ptr(), err.len() as c_int) };
        if rc != 0 {
            return Err(MediaError::Failed(take_err(&err)));
        }
        let bytes = std::fs::read(&self.path).map_err(|e| MediaError::Failed(e.to_string()))?;
        let _ = std::fs::remove_file(&self.path);
        Ok(bytes)
    }
}

impl Drop for MacOsRecording {
    fn drop(&mut self) {
        if !self.rec.is_null() {
            let mut err = [0 as c_char; 16];
            unsafe {
                asterism_macos_recorder_finish(self.rec, err.as_mut_ptr(), err.len() as c_int);
            }
            self.rec = std::ptr::null_mut();
        }
    }
}

fn uuid_lite() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}
