//! Project coordination, separate from the per-user Session/tool runtime. An account,
//! repository URL or project role never grants authority to execute on another machine.
pub mod api;
pub mod client;
pub mod error;
pub mod ids;
pub mod model;
pub mod repository;

pub mod auth;
pub use auth::{Auth, BackendCredentials};

pub mod serve;
pub mod server;
pub mod storage;
pub use serve::{ServeConfig, TlsOptions, serve};

#[cfg(test)]
mod tests;

pub mod enrollment;

pub mod openapi;

#[cfg(test)]
#[path = "../../cyber-account/tests/support/oidc.rs"]
pub(crate) mod fixture;
