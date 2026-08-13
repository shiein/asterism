use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use asterism_core::id::BlobId;
use asterism_crypto::hash::blake3_bytes;

use crate::error::{Result, StorageError};
use crate::paths::blob_path;

#[derive(Clone, Debug)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(data_root: impl Into<PathBuf>) -> Result<Self> {
        let root = data_root.into();
        fs::create_dir_all(root.join("blobs"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobId> {
        let id = BlobId::from_blake3(&blake3_bytes(bytes));
        let path = blob_path(&self.root, &id);
        if path.exists() {
            return Ok(id);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(tmp, path)?;
        Ok(id)
    }

    pub fn get(&self, id: &BlobId) -> Result<Vec<u8>> {
        let path = blob_path(&self.root, id);
        let mut file =
            fs::File::open(&path).map_err(|_| StorageError::MissingBlob(id.to_string()))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    pub fn exists(&self, id: &BlobId) -> bool {
        blob_path(&self.root, id).exists()
    }

    pub fn remove_if_unused(&self, id: &BlobId) -> Result<()> {
        let path = blob_path(&self.root, id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn remove_orphans(&self, referenced: &std::collections::HashSet<String>) -> Result<u64> {
        let root = self.root.join("blobs");
        if !root.exists() {
            return Ok(0);
        }
        let mut removed = 0u64;
        visit_blob_files(&root, &mut |path, name| {
            if !referenced.contains(name) {
                let _ = fs::remove_file(path);
                removed += 1;
            }
        })?;
        Ok(removed)
    }
}

fn visit_blob_files(dir: &Path, on_file: &mut impl FnMut(&Path, &str)) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_blob_files(&path, on_file)?;
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if !name.ends_with(".tmp") {
                on_file(&path, name);
            }
        }
    }
    Ok(())
}
