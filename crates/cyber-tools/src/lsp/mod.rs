//! Bounded LSP transport. Process authorization and lifetime belong to its owner.
mod client;
mod connection;
mod documents;
mod framing;
mod launch;
mod locations;
mod pool;
pub(crate) use documents::MAX_DOCUMENT_BYTES;
pub(crate) use locations::read_origin;

pub use client::{LspError, StdioClient};
pub use connection::{ConnectionError, Shutdown, StdioConnection};
pub use framing::{Framed, TransportError};
pub use launch::{CheckoutClaim, LaunchOptions, LocalLauncher};
pub use locations::{Locations, PoolFactory};
pub use pool::{
    AdmissionFn, AuthorizedProcess, LaunchError, LaunchFn, LaunchRequest, Pool, ResourceLease,
    ServerHandle, ServerState, ServerStatus, Settlement,
};
