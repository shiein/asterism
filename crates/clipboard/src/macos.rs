use std::path::PathBuf;

use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString,
    NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::capture::CapturedClipboard;
use crate::error::{ClipboardError, Result};
use crate::normalize::NormalizedContent;
use crate::sensitive::MACOS_CONCEALED;

pub fn change_token() -> Result<u64> {
    let pb = NSPasteboard::generalPasteboard();
    Ok(pb.changeCount() as u64)
}

pub fn read() -> Result<Option<CapturedClipboard>> {
    let pb = NSPasteboard::generalPasteboard();
    let change_token = pb.changeCount() as u64;
    let types = pb.types().map(|arr| nsstring_list(&arr)).unwrap_or_default();
    if types.is_empty() {
        return Ok(None);
    }

    let sensitive = types.iter().any(|t| t.eq_ignore_ascii_case(MACOS_CONCEALED));
    let fg = asterism_platform::foreground_app();
    let source_app = fg.identifier.or(fg.name);

    let text = pb
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let image = read_image(&pb);
    let files = read_files(&pb);

    Ok(Some(CapturedClipboard { change_token, source_app, formats: types, text, image, files, sensitive }))
}

pub fn write(content: &NormalizedContent) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    match content {
        NormalizedContent::Text { text, .. } => {
            let s = NSString::from_str(text);
            if !pb.setString_forType(&s, unsafe { NSPasteboardTypeString }) {
                return Err(ClipboardError::Platform("failed to write text".into()));
            }
        }
        NormalizedContent::Image { png, .. } => {
            let data = NSData::with_bytes(png);
            if !pb.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG }) {
                return Err(ClipboardError::Platform("failed to write png".into()));
            }
        }
        NormalizedContent::Files { paths, .. } => {
            let objects: Vec<_> = paths
                .iter()
                .filter_map(|p| {
                    let url = NSURL::from_file_path(p)?;
                    Some(ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url))
                })
                .collect();
            if objects.is_empty() {
                return Err(ClipboardError::Empty);
            }
            let array = NSArray::from_retained_slice(&objects);
            if !pb.writeObjects(&array) {
                return Err(ClipboardError::Platform("failed to write file urls".into()));
            }
        }
    }
    Ok(())
}

fn read_image(pb: &NSPasteboard) -> Option<Vec<u8>> {
    if let Some(data) = pb.dataForType(unsafe { NSPasteboardTypePNG }) {
        return Some(data.to_vec());
    }
    pb.dataForType(unsafe { NSPasteboardTypeTIFF }).map(|data| data.to_vec())
}

fn read_files(pb: &NSPasteboard) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(items) = pb.pasteboardItems() {
        for item in items {
            let Some(s) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else {
                continue;
            };
            let Some(url) = NSURL::URLWithString(&s) else {
                continue;
            };
            if let Some(path) = url.to_file_path() {
                out.push(path);
            }
        }
    }
    out
}

fn nsstring_list(arr: &NSArray<NSString>) -> Vec<String> {
    arr.iter().map(|s| s.to_string()).collect()
}
