use asterism_core::content::{ContentFlags, ContentKind};
use asterism_core::policy::{CapturePolicy, RemotePolicy};
use asterism_crypto::local_dedup_tag;

use crate::capture::CapturedClipboard;
use crate::error::{ClipboardError, Result};

use crate::image::normalize_png;
use crate::sensitive::decide;

#[derive(Clone, Debug)]
pub enum NormalizedContent {
    Text {
        text: String,
        dedup_tag: [u8; 32],
        flags: ContentFlags,
        source_app: Option<String>,
    },
    Image {
        png: Vec<u8>,
        width: u32,
        height: u32,
        dedup_tag: [u8; 32],
        flags: ContentFlags,
        source_app: Option<String>,
    },
    Files {
        paths: Vec<std::path::PathBuf>,
        manifest: asterism_core::content::FileManifest,
        dedup_tag: [u8; 32],
        flags: ContentFlags,
        source_app: Option<String>,
    },
}

impl NormalizedContent {
    pub fn kind(&self) -> ContentKind {
        match self {
            Self::Text { .. } => ContentKind::Text,
            Self::Image { .. } => ContentKind::Image,
            Self::Files { .. } => ContentKind::Files,
        }
    }

    pub fn dedup_tag(&self) -> [u8; 32] {
        match self {
            Self::Text { dedup_tag, .. }
            | Self::Image { dedup_tag, .. }
            | Self::Files { dedup_tag, .. } => *dedup_tag,
        }
    }

    pub fn flags(&self) -> ContentFlags {
        match self {
            Self::Text { flags, .. } | Self::Image { flags, .. } | Self::Files { flags, .. } => {
                *flags
            }
        }
    }

    #[cfg(feature = "assemble")]
    pub fn into_item(self, device_id: DeviceId, created_at_ms: i64) -> ContentItem {
        match self {
            Self::Text { text, dedup_tag, flags, source_app } => {
                let bytes = Bytes::from(text.into_bytes());
                let preview = asterism_core::content::truncate_preview(
                    std::str::from_utf8(&bytes).unwrap_or_default(),
                    240,
                );
                ContentItem::from_trusted(
                    ContentId::new(),
                    device_id,
                    ContentKind::Text,
                    created_at_ms,
                    bytes.len() as u64,
                    bytes.len() as u64,
                    dedup_tag,
                    flags,
                    ContentStatus::Local,
                    ItemMetadata {
                        source_app,
                        mime_hint: Some("text/plain;charset=utf-8".into()),
                        text_preview: Some(preview),
                        ..ItemMetadata::default()
                    },
                    PayloadRef::Inline { bytes },
                    Bytes::new(),
                )
            }
            Self::Image { png, width, height, dedup_tag, flags, source_app } => {
                let bytes = Bytes::from(png);
                ContentItem::from_trusted(
                    ContentId::new(),
                    device_id,
                    ContentKind::Image,
                    created_at_ms,
                    bytes.len() as u64,
                    bytes.len() as u64,
                    dedup_tag,
                    flags,
                    ContentStatus::Local,
                    ItemMetadata {
                        source_app,
                        mime_hint: Some("image/png".into()),
                        image: Some(asterism_core::content::ImageMeta {
                            width,
                            height,
                            mime: "image/png".into(),
                        }),
                        ..ItemMetadata::default()
                    },
                    PayloadRef::Inline { bytes },
                    Bytes::new(),
                )
            }
            Self::Files { manifest, dedup_tag, flags, source_app, .. } => {
                let summary = manifest.summary();
                ContentItem::from_trusted(
                    ContentId::new(),
                    device_id,
                    ContentKind::Files,
                    created_at_ms,
                    summary.total_logical_size,
                    summary.total_logical_size,
                    dedup_tag,
                    flags,
                    ContentStatus::Local,
                    ItemMetadata {
                        source_app,
                        mime_hint: Some("application/x-asterism-files".into()),
                        files: Some(summary),
                        ..ItemMetadata::default()
                    },
                    PayloadRef::FileManifest { manifest_id: manifest.id },
                    Bytes::new(),
                )
            }
        }
    }
}

/// 优先级：敏感标志 > 应用排除 > 文件 > 图片 > 文本。
/// HTML/RTF/私有格式在同时存在通用格式时被降级忽略。
pub fn normalize(
    captured: &CapturedClipboard,
    policy: &CapturePolicy,
    remote: &RemotePolicy,
) -> Result<Option<NormalizedContent>> {
    let _ = remote;
    let decision = decide(captured, policy);
    if decision.should_ignore() {
        tracing::info!(?decision, "clipboard ignored by policy");
        return Ok(None);
    }

    if !captured.files.is_empty() {
        // 预检可能枚举十万项，不能堵在 watcher 线程。persist 侧再做 preflight / policy。
        return Ok(Some(NormalizedContent::Files {
            paths: captured.files.clone(),
            manifest: asterism_core::FileManifest {
                id: asterism_core::ManifestId::new(),
                root_name: "pending".into(),
                entries: Vec::new(),
                unsupported: Vec::new(),
            },
            dedup_tag: files_local_dedup_tag(&captured.files),
            flags: decision.flags(),
            source_app: captured.source_app.clone(),
        }));
    }

    if let Some(image) = &captured.image {
        let png = normalize_png(image)?;
        let tag = local_dedup_tag(&png.bytes);
        return Ok(Some(NormalizedContent::Image {
            png: png.bytes,
            width: png.width,
            height: png.height,
            dedup_tag: tag,
            flags: decision.flags(),
            source_app: captured.source_app.clone(),
        }));
    }

    if let Some(text) = &captured.text {
        if text.is_empty() {
            return Err(ClipboardError::Empty);
        }
        return Ok(Some(NormalizedContent::Text {
            dedup_tag: local_dedup_tag(text.as_bytes()),
            text: text.clone(),
            flags: decision.flags(),
            source_app: captured.source_app.clone(),
        }));
    }

    Err(ClipboardError::Unsupported)
}

/// Watcher 读回系统剪贴板时用的文件去重标签。回写前必须登记同一套标签。
pub fn files_local_dedup_tag(paths: &[std::path::PathBuf]) -> [u8; 32] {
    local_dedup_tag(&path_list_fingerprint(paths))
}

fn path_list_fingerprint(paths: &[std::path::PathBuf]) -> Vec<u8> {
    let mut sorted: Vec<String> = paths.iter().map(|p| stable_path_key(p)).collect();
    sorted.sort();
    let mut buf = Vec::new();
    for path in sorted {
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
    }
    buf
}

fn stable_path_key(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedClipboard;

    #[test]
    fn prefers_files_over_text() {
        let captured = CapturedClipboard {
            change_token: 1,
            source_app: None,
            formats: vec!["public.file-url".into(), "public.utf8-plain-text".into()],
            text: Some("ignored".into()),
            image: None,
            files: vec![std::env::temp_dir()],
            sensitive: false,
        };
        let out = normalize(&captured, &CapturePolicy::default(), &RemotePolicy::default())
            .unwrap()
            .unwrap();
        assert_eq!(out.kind(), ContentKind::Files);
    }

    #[test]
    fn file_path_tag_ignores_order() {
        let a = std::path::PathBuf::from("/tmp/a");
        let b = std::path::PathBuf::from("/tmp/b");
        assert_eq!(files_local_dedup_tag(&[a.clone(), b.clone()]), files_local_dedup_tag(&[b, a]));
    }

    #[test]
    fn file_path_tag_collapses_macos_var_alias_when_target_exists() {
        let tmp = std::env::temp_dir();
        let via_tmp = tmp.join(format!("asterism-tag-{}", asterism_core::ContentId::new()));
        std::fs::write(&via_tmp, b"x").unwrap();
        let via_private = std::fs::canonicalize(&via_tmp).unwrap();
        if via_tmp != via_private {
            assert_eq!(
                files_local_dedup_tag(std::slice::from_ref(&via_tmp)),
                files_local_dedup_tag(std::slice::from_ref(&via_private))
            );
        }
        let _ = std::fs::remove_file(via_tmp);
    }

    #[test]
    fn different_change_tokens_same_text_both_normalize() {
        let make = |token| CapturedClipboard {
            change_token: token,
            source_app: None,
            formats: vec!["public.utf8-plain-text".into()],
            text: Some("same body".into()),
            image: None,
            files: vec![],
            sensitive: false,
        };
        let first = normalize(&make(1), &CapturePolicy::default(), &RemotePolicy::default())
            .unwrap()
            .unwrap();
        let second = normalize(&make(2), &CapturePolicy::default(), &RemotePolicy::default())
            .unwrap()
            .unwrap();
        assert_eq!(first.kind(), ContentKind::Text);
        assert_eq!(second.kind(), ContentKind::Text);
        assert_eq!(first.dedup_tag(), second.dedup_tag());
    }

    #[test]
    fn sensitive_short_circuits() {
        let captured = CapturedClipboard {
            change_token: 1,
            source_app: None,
            formats: vec!["org.nspasteboard.ConcealedType".into()],
            text: Some("password".into()),
            image: None,
            files: vec![],
            sensitive: true,
        };
        assert!(
            normalize(&captured, &CapturePolicy::default(), &RemotePolicy::default())
                .unwrap()
                .is_none()
        );
    }
}
