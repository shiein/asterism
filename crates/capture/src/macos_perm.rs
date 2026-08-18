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

/// 把 Overlay 抬到菜单栏之上，并留在当前 Space。
/// 不能用系统 Fullscreen：那会新建桌面，产生闪屏且盖不住菜单栏。
pub fn elevate_overlay_ns_view(ns_view: *mut std::ffi::c_void) {
    if ns_view.is_null() {
        return;
    }
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    const NS_STATUS_WINDOW_LEVEL: isize = 25;
    const NS_WINDOW_ANIMATION_BEHAVIOR_NONE: isize = 2;
    const NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY: isize = 1;
    const JOIN_ALL_SPACES: usize = 1 << 0;
    const TRANSIENT: usize = 1 << 3;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !app.is_null() {
            let _: bool =
                msg_send![app, setActivationPolicy: NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY];
        }

        let view = ns_view.cast::<AnyObject>();
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return;
        }
        let _: () = msg_send![window, setLevel: NS_STATUS_WINDOW_LEVEL];
        let behavior = JOIN_ALL_SPACES | TRANSIENT | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY;
        let _: () = msg_send![window, setCollectionBehavior: behavior];
        let _: () = msg_send![window, setHidesOnDeactivate: false];
        let _: () = msg_send![window, setAnimationBehavior: NS_WINDOW_ANIMATION_BEHAVIOR_NONE];
        let _: () = msg_send![window, orderFrontRegardless];

        if !app.is_null() {
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
        }
    }
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

/// 只读检查当前进程的屏幕捕获权限，不弹出系统请求。
pub fn screen_access_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 每次截图/录屏前检查，不能只在启动时检查。
pub fn ensure_screen_access() -> Result<(), CaptureError> {
    if screen_access_granted() {
        return Ok(());
    }
    unsafe {
        if CGRequestScreenCaptureAccess() && screen_access_granted() {
            return Ok(());
        }
    }
    Err(CaptureError::PermissionDenied)
}
