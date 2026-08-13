use crate::backend::CaptureError;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 每次截图/录屏前检查，不能只在启动时检查。
pub fn ensure_screen_access() -> Result<(), CaptureError> {
    unsafe {
        if CGPreflightScreenCaptureAccess() {
            return Ok(());
        }
        if CGRequestScreenCaptureAccess() && CGPreflightScreenCaptureAccess() {
            return Ok(());
        }
    }
    Err(CaptureError::PermissionDenied)
}
