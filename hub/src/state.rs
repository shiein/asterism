use std::collections::HashMap;
use std::sync::Arc;

use asterism_core::id::{AccountId, DeviceId};
use asterism_sync::Envelope;
use parking_lot::Mutex;
use rusqlite::Connection;
use tokio::sync::mpsc;

use crate::config::HubConfig;

pub type Outbox = mpsc::UnboundedSender<Envelope>;

pub struct HubState {
    pub config: Arc<HubConfig>,
    pub db: Mutex<Connection>,
    pub sockets: Mutex<HashMap<DeviceId, Outbox>>,
}

impl HubState {
    pub fn new(config: HubConfig, conn: Connection) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(config),
            db: Mutex::new(conn),
            sockets: Mutex::new(HashMap::new()),
        })
    }

    pub fn register_socket(&self, device: DeviceId, tx: Outbox) {
        self.sockets.lock().insert(device, tx);
    }

    pub fn unregister_socket(&self, device: DeviceId) {
        self.sockets.lock().remove(&device);
    }

    pub fn relay(&self, account: AccountId, from: DeviceId, env: Envelope) {
        let peers = {
            let db = self.db.lock();
            list_peer_ids(&db, account, from).unwrap_or_default()
        };
        let sockets = self.sockets.lock();
        for id in peers {
            if let Some(tx) = sockets.get(&id) {
                let _ = tx.send(env.clone());
            }
        }
    }
}

fn list_peer_ids(
    conn: &Connection,
    account: AccountId,
    except: DeviceId,
) -> rusqlite::Result<Vec<DeviceId>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM devices WHERE account_id = ?1 AND revoked_at_ms IS NULL AND id != ?2",
    )?;
    let rows =
        stmt.query_map((account.as_bytes().as_slice(), except.as_bytes().as_slice()), |row| {
            let raw: Vec<u8> = row.get(0)?;
            let mut id = [0u8; 16];
            if raw.len() == 16 {
                id.copy_from_slice(&raw);
            }
            Ok(DeviceId::from_bytes(id))
        })?;
    rows.collect()
}
