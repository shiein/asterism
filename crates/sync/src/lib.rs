//! Sync Protocol 与 Transport 分离。
//!
//! ```text
//! Sync Protocol
//!      ├─ DirectTransport  (TCP + TLS)
//!      └─ HubTransport     (WSS + HTTPS blobs)
//! ```

pub mod protocol;
pub mod router;
pub mod transport;

pub use protocol::{Envelope, MessageBody, PROTOCOL_VERSION};
pub use transport::{DirectTransport, HubTransport, TransportError};
