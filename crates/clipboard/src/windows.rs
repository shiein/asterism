//! Windows 剪贴板：`AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE`。
//! 禁止以轮询作为主路径；`change_token` 仅作兜底与自写核对。

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CF_DIB, CF_HDROP, CF_UNICODETEXT, CloseClipboard, EmptyClipboard,
    EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    RegisterClassExW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
    WNDCLASSEXW,
};
use windows::core::{PCWSTR, w};

use crate::capture::CapturedClipboard;
use crate::error::{ClipboardError, Result};
use crate::normalize::NormalizedContent;
use crate::sensitive::{WIN_EXCLUDE_MONITOR, WIN_NO_CLOUD, WIN_NO_HISTORY};

const PNG_FORMAT: &str = "PNG";

pub fn change_token() -> Result<u64> {
    unsafe { Ok(u64::from(GetClipboardSequenceNumber())) }
}

pub fn read() -> Result<Option<CapturedClipboard>> {
    with_clipboard(|| unsafe { read_inner() })
}

pub fn write(content: &NormalizedContent) -> Result<()> {
    with_clipboard(|| unsafe { write_inner(content) })
}

/// 在隐藏消息窗口上挂 `AddClipboardFormatListener`。成功后每次更新向通道发信号。
pub fn spawn_update_signal() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("asterism-clipboard-win".into())
        .spawn(move || {
            if let Err(err) = run_listener(tx) {
                tracing::warn!(error = %err, "windows clipboard listener failed");
            }
        })
        .ok();
    rx
}

fn clipboard_owner_hwnd() -> HWND {
    LISTENER_HWND.get().copied().map(|raw| HWND(raw as *mut core::ffi::c_void)).unwrap_or_default()
}

fn with_clipboard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    unsafe {
        let mut last = None;
        for _ in 0..8 {
            match OpenClipboard(clipboard_owner_hwnd()) {
                Ok(()) => {
                    let out = f();
                    let _ = CloseClipboard();
                    return out;
                }
                Err(err) => {
                    last = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis(8));
                }
            }
        }
        Err(ClipboardError::Platform(format!("OpenClipboard failed: {:?}", last)))
    }
}

unsafe fn read_inner() -> Result<Option<CapturedClipboard>> {
    let change_token = u64::from(GetClipboardSequenceNumber());
    let formats = enum_formats();
    if formats.is_empty() {
        return Ok(None);
    }
    let sensitive = is_sensitive_clipboard(&formats);
    let source_app = clipboard_owner_app();
    let text = read_unicode_text();
    let image = read_png().or_else(read_dib);
    let files = read_hdrop();
    Ok(Some(CapturedClipboard { change_token, source_app, formats, text, image, files, sensitive }))
}

unsafe fn write_inner(content: &NormalizedContent) -> Result<()> {
    EmptyClipboard().map_err(|e| ClipboardError::Platform(e.to_string()))?;
    match content {
        NormalizedContent::Text { text, .. } => set_unicode_text(text)?,
        NormalizedContent::Image { png, .. } => set_png(png)?,
        NormalizedContent::Files { paths, .. } => set_hdrop(paths)?,
    }
    Ok(())
}

unsafe fn enum_formats() -> Vec<String> {
    let mut out = Vec::new();
    let mut fmt = 0u32;
    loop {
        fmt = EnumClipboardFormats(fmt);
        if fmt == 0 {
            break;
        }
        out.push(format_name(fmt));
    }
    out
}

unsafe fn format_name(fmt: u32) -> String {
    match fmt {
        x if x == CF_UNICODETEXT.0 => "CF_UNICODETEXT".into(),
        x if x == CF_DIB.0 => "CF_DIB".into(),
        x if x == CF_HDROP.0 => "CF_HDROP".into(),
        _ => {
            let mut buf = [0u16; 128];
            let n = windows::Win32::System::DataExchange::GetClipboardFormatNameW(fmt, &mut buf);
            if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("fmt:{fmt}") }
        }
    }
}

fn has_format(formats: &[String], name: &str) -> bool {
    formats.iter().any(|f| {
        f.eq_ignore_ascii_case(name) || f.to_ascii_lowercase().contains(&name.to_ascii_lowercase())
    })
}

unsafe fn is_sensitive_clipboard(formats: &[String]) -> bool {
    if has_format(formats, WIN_EXCLUDE_MONITOR) {
        return true;
    }
    if has_format(formats, WIN_NO_HISTORY) && dword_forbids(WIN_NO_HISTORY) {
        return true;
    }
    if has_format(formats, WIN_NO_CLOUD) && dword_forbids(WIN_NO_CLOUD) {
        return true;
    }
    false
}

/// `CanIncludeInClipboardHistory` / `CanUploadToCloudClipboard` 是 DWORD：0 禁止，1 允许。
/// 格式存在但读失败时保守当作敏感。
unsafe fn dword_forbids(format_name: &str) -> bool {
    let fmt = RegisterClipboardFormatW(PCWSTR(wide(format_name).as_ptr()));
    if fmt == 0 {
        return true;
    }
    let Ok(handle) = GetClipboardData(windows::Win32::System::DataExchange::CLIPBOARD_FORMATS(fmt))
    else {
        return true;
    };
    lock_hglobal(handle, |slice| {
        if slice.len() < 4 {
            return Some(true);
        }
        let value = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
        Some(value == 0)
    })
    .unwrap_or(true)
}

unsafe fn read_unicode_text() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
    lock_hglobal(handle, |slice| {
        let words = as_u16_slice(slice);
        let end = words.iter().position(|&c| c == 0).unwrap_or(words.len());
        let s = String::from_utf16_lossy(&words[..end]);
        if s.is_empty() { None } else { Some(s) }
    })
}

unsafe fn read_png() -> Option<Vec<u8>> {
    let fmt = RegisterClipboardFormatW(PCWSTR(wide(PNG_FORMAT).as_ptr()));
    if fmt == 0 {
        return None;
    }
    let handle =
        GetClipboardData(windows::Win32::System::DataExchange::CLIPBOARD_FORMATS(fmt)).ok()?;
    lock_hglobal(handle, |slice| Some(slice.to_vec()))
}

unsafe fn read_dib() -> Option<Vec<u8>> {
    let handle = GetClipboardData(CF_DIB).ok()?;
    lock_hglobal(handle, |slice| dib_to_png(slice).ok())
}

unsafe fn read_hdrop() -> Vec<PathBuf> {
    let Ok(handle) = GetClipboardData(CF_HDROP) else {
        return Vec::new();
    };
    let drop = HDROP(handle.0);
    let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
    let mut out = Vec::new();
    for i in 0..count {
        let needed = DragQueryFileW(drop, i, None) as usize;
        if needed == 0 {
            continue;
        }
        let mut buf = vec![0u16; needed + 1];
        let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
        buf.truncate(n);
        out.push(PathBuf::from(OsString::from_wide(&buf)));
    }
    out
}

unsafe fn set_unicode_text(text: &str) -> Result<()> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * 2;
    let mem =
        GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| ClipboardError::Platform(e.to_string()))?;
    let ptr = GlobalLock(mem);
    if ptr.is_null() {
        return Err(ClipboardError::Platform("GlobalLock text".into()));
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr as *mut u8, bytes);
    let _ = GlobalUnlock(mem);
    SetClipboardData(CF_UNICODETEXT, HANDLE(mem.0))
        .map_err(|e| ClipboardError::Platform(e.to_string()))?;
    Ok(())
}

unsafe fn set_png(png: &[u8]) -> Result<()> {
    let fmt = RegisterClipboardFormatW(PCWSTR(wide(PNG_FORMAT).as_ptr()));
    if fmt == 0 {
        return Err(ClipboardError::Platform("register PNG format".into()));
    }
    let mem = GlobalAlloc(GMEM_MOVEABLE, png.len())
        .map_err(|e| ClipboardError::Platform(e.to_string()))?;
    let ptr = GlobalLock(mem);
    if ptr.is_null() {
        return Err(ClipboardError::Platform("GlobalLock png".into()));
    }
    std::ptr::copy_nonoverlapping(png.as_ptr(), ptr as *mut u8, png.len());
    let _ = GlobalUnlock(mem);
    SetClipboardData(windows::Win32::System::DataExchange::CLIPBOARD_FORMATS(fmt), HANDLE(mem.0))
        .map_err(|e| ClipboardError::Platform(e.to_string()))?;
    Ok(())
}

unsafe fn set_hdrop(paths: &[PathBuf]) -> Result<()> {
    let mut payload: Vec<u16> = Vec::new();
    for path in paths {
        payload.extend(path.as_os_str().encode_wide());
        payload.push(0);
    }
    payload.push(0);
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        x: i32,
        y: i32,
        nc: i32,
        wide: i32,
    }
    let header =
        DropFiles { p_files: std::mem::size_of::<DropFiles>() as u32, x: 0, y: 0, nc: 0, wide: 1 };
    let bytes = std::mem::size_of::<DropFiles>() + payload.len() * 2;
    let mem =
        GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| ClipboardError::Platform(e.to_string()))?;
    let ptr = GlobalLock(mem) as *mut u8;
    if ptr.is_null() {
        return Err(ClipboardError::Platform("GlobalLock hdrop".into()));
    }
    std::ptr::copy_nonoverlapping(
        &header as *const DropFiles as *const u8,
        ptr,
        std::mem::size_of::<DropFiles>(),
    );
    std::ptr::copy_nonoverlapping(
        payload.as_ptr() as *const u8,
        ptr.add(std::mem::size_of::<DropFiles>()),
        payload.len() * 2,
    );
    let _ = GlobalUnlock(mem);
    SetClipboardData(CF_HDROP, HANDLE(mem.0))
        .map_err(|e| ClipboardError::Platform(e.to_string()))?;
    Ok(())
}

unsafe fn lock_hglobal<T>(handle: HANDLE, f: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
    let hg = HGLOBAL(handle.0);
    let ptr = GlobalLock(hg);
    if ptr.is_null() {
        return None;
    }
    let size = GlobalSize(hg);
    let slice = std::slice::from_raw_parts(ptr as *const u8, size);
    let out = f(slice);
    let _ = GlobalUnlock(hg);
    out
}

fn as_u16_slice(bytes: &[u8]) -> &[u16] {
    let len = bytes.len() / 2;
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, len) }
}

fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>> {
    if dib.len() < 40 {
        return Err(ClipboardError::Unsupported);
    }
    let width = i32::from_le_bytes(dib[4..8].try_into().map_err(|_| ClipboardError::Unsupported)?);
    let height_raw =
        i32::from_le_bytes(dib[8..12].try_into().map_err(|_| ClipboardError::Unsupported)?);
    let bit_count =
        u16::from_le_bytes(dib[14..16].try_into().map_err(|_| ClipboardError::Unsupported)?);
    let compression =
        u32::from_le_bytes(dib[16..20].try_into().map_err(|_| ClipboardError::Unsupported)?);
    if compression != 0 || width <= 0 || !(bit_count == 24 || bit_count == 32) {
        return Err(ClipboardError::Unsupported);
    }
    let top_down = height_raw < 0;
    let height = height_raw.unsigned_abs();
    const MAX_DIM: u32 = 16_384;
    const MAX_PIXELS: u32 = 64 * 1024 * 1024;
    if width as u32 > MAX_DIM
        || height > MAX_DIM
        || height.saturating_mul(width as u32) > MAX_PIXELS
    {
        return Err(ClipboardError::Unsupported);
    }
    let header =
        u32::from_le_bytes(dib[0..4].try_into().map_err(|_| ClipboardError::Unsupported)?) as usize;
    let stride = ((width as usize * bit_count as usize + 31) / 32) * 4;
    let pixels = dib.get(header..).ok_or(ClipboardError::Unsupported)?;
    let pixel_bytes = bit_count as usize / 8;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        let row_start = src_y.checked_mul(stride).ok_or(ClipboardError::Unsupported)?;
        let row = pixels.get(row_start..).ok_or(ClipboardError::Unsupported)?;
        for x in 0..width as usize {
            let i = x.checked_mul(pixel_bytes).ok_or(ClipboardError::Unsupported)?;
            let needed = i
                .checked_add(if bit_count == 32 { 4 } else { 3 })
                .ok_or(ClipboardError::Unsupported)?;
            if needed > row.len() {
                return Err(ClipboardError::Unsupported);
            }
            let dst = (y * width as usize + x) * 4;
            rgba[dst] = row[i + 2];
            rgba[dst + 1] = row[i + 1];
            rgba[dst + 2] = row[i];
            rgba[dst + 3] = if bit_count == 32 { row[i + 3] } else { 255 };
        }
    }
    let img = image::RgbaImage::from_raw(width as u32, height, rgba)
        .ok_or(ClipboardError::Unsupported)?;
    crate::image::normalize_png(&{
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        png
    })
    .map(|p| p.bytes)
}

fn clipboard_owner_app() -> Option<String> {
    asterism_platform::foreground_app()
        .identifier
        .or_else(|| asterism_platform::foreground_app().name)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

static CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();
static LISTENER_HWND: OnceLock<isize> = OnceLock::new();

fn run_listener(tx: Sender<()>) -> Result<()> {
    unsafe {
        let class = CLASS_NAME.get_or_init(|| wide("AsterismClipboardSink"));
        let instance =
            GetModuleHandleW(None).map_err(|e| ClipboardError::Platform(e.to_string()))?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class.as_ptr()),
            w!("asterism"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| ClipboardError::Platform(e.to_string()))?;
        let _ = LISTENER_HWND.set(hwnd.0 as isize);
        LISTENER.with(|slot| *slot.borrow_mut() = Some(tx));
        AddClipboardFormatListener(hwnd).map_err(|e| ClipboardError::Platform(e.to_string()))?;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

thread_local! {
    static LISTENER: std::cell::RefCell<Option<Sender<()>>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        LISTENER.with(|slot| {
            if let Some(tx) = slot.borrow().as_ref() {
                let _ = tx.send(());
            }
        });
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
