use crate::backend::CaptureError;

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
}

pub fn cursor_point() -> Option<(i32, i32)> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event);
        Some((point.x.round() as i32, point.y.round() as i32))
    }
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
