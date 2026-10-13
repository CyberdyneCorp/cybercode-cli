//! Request credentials of one remote server, layered in the same fixed order on every
//! platform: static `headers`, then the stored OAuth bearer, then `headers_command` output.
//! A due command run or token refresh happens in place before a call, catalog refresh or
//! idle ping, and the composed set replaces the bridge's headers at once. Before each of
//! them the stored credential is re-read, so tokens another connection rotated (or a
//! re-authorization replaced) are used without a request. An in-session refresh is bounded
//! by the server's `timeout`, and a failed one is final: it is never sent again.
//! Diagnostics never contain header values or tokens.
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap};
use serde_json::json;

use super::headers_command::{CommandFailure, HeadersCommand};
use super::oauth::{OAuthError, TokenSource};
use super::{HeaderSet, HttpFailure, McpError};

/// The settlement diagnostic of a 403 `insufficient_scope`; the scopes go to the store.
const STEP_UP: &str = "MCP server requires additional OAuth scopes";

/// Periodic `headers_command` re-runs for a connected remote server.
pub(super) struct HeaderRefresh {
    pub command: HeadersCommand,
    /// The command's last output, without the static headers.
    pub output: HeaderMap,
    pub every: Duration,
    pub timeout: Duration,
    pub last: tokio::time::Instant,
}

impl HeaderRefresh {
    fn due(&self) -> bool {
        self.last.elapsed() >= self.every
    }

    /// Run the command once before `deadline` and keep its output.
    pub async fn run(
        &mut self,
        deadline: tokio::time::Instant,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<(), CommandFailure> {
        let started = tokio::time::Instant::now();
        self.output = self.command.headers(deadline, cancel).await?;
        self.last = started;
        Ok(())
    }
}

/// A remote server's credential layers and the bridge's live header set.
pub(super) struct RemoteAuth {
    pub headers: HeaderSet,
    base: HeaderMap,
    pub command: Option<HeaderRefresh>,
    pub tokens: Option<TokenSource>,
    /// The policy client that also carries token refreshes.
    http: reqwest::Client,
    /// The bound of one in-session token refresh (the server's `timeout`).
    refresh_timeout: Duration,
    /// Why the stored credential was refused; it settles `needs_auth`.
    refused: Option<String>,
    /// Whether the access token was renewed since this connect began.
    pub renewed: bool,
    /// A header command whose termination was not acknowledged; shutdown cannot settle.
    pub command_unsettled: bool,
    /// No stored credential, no configured `client_id`, and `cyber mcp auth` recorded that
    /// the authorization server offers no client registration: a refusal settles
    /// `needs_client_registration`.
    pub unregistered: bool,
}

impl RemoteAuth {
    pub fn new(
        base: HeaderMap,
        command: Option<HeaderRefresh>,
        http: reqwest::Client,
        refresh_timeout: Duration,
    ) -> Self {
        Self {
            headers: HeaderSet::default(),
            base,
            command,
            tokens: None,
            http,
            refresh_timeout,
            refused: None,
            renewed: false,
            command_unsettled: false,
            unregistered: false,
        }
    }

    /// Static headers, then the bearer, then command output: later layers win.
    pub fn compose(&self) -> HeaderMap {
        let mut headers = self.base.clone();
        if let Some(tokens) = &self.tokens {
            headers.insert(AUTHORIZATION, tokens.authorization());
        }
        if let Some(command) = &self.command {
            for (name, value) in &command.output {
                headers.insert(name.clone(), value.clone());
            }
        }
        headers
    }

    /// Publish the composed set to the bridge.
    pub fn publish(&self) {
        self.headers.replace(self.compose());
    }

    /// Whether a refused request may be retried once after renewing the access token.
    pub fn refreshable(&self) -> bool {
        self.tokens.as_ref().is_some_and(TokenSource::refreshable)
    }

    /// Renew the access token now. A rejected refresh (for example `invalid_grant`) is
    /// recorded so the connection settles `needs_auth`; nothing is retried.
    pub async fn refresh_tokens(&mut self) -> Result<(), OAuthError> {
        let Some(tokens) = self.tokens.as_mut() else {
            return Ok(());
        };
        match tokens.refresh(&self.http).await {
            Ok(()) => {
                self.renewed = true;
                self.publish();
                Ok(())
            }
            Err(error) => {
                self.refused = Some(format!("MCP OAuth token refresh failed: {error}"));
                Err(error)
            }
        }
    }

    /// A 403 `insufficient_scope` settles `needs_auth` whatever credential layers exist,
    /// since neither a refresh nor a header command adds scope. The union of the granted
    /// and the required scopes is recorded for the next `cyber mcp auth`; a failure to
    /// record it is logged and still settles.
    pub fn step_up(&self, name: &str, failure: &HttpFailure) -> Option<String> {
        let required = failure.insufficient_scope()?;
        if let Some(Err(error)) = self.tokens.as_ref().map(|t| t.require_scopes(&required)) {
            cyber_core::log::error("mcp", &error.to_string(), json!({"server":name}));
        }
        Some(STEP_UP.into())
    }

    /// Whether an in-session refresh failed, so the connection must end and settle
    /// `needs_auth` rather than serve another request.
    pub fn failed(&self) -> bool {
        self.refused.is_some()
    }

    /// Whether the credential in use was removed by `cyber mcp logout`.
    pub fn logged_out(&self) -> bool {
        self.tokens.as_ref().is_some_and(TokenSource::removed)
    }

    /// The refusal that settles `needs_auth` instead of reconnecting. With a
    /// `headers_command` or a refresh token a refused request may only mean the credential
    /// expired, so it is loss: the reconnect renews it and settles `needs_auth` only when
    /// that connect is refused too.
    pub fn refusal(&self, refused: Option<String>) -> Option<String> {
        if self.refused.is_some() {
            return self.refused.clone();
        }
        if self.command.is_some() || self.refreshable() {
            return None;
        }
        refused
    }

    /// Run a due `headers_command` and renew a due token, then swap the bridge's headers.
    /// `authorize` rechecks approval before either effect. A failure leaves the layer due,
    /// so no later request reuses the stale credential; after a failed token refresh every
    /// later request fails without contacting the authorization server again.
    pub async fn refresh(
        &mut self,
        name: &str,
        authorize: impl FnOnce() -> Result<(), McpError>,
    ) -> Result<(), McpError> {
        if self.refused.is_some() {
            return Err(McpError::Protocol("MCP OAuth token refresh failed"));
        }
        self.sync_tokens(name).await?;
        let command = self.command.as_ref().is_some_and(HeaderRefresh::due);
        let token = self
            .tokens
            .as_ref()
            .is_some_and(|tokens| tokens.due() && tokens.refreshable());
        if !command && !token {
            return Ok(());
        }
        authorize()?;
        if command {
            self.run_command(name).await?;
        }
        if token {
            self.renew_in_session(name).await?;
        }
        self.publish();
        Ok(())
    }

    /// Adopt what the store now holds; a stored credential that fails validation or was
    /// removed (`cyber mcp logout`) is a refusal, so the connection ends and settles
    /// `needs_auth` without sending the removed token again. A store busy in another
    /// process, or a read outlasting the server's `timeout` (an OS keyring prompt), fails
    /// only this request: the credential is unknown, not refused.
    async fn sync_tokens(&mut self, name: &str) -> Result<(), McpError> {
        let Some(tokens) = self.tokens.as_mut() else {
            return Ok(());
        };
        let synced = tokio::time::timeout(self.refresh_timeout, tokens.sync()).await;
        match synced {
            Ok(Ok(changed)) => {
                if changed {
                    self.publish();
                }
                Ok(())
            }
            Ok(Err(OAuthError::Busy)) | Err(_) => {
                cyber_core::log::error(
                    "mcp",
                    "MCP OAuth credential store is unavailable",
                    json!({"server":name}),
                );
                Err(McpError::Protocol(
                    "MCP OAuth credential store is unavailable",
                ))
            }
            Ok(Err(error)) => {
                cyber_core::log::error("mcp", &error.to_string(), json!({"server":name}));
                self.refused = Some(format!("MCP OAuth credential is unusable: {error}"));
                Err(McpError::Protocol("MCP OAuth credential is unusable"))
            }
        }
    }

    /// One bounded refresh; a timeout is a failed refresh.
    async fn renew_in_session(&mut self, name: &str) -> Result<(), McpError> {
        let error = match tokio::time::timeout(self.refresh_timeout, self.refresh_tokens()).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => error.to_string(),
            Err(_) => {
                self.refused = Some("MCP OAuth token refresh timed out".into());
                "MCP OAuth token refresh timed out".into()
            }
        };
        cyber_core::log::error("mcp", &error, json!({"server":name}));
        Err(McpError::Protocol("MCP OAuth token refresh failed"))
    }

    async fn run_command(&mut self, name: &str) -> Result<(), McpError> {
        let Some(refresh) = self.command.as_mut() else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + refresh.timeout;
        let Err(failure) = refresh.run(deadline, None).await else {
            return Ok(());
        };
        self.command_unsettled |= !failure.acknowledged;
        cyber_core::log::error(
            "mcp",
            &failure.diagnostic,
            json!({"server":name,"acknowledged":failure.acknowledged}),
        );
        Err(McpError::Protocol("MCP headers_command refresh failed"))
    }
}
