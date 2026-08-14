use asterism_core::content::ContentItem;
use asterism_storage::{HistoryQuery, Store};

use crate::ingest::Ingestion;

pub struct ContentQueryService<'a> {
    ingestion: &'a Ingestion,
}

impl<'a> ContentQueryService<'a> {
    pub fn new(ingestion: &'a Ingestion) -> Self {
        Self { ingestion }
    }

    pub fn history(&self, query: HistoryQuery) -> anyhow::Result<Vec<ContentItem>> {
        Ok(self.store().history(query)?)
    }

    fn store(&self) -> &Store {
        self.ingestion.store()
    }
}
