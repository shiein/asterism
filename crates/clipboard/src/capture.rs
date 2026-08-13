use std::path::PathBuf;

use crate::error::Result;
use crate::normalize::NormalizedContent;

#[derive(Clone, Debug)]
pub struct CapturedClipboard {
    pub change_token: u64,
    pub source_app: Option<String>,
    pub formats: Vec<String>,
    pub text: Option<String>,
    pub image: Option<Vec<u8>>,
    pub files: Vec<PathBuf>,
    pub sensitive: bool,
}

pub trait ClipboardBackend: Send + Sync {
    fn change_token(&self) -> Result<u64>;
    fn read(&self) -> Result<Option<CapturedClipboard>>;
    fn write(&self, content: &NormalizedContent) -> Result<()>;
}

#[derive(Default)]
pub struct NativeClipboard;

impl ClipboardBackend for NativeClipboard {
    fn change_token(&self) -> Result<u64> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::change_token()
        }
        #[cfg(windows)]
        {
            crate::windows::change_token()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Ok(0)
        }
    }

    fn read(&self) -> Result<Option<CapturedClipboard>> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::read()
        }
        #[cfg(windows)]
        {
            crate::windows::read()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Ok(None)
        }
    }

    fn write(&self, content: &NormalizedContent) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::write(content)
        }
        #[cfg(windows)]
        {
            crate::windows::write(content)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = content;
            Err(crate::error::ClipboardError::Platform("clipboard not supported on this OS".into()))
        }
    }
}
