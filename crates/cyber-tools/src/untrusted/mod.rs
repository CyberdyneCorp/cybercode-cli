//! Untrusted external content (`untrusted-content`): the frame the registry wraps
//! around every result with external provenance, and the provenance records of
//! managed overflow files.

pub(crate) mod frame;
pub(crate) mod overflow;

pub(crate) use cyber_core::untrusted::without_userinfo;
#[cfg(test)]
pub(crate) use frame::frame;
pub(crate) use frame::{scanned, scanned_with};
