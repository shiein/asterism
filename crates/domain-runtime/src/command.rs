use asterism_core::id::ContentId;
use asterism_plugin_api::ContentCommandGrant;
use asterism_storage::Store;

use crate::ingest::Ingestion;

pub struct ContentCommandService<'a> {
    ingestion: &'a Ingestion,
}

impl<'a> ContentCommandService<'a> {
    pub fn new(ingestion: &'a Ingestion) -> Self {
        Self { ingestion }
    }

    pub fn set_favorite(&self, grant: &ContentCommandGrant, favorite: bool) -> anyhow::Result<()> {
        if !grant.favorite() {
            anyhow::bail!("favorite grant missing");
        }
        self.store().set_favorite(grant.content_id(), favorite)?;
        Ok(())
    }

    pub fn delete(&self, grant: &ContentCommandGrant) -> anyhow::Result<()> {
        if !grant.delete() {
            anyhow::bail!("delete grant missing");
        }
        self.store().delete(grant.content_id())?;
        Ok(())
    }

    pub fn get(&self, id: ContentId) -> anyhow::Result<asterism_core::ContentItem> {
        Ok(self.store().get(id)?)
    }

    fn store(&self) -> &Store {
        self.ingestion.store()
    }
}
