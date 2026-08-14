use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::ForegroundApp;

/// Clipboard Owner / 前台进程推断。Best Effort，不能识别应用内部模式。
pub fn foreground_app() -> ForegroundApp {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == HWND::default() {
            return ForegroundApp::default();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return ForegroundApp::default();
        }
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid)
        else {
            return ForegroundApp::default();
        };
        let mut buf = [0u16; MAX_PATH as usize];
        let n = K32GetModuleFileNameExW(Some(proc), None, &mut buf);
        let _ = CloseHandle(proc);
        if n == 0 {
            return ForegroundApp::default();
        }
        let path = OsString::from_wide(&buf[..n as usize]);
        let path = Path::new(&path);
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        ForegroundApp { name: name.clone(), identifier: name }
    }
}
