use std::path::{Path, PathBuf};

use asterism_core::id::DeviceId;
use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: DeviceId,
    pub fingerprint_hex: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    peers: Vec<TrustedPeer>,
}

#[derive(Clone, Debug)]
pub struct TrustStore {
    path: PathBuf,
    peers: Vec<TrustedPeer>,
}

impl TrustStore {
    pub fn load(config_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("trusted_peers.json");
        let peers = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let file: TrustFile = serde_json::from_slice(&bytes).map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid trusted_peers.json: {err}"),
                )
            })?;
            file.peers
        } else {
            Vec::new()
        };
        Ok(Self { path, peers })
    }

    pub fn peers(&self) -> &[TrustedPeer] {
        &self.peers
    }

    pub fn fingerprints(&self) -> Vec<[u8; 32]> {
        self.peers.iter().filter_map(|p| decode_fp(&p.fingerprint_hex)).collect()
    }

    pub fn contains_device(&self, device_id: DeviceId) -> bool {
        self.peers.iter().any(|p| p.device_id == device_id)
    }

    pub fn is_trusted(&self, device_id: DeviceId, fingerprint: [u8; 32]) -> bool {
        self.peers.iter().any(|p| {
            p.device_id == device_id
                && decode_fp(&p.fingerprint_hex).is_some_and(|fp| fp == fingerprint)
        })
    }

    pub fn is_trusted_fp(&self, fingerprint: [u8; 32]) -> bool {
        self.peers.iter().any(|p| decode_fp(&p.fingerprint_hex).is_some_and(|fp| fp == fingerprint))
    }

    pub fn add(
        &mut self,
        device_id: DeviceId,
        fingerprint_hex: String,
        name: String,
    ) -> std::io::Result<()> {
        if let Some(existing) = self.peers.iter_mut().find(|p| p.device_id == device_id) {
            existing.fingerprint_hex = fingerprint_hex;
            existing.name = name;
        } else {
            self.peers.push(TrustedPeer { device_id, fingerprint_hex, name });
        }
        self.save()
    }

    pub fn remove(&mut self, device_id: DeviceId) -> std::io::Result<bool> {
        let before = self.peers.len();
        self.peers.retain(|peer| peer.device_id != device_id);
        if self.peers.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> std::io::Result<()> {
        let file = TrustFile { peers: self.peers.clone() };
        atomic_write(&self.path, &serde_json::to_vec_pretty(&file).expect("trust json"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

fn decode_fp(hex_str: &str) -> Option<[u8; 32]> {
    let raw = hex::decode(hex_str).ok()?;
    (raw.len() == 32).then(|| {
        let mut a = [0u8; 32];
        a.copy_from_slice(&raw);
        a
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_drops_revoked_peer() {
        let root = std::env::temp_dir().join(format!("asterism-trust-{}", DeviceId::new()));
        std::fs::create_dir_all(&root).unwrap();
        let mut store = TrustStore::load(&root).unwrap();
        let id = DeviceId::new();
        store.add(id, "ab".repeat(32), "peer".into()).unwrap();
        assert!(store.contains_device(id));
        assert!(store.remove(id).unwrap());
        assert!(!store.contains_device(id));
        let reloaded = TrustStore::load(&root).unwrap();
        assert!(!reloaded.contains_device(id));
        let _ = std::fs::remove_dir_all(root);
    }
}
