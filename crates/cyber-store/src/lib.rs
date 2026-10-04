//! Local persistence for Cyber Code (`storage-events`, `docs/decisions/0001`).
//!
//! One SQLite database in WAL mode with `synchronous=FULL`, written by exactly one owner
//! through a bounded writer queue. Events are appended per aggregate with gapless sequence
//! numbers, and registered projectors run inside the same transaction, so a projection
//! never diverges from its events. An acknowledgement is returned only after commit.

pub mod backup;
mod error;
mod events;
mod fs_check;
mod lock;
mod migrate;
mod query;
mod store;
mod writer;

pub use error::StoreError;
pub use events::{EventRegistry, EventType, Expected, NewEvent, Projector, StoredEvent};
pub use fs_check::network_filesystem;
pub use lock::OwnershipLock;
pub use query::{QueryResult, query_readonly};
pub use store::{Committed, Durability, EventPage, Pragmas, Store, StoreOptions, Tail};

/// Default and maximum page sizes for event replay.
pub const DEFAULT_PAGE_LIMIT: u32 = 100;
pub const MAX_PAGE_LIMIT: u32 = 500;
