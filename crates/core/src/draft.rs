use serde::{Deserialize, Serialize};

use crate::content::ContentKind;
use crate::id::{ContentId, DeviceId};

/// 已提交内容的受限句柄。插件凭 Grant 使用，不能还原出密钥或 Store。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentHandle {
    id: ContentId,
}

impl ContentHandle {
    pub fn from_id(id: ContentId) -> Self {
        Self { id }
    }

    pub fn id(self) -> ContentId {
        self.id
    }
}

/// 捕获来源。由 Draft 携带，正式 flags 仍由 Ingestion 生成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    pub producer_plugin_id: String,
    pub source_device_id: DeviceId,
    pub source_event_id: String,
    pub change_token: Option<u64>,
    pub parent_content_id: Option<ContentId>,
}

/// 不可信输入。最终 flags / dedup tag / ContentId 由 sealed Ingestion 生成。
#[derive(Clone, Debug)]
pub struct ContentDraft {
    pub producer_plugin_id: String,
    pub source_device_id: DeviceId,
    pub source_event_id: String,
    pub change_token: Option<u64>,
    pub kind_hint: ContentKind,
    pub source_app: Option<String>,
    pub parent_content_id: Option<ContentId>,
    pub kind_override: Option<ContentKind>,
    pub mime_hint: Option<String>,
}

impl ContentDraft {
    pub fn provenance(&self) -> Provenance {
        Provenance {
            producer_plugin_id: self.producer_plugin_id.clone(),
            source_device_id: self.source_device_id,
            source_event_id: self.source_event_id.clone(),
            change_token: self.change_token,
            parent_content_id: self.parent_content_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DedupDecision {
    /// 不同 change_token / 用户再次复制：必须新 ContentId。
    NewCapture,
    /// 同一 source event 的重复 OS 通知。
    SameSourceEvent,
    /// Asterism 自写回放。
    SelfWrite,
    /// Remote 相同 ContentId / event / sequence 重放。
    RemoteReplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestionOutcome {
    Committed(ContentId),
    Ignored(DedupDecision),
    RejectedPolicy,
}

/// Stage 0 冻结的文件 worker 现网去抖窗口。
pub const FILE_WORKER_DEBOUNCE_MS: u64 = 1500;

/// 现网行为：1.5s 内相同 payload tag 视为重复 OS 通知。
/// Stage 3/5 应改为按 source_event_id / change_token 判断。
pub fn should_skip_duplicate_payload_tag(
    last: Option<(&[u8; 32], std::time::Instant)>,
    tag: &[u8; 32],
    now: std::time::Instant,
    window: std::time::Duration,
) -> bool {
    last.is_some_and(|(prev, at)| prev == tag && now.duration_since(at) < window)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxPayload {
    pub aggregate_id: String,
    pub origin_device_id: String,
    pub kind: String,
    pub from_remote: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_user_copy_is_new_capture_not_content_merge() {
        assert_ne!(DedupDecision::NewCapture, DedupDecision::SameSourceEvent);
        assert_ne!(DedupDecision::NewCapture, DedupDecision::SelfWrite);
        assert_ne!(DedupDecision::NewCapture, DedupDecision::RemoteReplay);
        assert_ne!(DedupDecision::SameSourceEvent, DedupDecision::SelfWrite);
    }

    #[test]
    fn handle_and_provenance_round_trip() {
        let id = ContentId::new();
        assert_eq!(ContentHandle::from_id(id).id(), id);
        let draft = ContentDraft {
            producer_plugin_id: "asterism.clipboard".into(),
            source_device_id: DeviceId::new(),
            source_event_id: "7".into(),
            change_token: Some(7),
            kind_hint: ContentKind::Text,
            source_app: None,
            parent_content_id: None,
            kind_override: None,
            mime_hint: None,
        };
        let provenance = draft.provenance();
        assert_eq!(provenance.source_event_id, "7");
        assert_eq!(provenance.change_token, Some(7));
    }

    #[test]
    fn file_worker_current_debounce_is_tag_and_window() {
        let tag = [3u8; 32];
        let t0 = std::time::Instant::now();
        let window = std::time::Duration::from_millis(FILE_WORKER_DEBOUNCE_MS);
        assert!(should_skip_duplicate_payload_tag(Some((&tag, t0)), &tag, t0, window));
        assert!(!should_skip_duplicate_payload_tag(Some((&tag, t0)), &[4u8; 32], t0, window));
    }
}
