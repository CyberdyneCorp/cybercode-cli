//! The direct peer transport's server-side state (`cross-session-messaging` → Direct peer
//! transport, Peer pairing and grants).

pub mod attach;
pub mod code;
pub mod grants;

pub use code::PairingCode;
pub use grants::{Grant, GrantError, Issued, NewGrant};
