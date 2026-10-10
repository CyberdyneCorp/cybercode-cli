//! Bounded LSP transport. Process authorization and lifetime belong to its owner.
mod framing;

pub use framing::{Framed, TransportError};
