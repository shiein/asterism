use std::path::{Path, PathBuf};

use asterism_core::id::{BlobId, ContentId};

pub fn blob_path(root: &Path, id: &BlobId) -> PathBuf {
    root.join("blobs").join(id.shard_dir()).join(id.as_str())
}

pub fn item_cache_dir(cache_root: &Path, id: ContentId) -> PathBuf {
    cache_root.join("items").join(id.to_string())
}

pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("asterism.db")
}
