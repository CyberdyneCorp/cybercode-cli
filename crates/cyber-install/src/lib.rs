//! Installation and upgrade (`installation-upgrade`): the latest-version lookup, how the
//! running binary was installed, the verified self-update and the uninstall plan. The
//! release source itself is `cyber_core::version::ReleaseSource`.

pub mod archive;
pub mod fetch;
pub mod manifest;
pub mod method;
pub mod policy;
pub mod replace;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod uninstall;
pub mod upgrade;
pub mod verify;
