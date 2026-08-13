//! Sync Protocol 与 Transport 分离。
//!
//! ```text
//! Sync Protocol
//!      ├─ DirectTransport  (TCP + TLS)
//!      └─ HubTransport     (WSS + HTTPS blobs)
//! ```

pub mod archive;
pub mod cert;
pub mod codec;
pub mod error;
pub mod file_stream;
pub mod hub_client;
pub mod lan;
pub mod lan_item;
pub mod pairing;
pub mod payload;
pub mod protocol;
pub mod router;
pub mod session;
pub mod transport;

pub use archive::{pack_tree, unpack_tree};
pub use cert::DeviceCert;
pub use error::SyncError;
pub use hub_client::HubClient;
pub use lan::DiscoveredPeer;
pub use pairing::{PairingFinish, PairingOffer};
pub use payload::{SyncPackage, decode_package, encode_package, pack, unpack_body, unpack_meta};
pub use protocol::{Envelope, MessageBody, PROTOCOL_VERSION};
pub use session::SyncSession;
pub use transport::{DirectTransport, HubTransport, Route, TransportError};
