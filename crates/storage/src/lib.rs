//! 桌面与 Hub 共用的 SQLite 访问层。
//!
//! 写入全部进入单一 Writer 线程；读取使用 2–4 条独立连接。
//! WebView 不得直接访问本 crate。

pub mod blob;
pub mod cleanup;
pub mod error;
pub mod outbox;
pub mod paths;
pub mod repo;
pub mod schema;
pub mod store;

pub use blob::BlobStore;
pub use error::StorageError;
pub use outbox::{
    CONSUMER_HUB, CONSUMER_HUB_DELETE, CONSUMER_LAN, EVENT_COMMITTED, EVENT_DELETED, OutboxEvent,
};
pub use repo::HistoryQuery;
pub use store::{ContentCommitPort, Store};
