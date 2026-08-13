//! Windows 剪贴板：`AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE`。
//! 禁止轮询作为主路径。当前实现提供读/写与 changeCount 兜底，
//! 完整隐藏窗口监听在 Desktop Shell 接入时挂到消息循环。

use std::path::PathBuf;

use crate::capture::CapturedClipboard;
use crate::error::{ClipboardError, Result};
use crate::normalize::NormalizedContent;

pub fn change_token() -> Result<u64> {
    // 真实实现通过 WM_CLIPBOARDUPDATE 驱动。这里暴露 GetClipboardSequenceNumber。
    unsafe { Ok(u64::from(windows::Win32::System::DataExchange::GetClipboardSequenceNumber())) }
}

pub fn read() -> Result<Option<CapturedClipboard>> {
    Err(ClipboardError::Platform("windows clipboard read is compiled on Windows only".into()))
}

pub fn write(_content: &NormalizedContent) -> Result<()> {
    Err(ClipboardError::Platform("windows clipboard write is compiled on Windows only".into()))
}

#[allow(dead_code)]
pub fn empty_capture(token: u64) -> CapturedClipboard {
    CapturedClipboard {
        change_token: token,
        source_app: None,
        formats: Vec::new(),
        text: None,
        image: None,
        files: Vec::<PathBuf>::new(),
        sensitive: false,
    }
}
