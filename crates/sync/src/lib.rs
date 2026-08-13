//! Sync Protocol 与 Transport 分离。
//!
//! ```text
//! Sync Protocol
//!      ├─ DirectTransport  (TCP + TLS)
//!      └─ HubTransport     (WSS + HTTPS blobs)
//! ```

pub mod cert;
pub mod codec;
pub mod error;
pub mod file_stream;
pub mod hub_client;
pub mod lan;
pub mod pairing;
pub mod protocol;
pub mod router;
pub mod session;
pub mod transport;

pub use cert::DeviceCert;
pub use error::SyncError;
pub use hub_client::HubClient;
pub use pairing::{PairingFinish, PairingOffer};
pub use protocol::{Envelope, MessageBody, PROTOCOL_VERSION};
pub use session::SyncSession;
pub use transport::{DirectTransport, HubTransport, Route, TransportError};
