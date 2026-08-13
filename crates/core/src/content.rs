use std::path::{Component, Path};

use bitflags::bitflags;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::id::{BlobId, ContentId, DeviceId, ManifestId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContentKind {
    Text,
    Image,
    Files,
    Screenshot,
    Gif,
    Video,
    /// 预留，V1 不实现。
    AiResult,
    /// 预留，V1 不实现。
    OcrResult,
}

impl ContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "TEXT",
            Self::Image => "IMAGE",
            Self::Files => "FILES",
            Self::Screenshot => "SCREENSHOT",
            Self::Gif => "GIF",
            Self::Video => "VIDEO",
            Self::AiResult => "AI_RESULT",
            Self::OcrResult => "OCR_RESULT",
        }
    }

    pub fn is_reserved(self) -> bool {
        matches!(self, Self::AiResult | Self::OcrResult)
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "TEXT" => Ok(Self::Text),
            "IMAGE" => Ok(Self::Image),
            "FILES" => Ok(Self::Files),
            "SCREENSHOT" => Ok(Self::Screenshot),
            "GIF" => Ok(Self::Gif),
            "VIDEO" => Ok(Self::Video),
            "AI_RESULT" => Ok(Self::AiResult),
            "OCR_RESULT" => Ok(Self::OcrResult),
            other => Err(CoreError::InvalidContentKind(other.to_string())),
        }
    }
}

impl std::fmt::Display for ContentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct ContentFlags: u32 {
        const SENSITIVE = 1 << 0;
        const LOCAL_ONLY = 1 << 1;
        const REMOTE_ALLOWED = 1 << 2;
        const FAVORITE = 1 << 3;
        const FROM_REMOTE = 1 << 4;
        const TRANSIENT = 1 << 5;
    }
}

/// 同步/存储状态。本地阶段主要使用 LOCAL。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContentStatus {
    Local,
    Uploading,
    SyncedToHub,
    DeliveredToPeer,
    Failed,
    Expired,
}

impl ContentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Uploading => "UPLOADING",
            Self::SyncedToHub => "SYNCED_TO_HUB",
            Self::DeliveredToPeer => "DELIVERED_TO_PEER",
            Self::Failed => "FAILED",
            Self::Expired => "EXPIRED",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "LOCAL" => Ok(Self::Local),
            "UPLOADING" => Ok(Self::Uploading),
            "SYNCED_TO_HUB" => Ok(Self::SyncedToHub),
            "DELIVERED_TO_PEER" => Ok(Self::DeliveredToPeer),
            "FAILED" => Ok(Self::Failed),
            "EXPIRED" => Ok(Self::Expired),
            other => Err(CoreError::InvalidContentKind(other.to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PayloadRef {
    Inline { bytes: Bytes },
    Blob { blob_id: BlobId },
    FileManifest { manifest_id: ManifestId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentItem {
    pub id: ContentId,
    pub origin_device_id: DeviceId,
    pub kind: ContentKind,
    pub created_at_ms: i64,
    pub logical_size: u64,
    pub payload_size: u64,
    /// HMAC(VaultKey, BLAKE3(plaintext))；本地未建 Vault 时为 BLAKE3(plaintext)。
    pub dedup_tag: [u8; 32],
    pub flags: ContentFlags,
    pub status: ContentStatus,
    pub metadata: ItemMetadata,
    pub payload_ref: PayloadRef,
    /// Hub 侧密文 metadata；本地历史使用 `metadata`。
    pub encrypted_metadata: Bytes,
}

impl ContentItem {
    pub fn is_sensitive(&self) -> bool {
        self.flags.contains(ContentFlags::SENSITIVE)
    }

    pub fn may_enter_history(&self) -> bool {
        !self.is_sensitive() && !self.flags.contains(ContentFlags::TRANSIENT)
    }

    pub fn may_sync_remote(&self) -> bool {
        self.may_enter_history()
            && self.flags.contains(ContentFlags::REMOTE_ALLOWED)
            && !self.flags.contains(ContentFlags::LOCAL_ONLY)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMetadata {
    pub source_app: Option<String>,
    pub mime_hint: Option<String>,
    /// 截断后的文本预览；敏感项必须为空。
    pub text_preview: Option<String>,
    pub image: Option<ImageMeta>,
    pub files: Option<FileManifestSummary>,
    /// 本地文件缓存相对 `cache/items/<id>`；跨设备同步时不得当作源路径。
    pub local_cache_rel: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub width: u32,
    pub height: u32,
    /// 进入 Core 后统一为 `image/png`。
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifestSummary {
    pub root_name: String,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_logical_size: u64,
    pub unsupported_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    pub id: ManifestId,
    pub root_name: String,
    pub entries: Vec<FileEntry>,
    pub unsupported: Vec<UnsupportedEntry>,
}

impl FileManifest {
    pub fn summary(&self) -> FileManifestSummary {
        let file_count =
            self.entries.iter().filter(|e| e.kind == FileEntryKind::File).count() as u64;
        let dir_count =
            self.entries.iter().filter(|e| e.kind == FileEntryKind::Directory).count() as u64;
        let total_logical_size = self.entries.iter().map(|e| e.size).sum();
        FileManifestSummary {
            root_name: self.root_name.clone(),
            file_count,
            dir_count,
            total_logical_size,
            unsupported_count: self.unsupported.len() as u64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEntryKind {
    File,
    Directory,
}

impl FileEntryKind {
    pub fn is_file(self) -> bool {
        matches!(self, Self::File)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// 相对路径，禁止绝对路径与 `..`。
    pub relative_path: String,
    pub size: u64,
    pub kind: FileEntryKind,
    pub blob_id: Option<BlobId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    Symlink,
    Junction,
    SpecialDevice,
    Unreadable,
    InvalidName,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedEntry {
    pub relative_path: String,
    pub reason: UnsupportedReason,
}

/// 拒绝绝对路径、盘符、`..` 与空路径。不跟随调用方去解析 symlink。
pub fn sanitize_relative_path(path: &str) -> Result<String> {
    let raw = path.replace('\\', "/");
    if raw.is_empty() || raw.starts_with('/') {
        return Err(CoreError::PathTraversal(path.to_string()));
    }
    // 只拒绝 Windows 盘符前缀（`C:`），文件名里的 `:`（如 `12:30 note.txt`）合法。
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(CoreError::PathTraversal(path.to_string()));
    }
    let mut parts = Vec::new();
    for component in Path::new(&raw).components() {
        match component {
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                if s == ".." || s == "." {
                    return Err(CoreError::PathTraversal(path.to_string()));
                }
                parts.push(s.into_owned());
            }
            Component::CurDir => {}
            _ => return Err(CoreError::PathTraversal(path.to_string())),
        }
    }
    if parts.is_empty() {
        return Err(CoreError::PathTraversal(path.to_string()));
    }
    Ok(parts.join("/"))
}

pub fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize_relative_path("../etc/passwd").is_err());
        assert!(sanitize_relative_path("/etc/passwd").is_err());
        assert!(sanitize_relative_path("C:\\Windows").is_err());
        assert!(sanitize_relative_path("foo/../../bar").is_err());
        assert!(sanitize_relative_path("").is_err());
    }

    #[test]
    fn sanitize_normalizes_separators() {
        assert_eq!(sanitize_relative_path("a\\b\\c").unwrap(), "a/b/c");
        assert_eq!(sanitize_relative_path("docs/readme.txt").unwrap(), "docs/readme.txt");
        assert_eq!(sanitize_relative_path("12:30 note.txt").unwrap(), "12:30 note.txt");
    }

    #[test]
    fn sensitive_item_never_syncs_or_histories() {
        let item = ContentItem {
            id: ContentId::new(),
            origin_device_id: DeviceId::new(),
            kind: ContentKind::Text,
            created_at_ms: 0,
            logical_size: 4,
            payload_size: 4,
            dedup_tag: [0; 32],
            flags: ContentFlags::SENSITIVE | ContentFlags::REMOTE_ALLOWED,
            status: ContentStatus::Local,
            metadata: ItemMetadata::default(),
            payload_ref: PayloadRef::Inline { bytes: Bytes::from_static(b"pass") },
            encrypted_metadata: Bytes::new(),
        };
        assert!(!item.may_enter_history());
        assert!(!item.may_sync_remote());
    }

    #[test]
    fn kind_roundtrip() {
        for kind in [
            ContentKind::Text,
            ContentKind::Image,
            ContentKind::Files,
            ContentKind::Screenshot,
            ContentKind::Gif,
            ContentKind::Video,
            ContentKind::AiResult,
            ContentKind::OcrResult,
        ] {
            assert_eq!(ContentKind::parse(kind.as_str()).unwrap(), kind);
        }
    }
}
