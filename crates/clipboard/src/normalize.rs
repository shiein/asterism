use asterism_core::content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, ItemMetadata, PayloadRef,
};
use asterism_core::id::{ContentId, DeviceId};
use asterism_core::policy::{CapturePolicy, RemotePolicy};
use asterism_crypto::local_dedup_tag;
use bytes::Bytes;

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

    pub fn into_item(self, device_id: DeviceId, created_at_ms: i64) -> ContentItem {
        match self {
            Self::Text { text, dedup_tag, flags, source_app } => {
                let bytes = Bytes::from(text.into_bytes());
                let preview = asterism_core::content::truncate_preview(
                    std::str::from_utf8(&bytes).unwrap_or_default(),
                    240,
                );
                ContentItem {
                    id: ContentId::new(),
                    origin_device_id: device_id,
                    kind: ContentKind::Text,
                    created_at_ms,
                    logical_size: bytes.len() as u64,
                    payload_size: bytes.len() as u64,
                    dedup_tag,
                    flags,
                    status: ContentStatus::Local,
                    metadata: ItemMetadata {
                        source_app,
                        mime_hint: Some("text/plain;charset=utf-8".into()),
                        text_preview: Some(preview),
                        ..ItemMetadata::default()
                    },
                    payload_ref: PayloadRef::Inline { bytes },
                    encrypted_metadata: Bytes::new(),
                }
            }
            Self::Image { png, width, height, dedup_tag, flags, source_app } => {
                let bytes = Bytes::from(png);
                ContentItem {
                    id: ContentId::new(),
                    origin_device_id: device_id,
                    kind: ContentKind::Image,
                    created_at_ms,
                    logical_size: bytes.len() as u64,
                    payload_size: bytes.len() as u64,
                    dedup_tag,
                    flags,
                    status: ContentStatus::Local,
                    metadata: ItemMetadata {
                        source_app,
                        mime_hint: Some("image/png".into()),
                        image: Some(asterism_core::content::ImageMeta {
                            width,
                            height,
                            mime: "image/png".into(),
                        }),
                        ..ItemMetadata::default()
                    },
                    payload_ref: PayloadRef::Inline { bytes },
                    encrypted_metadata: Bytes::new(),
                }
            }
            Self::Files { manifest, dedup_tag, flags, source_app, .. } => {
                let summary = manifest.summary();
                ContentItem {
                    id: ContentId::new(),
                    origin_device_id: device_id,
                    kind: ContentKind::Files,
                    created_at_ms,
                    logical_size: summary.total_logical_size,
                    payload_size: summary.total_logical_size,
                    dedup_tag,
                    flags,
                    status: ContentStatus::Local,
                    metadata: ItemMetadata {
                        source_app,
                        mime_hint: Some("application/x-asterism-files".into()),
                        files: Some(summary),
                        ..ItemMetadata::default()
                    },
                    payload_ref: PayloadRef::FileManifest { manifest_id: manifest.id },
                    encrypted_metadata: Bytes::new(),
                }
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
    let decision = decide(captured, policy);
    if decision.should_ignore() {
        tracing::info!(?decision, "clipboard ignored by policy");
        return Ok(None);
    }

    if !captured.files.is_empty() {
        // 预检可能枚举十万项，不能堵在 watcher 线程。persist 侧再做 preflight / policy。
        let fingerprint = path_list_fingerprint(&captured.files);
        return Ok(Some(NormalizedContent::Files {
            paths: captured.files.clone(),
            manifest: asterism_core::FileManifest {
                id: asterism_core::ManifestId::new(),
                root_name: "pending".into(),
                entries: Vec::new(),
                unsupported: Vec::new(),
            },
            dedup_tag: local_dedup_tag(&fingerprint),
            flags: decision.flags(),
            source_app: captured.source_app.clone(),
        }));
    }

    if let Some(image) = &captured.image {
        let png = normalize_png(image)?;
        let flags = remote_flags(
            decision.flags(),
            remote,
            ContentKind::Image,
            0,
            png.bytes.len() as u64,
        );
        let tag = local_dedup_tag(&png.bytes);
        return Ok(Some(NormalizedContent::Image {
            png: png.bytes,
            width: png.width,
            height: png.height,
            dedup_tag: tag,
            flags,
            source_app: captured.source_app.clone(),
        }));
    }

    if let Some(text) = &captured.text {
        if text.is_empty() {
            return Err(ClipboardError::Empty);
        }
        let flags = remote_flags(
            decision.flags(),
            remote,
            ContentKind::Text,
            0,
            text.len() as u64,
        );
        return Ok(Some(NormalizedContent::Text {
            dedup_tag: local_dedup_tag(text.as_bytes()),
            text: text.clone(),
            flags,
            source_app: captured.source_app.clone(),
        }));
    }

    Err(ClipboardError::Unsupported)
}

fn path_list_fingerprint(paths: &[std::path::PathBuf]) -> Vec<u8> {
    let mut buf = Vec::new();
    for path in paths {
        buf.extend_from_slice(path.to_string_lossy().as_bytes());
        buf.push(0);
    }
    buf
}

fn remote_flags(
    base: ContentFlags,
    remote: &RemotePolicy,
    kind: ContentKind,
    file_count: u64,
    logical_size: u64,
) -> ContentFlags {
    if remote.check_preflight_ext(kind, file_count, logical_size, 0).is_ok() {
        base | ContentFlags::REMOTE_ALLOWED
    } else {
        base
    }
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
