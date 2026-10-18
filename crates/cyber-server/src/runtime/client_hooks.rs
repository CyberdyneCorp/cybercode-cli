//! Hook handlers registered by connected clients (`client-sdk` → Hook callbacks). The
//! tool host merges them after every configured handler; the server forwards each
//! matching event over the registering client's JSON-RPC channel as `hook/execute`.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// One live client registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHook {
    /// Server-issued registration ID (`hkr_…`).
    pub id: String,
    pub event: String,
    /// The configured matcher syntax: a glob, `*`, or `/regex/`.
    pub matcher: Option<String>,
    /// Seconds the client may take to answer, 1 through 600.
    pub timeout: u32,
}

/// The registrations of connected clients and the channel that runs them.
pub trait ClientHooks: Send + Sync {
    /// The live registrations, in registration order, of the clients that own any of
    /// `sessions` (a Session followed by its ancestors): a client owns the Sessions it
    /// created over its channel or attached with `v1.hook.attach`, and their descendants.
    fn client_hooks(&self, sessions: &[String]) -> Vec<ClientHook>;

    /// Ask the registering client to run `registration` with the hook `envelope`; the
    /// object it returns is a hook decision. Fails when the client disconnected, the
    /// registration is gone, `timeout` passes or `cancel` fires.
    fn execute_client_hook<'a>(
        &'a self,
        registration: &'a str,
        envelope: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Value, String>>;
}
