//! Bounded LSP transport. Process authorization and lifetime belong to its owner.
mod client;
mod connection;
mod framing;
mod pool;

pub use client::{LspError, StdioClient};
pub use connection::{ConnectionError, Shutdown, StdioConnection};
pub use framing::{Framed, TransportError};
pub use pool::{
    AuthorizedProcess, LaunchError, LaunchFn, LaunchRequest, Pool, ServerHandle, ServerState,
    ServerStatus, Settlement,
};
