use asterism_core::id::{ContentId, DeviceId};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MDNS_SERVICE: &str = "_asterism._tcp.local";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol_version: u32,
    pub message_id: String,
    pub request_id: Option<String>,
    pub device_id: DeviceId,
    pub timestamp_ms: i64,
    pub body: MessageBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type")]
pub enum MessageBody {
    Hello(Hello),
    DeviceState(DeviceState),
    ItemOffer(ItemOffer),
    ItemReady(ItemReady),
    ItemAck(ItemAck),
    ItemDelete(ItemDelete),
    LanCandidates(LanCandidates),
    SyncCursor(SyncCursor),
    Ping { nonce: u64 },
    Pong { nonce: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub identity_public_key: Vec<u8>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceState {
    pub device_id: DeviceId,
    pub last_seen_at_ms: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemOffer {
    pub item_id: ContentId,
    pub kind: String,
    pub logical_size: u64,
    pub payload_size: u64,
    pub dedup_tag: Vec<u8>,
    pub flags: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemReady {
    pub item_id: ContentId,
    pub blob_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemAck {
    pub item_id: ContentId,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemDelete {
    pub item_id: ContentId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanCandidates {
    pub endpoints: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncCursor {
    pub scope: String,
    pub cursor: String,
}

impl Envelope {
    pub fn new(device_id: DeviceId, body: MessageBody) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: asterism_core::ContentId::new().to_string(),
            request_id: None,
            device_id,
            timestamp_ms: now_ms(),
            body,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_json() {
        let env = Envelope::new(DeviceId::new(), MessageBody::Ping { nonce: 7 });
        let bytes = serde_json::to_vec(&env).unwrap();
        let back: Envelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.protocol_version, PROTOCOL_VERSION);
    }
}
