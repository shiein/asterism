use asterism_core::content::{ContentItem, ContentKind, PayloadRef};
use asterism_core::id::ContentId;
use asterism_plugin_api::{ContentReadGrant, HistoryQueryGrant};
use asterism_storage::{HistoryQuery, Store};

use crate::ingest::Ingestion;

pub struct ContentQueryService<'a> {
    ingestion: &'a Ingestion,
}

impl<'a> ContentQueryService<'a> {
    pub fn new(ingestion: &'a Ingestion) -> Self {
        Self { ingestion }
    }

    pub fn history(
        &self,
        grant: &HistoryQueryGrant,
        query: HistoryQuery,
    ) -> anyhow::Result<Vec<ContentItem>> {
        let _ = grant;
        Ok(self.store().history(query)?)
    }

    pub fn get(&self, grant: &ContentReadGrant, id: ContentId) -> anyhow::Result<ContentItem> {
        if !grant.is_valid(id) {
            anyhow::bail!("content read grant invalid");
        }
        Ok(self.store().get(id)?)
    }

    pub fn payload_bytes(
        &self,
        grant: &ContentReadGrant,
        item: &ContentItem,
    ) -> anyhow::Result<Vec<u8>> {
        if !grant.is_valid(item.id()) {
            anyhow::bail!("content read grant invalid");
        }
        if !matches!(item.kind(), ContentKind::Files) && item.payload_size() > grant.max_bytes() {
            anyhow::bail!("content exceeds grant max_bytes");
        }
        match item.payload_ref() {
            PayloadRef::Inline { bytes } => Ok(bytes.to_vec()),
            PayloadRef::Blob { blob_id } => Ok(self.store().get_blob(blob_id)?),
            PayloadRef::FileManifest { .. } => anyhow::bail!("file payload is not inline bytes"),
        }
    }

    fn store(&self) -> &Store {
        self.ingestion.store()
    }
}
