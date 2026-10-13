//! Configuration-driven local MCP launch under caller-retained durable ownership.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::{McpServer, McpSettings, McpToolFilter, Resolved};
use cyber_core::trust::TrustStore;
use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig};
use serde_json::Value;

use super::headers_command::{CommandFailure, HeadersCommand};
use super::oauth::{McpCredentialStore, TokenSource};
use super::remote_auth::{HeaderRefresh, RemoteAuth};
use super::{
    ConnectionError, DiscoveredTool, HttpConnection, HttpFailure, IdleUpdate, McpError,
    RemoteTransport, ServerSelection, StderrCapture, StdioConnection, authorize_server,
};
use crate::HookCommandProcess;

pub struct LocalLauncher<'a> {
    pub resolved: &'a Resolved,
    pub trust: &'a TrustStore,
    pub location: &'a Path,
    pub home: &'a Path,
    pub temp_dir: &'a Path,
    pub helper: Option<&'a Path>,
    pub credential_env_names: &'a [String],
    /// Stored MCP OAuth credentials; `None` sends no bearer token.
    pub credentials: Option<Arc<dyn McpCredentialStore>>,
}

/// Why a launch failed, so owners can choose the durable status and retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LaunchFailure {
    #[default]
    Failed,
    /// The remote server refused authorization (HTTP 401/403). Nothing else was sent,
    /// no transport fallback ran, and reconnecting cannot succeed without credentials.
    NeedsAuth,
    /// Like [`Self::NeedsAuth`], but `cyber mcp auth` recorded that the authorization
    /// server offers no client registration and no `client_id` is configured, so
    /// authorizing first needs `oauth.client_id`.
    NeedsClientRegistration,
}

impl LaunchFailure {
    /// Waiting for the user: retrying cannot succeed.
    pub fn needs_user(self) -> bool {
        matches!(self, Self::NeedsAuth | Self::NeedsClientRegistration)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{diagnostic} (native termination acknowledged: {acknowledged})")]
pub struct LocalLaunchError {
    pub diagnostic: String,
    pub acknowledged: bool,
    pub stderr: StderrCapture,
    pub scratch: Option<PathBuf>,
    pub(super) settled: bool,
    pub(super) kind: LaunchFailure,
}

impl LocalLaunchError {
    /// True only after the owned caller settles native resources, its durable receipt and pins.
    pub fn retry_safe(&self) -> bool {
        self.settled
    }

    pub fn kind(&self) -> LaunchFailure {
        self.kind
    }

    pub(super) fn before_launch(diagnostic: impl Into<String>) -> Self {
        Self {
            kind: LaunchFailure::Failed,
            settled: false,
            diagnostic: diagnostic.into(),
            acknowledged: true,
            stderr: StderrCapture::default(),
            scratch: None,
        }
    }
}

/// The owner must persist admission before connect and retain this object/future.
/// Disposal terminates transports but preserves scratch as unknown execution evidence.
pub struct LocalServer {
    connection: Transport,
    resources: Resources,
    tools: McpToolFilter,
    shutdown_acknowledged: bool,
    authority: ServerAuthority,
    catalog: Vec<super::DiscoveredTool>,
    prompts: Vec<super::DiscoveredPrompt>,
    metadata: Value,
    aliases: super::discovery::ToolAliases,
    /// A remote server's credential layers; `None` for a local server.
    auth: Option<RemoteAuth>,
    /// The local connection environment fingerprint; `None` for a remote server.
    environment: Option<String>,
}

/// A stdio process or a remote Streamable HTTP session. The HTTP arm owns no process, so
/// it never touches process-group grace or native tree state.
enum Transport {
    Stdio(StdioConnection),
    Http(HttpConnection),
}

impl Transport {
    fn clear_changed(&mut self, change: super::ListChange) {
        match self {
            Self::Stdio(connection) => connection.clear_changed(change),
            Self::Http(connection) => connection.clear_changed(change),
        }
    }

    fn take_prompts(&mut self) -> Vec<super::DiscoveredPrompt> {
        match self {
            Self::Stdio(connection) => connection.take_prompts(),
            Self::Http(connection) => connection.take_prompts(),
        }
    }

    async fn discover_prompts(
        &mut self,
        name: &str,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredPrompt>, McpError> {
        match self {
            Self::Stdio(connection) => connection.discover_prompts(name, timeout).await,
            Self::Http(connection) => connection.discover_prompts(name, timeout).await,
        }
    }

    async fn get_prompt(
        &mut self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
        timeout: Duration,
    ) -> Result<Vec<super::PromptMessage>, McpError> {
        match self {
            Self::Stdio(connection) => connection.get_prompt(name, arguments, timeout).await,
            Self::Http(connection) => connection.get_prompt(name, arguments, timeout).await,
        }
    }

    fn metadata(&self) -> Option<&Value> {
        match self {
            Self::Stdio(connection) => connection.metadata(),
            Self::Http(connection) => connection.metadata(),
        }
    }

    fn unresolved(&self) -> bool {
        match self {
            Self::Stdio(connection) => connection.unresolved(),
            Self::Http(connection) => connection.unresolved(),
        }
    }

    fn auth_refusal(&self) -> Option<HttpFailure> {
        match self {
            Self::Stdio(_) => None,
            Self::Http(connection) => connection.auth_refusal(),
        }
    }

    async fn discover_tools(
        &mut self,
        name: &str,
        filter: &McpToolFilter,
        timeout: Duration,
    ) -> Result<Vec<DiscoveredTool>, McpError> {
        match self {
            Self::Stdio(connection) => connection.discover_tools(name, filter, timeout).await,
            Self::Http(connection) => connection.discover_tools(name, filter, timeout).await,
        }
    }

    async fn poll_idle(&mut self, timeout: Duration) -> Result<IdleUpdate, McpError> {
        match self {
            Self::Stdio(connection) => connection.poll_idle(timeout).await,
            Self::Http(connection) => connection.poll_idle(timeout).await,
        }
    }

    async fn disconnected(&mut self) -> Result<bool, McpError> {
        match self {
            Self::Stdio(connection) => connection.disconnected().await,
            Self::Http(connection) => connection.disconnected().await,
        }
    }

    async fn call_tool_with_elicitation(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<Value, McpError> {
        match self {
            Self::Stdio(connection) => {
                connection
                    .call_tool_with_elicitation(name, arguments, timeout, context)
                    .await
            }
            Self::Http(connection) => {
                connection
                    .call_tool_with_elicitation(name, arguments, timeout, context)
                    .await
            }
        }
    }

    async fn resource(
        &mut self,
        request: super::ResourceRequest<'_>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        match self {
            Self::Stdio(connection) => connection.resource(request, timeout).await,
            Self::Http(connection) => connection.resource(request, timeout).await,
        }
    }

    async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        match self {
            Self::Stdio(connection) => connection.cancel_pending(timeout).await,
            Self::Http(connection) => connection.cancel_pending(timeout).await,
        }
    }

    async fn shutdown(&mut self, graceful: bool) -> (bool, StderrCapture) {
        match (self, graceful) {
            (Self::Stdio(connection), true) => connection.shutdown_gracefully().await,
            (Self::Stdio(connection), false) => connection.shutdown().await,
            (Self::Http(connection), _) => connection.shutdown().await,
        }
    }
}

struct ServerAuthority {
    resolved: Resolved,
    trust: TrustStore,
    location: PathBuf,
    name: String,
}

impl ServerAuthority {
    fn check(&self) -> Result<(), McpError> {
        authorize_server(&self.resolved, &self.trust, &self.location, &self.name)
            .map(|_| ())
            .map_err(|_| McpError::Protocol("MCP server authorization is no longer valid"))
    }
}

impl LocalServer {
    pub fn tools(&self) -> &[super::DiscoveredTool] {
        &self.catalog
    }

    pub fn prompts(&self) -> &[super::DiscoveredPrompt] {
        &self.prompts
    }

    pub fn metadata(&self) -> &Value {
        &self.metadata
    }

    /// The fingerprint of the roots and sandbox a local process was launched with (see
    /// [`environment_fingerprint`]); `None` for a remote server.
    pub fn environment(&self) -> Option<&str> {
        self.environment.as_deref()
    }

    /// The remote transport in use, or `None` for a local server.
    pub fn remote_transport(&self) -> Option<RemoteTransport> {
        match &self.connection {
            Transport::Stdio(_) => None,
            Transport::Http(connection) => Some(connection.transport()),
        }
    }

    pub async fn refresh_tools(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.authorize()?;
        self.refresh_headers().await?;
        self.connection.clear_changed(super::ListChange::Tools);
        self.catalog.clear();
        self.catalog = self
            .connection
            .discover_tools(&self.authority.name, &self.tools, timeout)
            .await?;
        self.aliases.retain(&self.authority.name, &mut self.catalog);
        Ok(())
    }

    /// Re-list prompts after `notifications/prompts/list_changed`; a failure leaves none.
    pub async fn refresh_prompts(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.authorize()?;
        self.refresh_headers().await?;
        self.connection.clear_changed(super::ListChange::Prompts);
        self.prompts.clear();
        self.prompts = self
            .connection
            .discover_prompts(&self.authority.name, timeout)
            .await?;
        Ok(())
    }

    /// Resources hold no cache: a `resources/list_changed` notification only clears.
    pub fn clear_resources_changed(&mut self) {
        self.connection.clear_changed(super::ListChange::Resources);
    }

    /// `prompts/get` for a prompt in the connected catalog, under the same fresh
    /// authority and credential refresh as tool calls.
    pub async fn get_prompt(
        &mut self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
        timeout: Duration,
    ) -> Result<Vec<super::PromptMessage>, McpError> {
        self.authorize()?;
        self.refresh_headers().await?;
        if !self.prompts.iter().any(|prompt| prompt.name == name) {
            return Err(McpError::Protocol(
                "MCP prompt is absent from connected server catalog",
            ));
        }
        self.connection.get_prompt(name, arguments, timeout).await
    }

    pub async fn call_exposed_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.call_exposed_tool_with_elicitation(name, arguments, timeout, None)
            .await
    }

    pub async fn call_exposed_tool_with_elicitation(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<Value, McpError> {
        self.authorize()?;
        let remote = self
            .catalog
            .iter()
            .find(|tool| tool.exposed_name == name)
            .map(|tool| tool.remote_name.clone())
            .ok_or_else(|| McpError::StaleTool(name.into()))?;
        self.call_tool_with_elicitation(&remote, arguments, timeout, context)
            .await
    }

    fn authorize(&self) -> Result<(), McpError> {
        self.authority.check()
    }

    pub fn scratch_path(&self) -> &Path {
        &self.resources.scratch.path
    }

    /// The pool monitor's idle tick. A due `headers_command` re-runs and a due OAuth token
    /// renews first, so the idle ping never carries an expired credential; a failed refresh
    /// is loss, and a refused token settles `needs_auth`.
    pub async fn poll_idle(&mut self, timeout: Duration) -> Result<super::IdleUpdate, McpError> {
        self.refresh_headers().await?;
        self.connection.poll_idle(timeout).await
    }

    /// Re-run a due `headers_command`, renew a due OAuth token and swap the bridge's headers
    /// in place. Every request path calls this first, so a busy server whose monitor tick
    /// is skipped still refreshes.
    async fn refresh_headers(&mut self) -> Result<(), McpError> {
        let Some(auth) = self.auth.as_mut() else {
            return Ok(());
        };
        let authority = &self.authority;
        // Boxed so the token refresh state is not inlined into every request future.
        Box::pin(auth.refresh(&authority.name, || authority.check())).await
    }

    pub async fn disconnected(&mut self) -> Result<bool, McpError> {
        self.connection.disconnected().await
    }

    /// Whether the connection must end before another request: its transport state is
    /// unknown, or an in-session OAuth refresh failed and the server settles `needs_auth`.
    pub fn unresolved(&self) -> bool {
        self.connection.unresolved() || self.auth.as_ref().is_some_and(RemoteAuth::failed)
    }

    /// Why an established connection settles `needs_auth` instead of a retryable failure:
    /// a remote refusal (HTTP 401/403) or a failed OAuth token refresh. With a
    /// `headers_command` or an OAuth refresh token a refusal may only mean the credential
    /// expired, so it is loss: the reconnect renews it once and settles `needs_auth` only
    /// if that connect is refused too.
    ///
    /// A 403 `insufficient_scope` always settles `needs_auth` and records the required
    /// scopes for the next `cyber mcp auth`; nothing refreshes or re-authorizes.
    pub fn auth_refusal(&self) -> Option<String> {
        let failure = self.connection.auth_refusal();
        let Some(auth) = &self.auth else {
            return failure.map(|failure| failure.to_string());
        };
        if let Some(step_up) = failure
            .as_ref()
            .and_then(|failure| auth.step_up(&self.authority.name, failure))
        {
            return Some(step_up);
        }
        auth.refusal(failure.map(|failure| failure.to_string()))
    }

    /// Whether this remote connection sends an OAuth credential that `cyber mcp logout`
    /// has since removed, so an explicit reconnect may replace it.
    pub fn logged_out(&self) -> bool {
        self.auth.as_ref().is_some_and(RemoteAuth::logged_out)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.call_tool_with_elicitation(name, arguments, timeout, None)
            .await
    }

    pub async fn call_tool_with_elicitation(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<Value, McpError> {
        self.authorize()?;
        self.refresh_headers().await?;
        if !self
            .tools
            .permits(name)
            .map_err(|_| McpError::Protocol("invalid tool filter"))?
        {
            return Err(McpError::Protocol("MCP tool is excluded by server filter"));
        }
        if !self.catalog.iter().any(|tool| tool.remote_name == name) {
            return Err(McpError::Protocol(
                "MCP tool is absent from connected server catalog",
            ));
        }
        // A credential removed by `cyber mcp logout` while a form waits withdraws the
        // call's authorization, so accepted answers are never forwarded.
        let auth = self.auth.as_ref();
        let authorize =
            || !auth.is_some_and(RemoteAuth::logged_out) && context.is_some_and(|c| c.authorized());
        let context = context.map(|context| context.authorized_by(&authorize));
        self.connection
            .call_tool_with_elicitation(name, arguments, timeout, context.as_ref())
            .await
    }

    /// A resource RPC under the same fresh authority and credential refresh as tool calls.
    pub async fn resource(
        &mut self,
        request: super::ResourceRequest<'_>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        self.authorize()?;
        self.refresh_headers().await?;
        self.connection.resource(request, timeout).await
    }

    /// Best-effort `notifications/cancelled` for the unresolved request, sent only while the
    /// definition is still authorized and its credential was not removed. It never settles
    /// the request; the caller still shuts the connection down.
    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.authorize()?;
        if self.logged_out() {
            return Err(McpError::Protocol("MCP server credential was removed"));
        }
        self.connection.cancel_pending(timeout).await
    }

    /// Cleanup is authorized only after both the native tree and local proxy settle.
    pub async fn shutdown(&mut self) -> (bool, StderrCapture) {
        self.shutdown_mode(false).await
    }

    pub async fn shutdown_gracefully(&mut self) -> (bool, StderrCapture) {
        self.shutdown_mode(true).await
    }

    async fn shutdown_mode(&mut self, graceful: bool) -> (bool, StderrCapture) {
        let (native, stderr) = self.connection.shutdown(graceful).await;
        let proxy = self.resources.stop_proxy().await;
        let unsettled = self
            .auth
            .as_ref()
            .is_some_and(|auth| auth.command_unsettled);
        let acknowledged = native && proxy && !unsettled;
        self.shutdown_acknowledged = acknowledged;
        (acknowledged, stderr)
    }

    /// Commit the durable terminal owner before permitting transient scratch cleanup.
    pub fn settle_after_shutdown<T>(
        &mut self,
        persist: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if !self.shutdown_acknowledged {
            return Err("MCP native shutdown is unverified".into());
        }
        let result = persist()?;
        self.resources.scratch.cleanup = true;
        Ok(result)
    }
}

impl LocalLauncher<'_> {
    /// `before_spawn` pins/marks the caller's durable execution owner before effects.
    pub async fn connect(
        &self,
        name: &str,
        before_spawn: impl FnOnce() -> Result<(), String>,
    ) -> Result<LocalServer, LocalLaunchError> {
        self.connect_before(name, before_spawn, None, RemoteTransport::default())
            .await
    }

    /// `first` is the remote transport to try first (a reconnect reuses the last one).
    pub(super) async fn connect_before(
        &self,
        name: &str,
        before_spawn: impl FnOnce() -> Result<(), String>,
        limit: Option<tokio::time::Instant>,
        first: RemoteTransport,
    ) -> Result<LocalServer, LocalLaunchError> {
        self.connect_options(name, before_spawn, limit, false, None, first)
            .await
    }

    /// The caller owns a durable hook admission and retains this future through cancellation.
    pub async fn connect_for_hook(
        &self,
        name: &str,
        before_spawn: impl FnOnce() -> Result<(), String>,
        deadline: tokio::time::Instant,
        required_sandbox: bool,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<LocalServer, LocalLaunchError> {
        self.connect_options(
            name,
            before_spawn,
            Some(deadline),
            required_sandbox,
            Some(cancel),
            RemoteTransport::default(),
        )
        .await
    }

    async fn connect_options(
        &self,
        name: &str,
        before_spawn: impl FnOnce() -> Result<(), String>,
        limit: Option<tokio::time::Instant>,
        required_sandbox: bool,
        cancel: Option<&tokio_util::sync::CancellationToken>,
        first: RemoteTransport,
    ) -> Result<LocalServer, LocalLaunchError> {
        if cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            return Err(LocalLaunchError::before_launch("MCP startup cancelled"));
        }
        let server = authorize_server(self.resolved, self.trust, self.location, name)
            .map_err(LocalLaunchError::before_launch)?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(server.definition.timeout().into());
        let launch = Startup {
            name,
            deadline: limit.map_or(deadline, |limit| limit.min(deadline)),
            required_sandbox,
            cancel,
            first,
        };
        let connected = match &server.definition {
            McpServer::Local { .. } => self.connect_local(&server, &launch, before_spawn).await?,
            // Boxed: the OAuth and transport-fallback states would otherwise inflate every
            // caller's future (and its stack) even for local servers.
            McpServer::Remote { .. } => {
                Box::pin(self.connect_remote(&server, &launch, before_spawn)).await?
            }
        };
        Ok(self.server(name, server.definition.tools(), connected))
    }

    fn server(&self, name: &str, tools: &McpToolFilter, connected: Connected) -> LocalServer {
        let Connected {
            mut connection,
            mut catalog,
            resources,
            auth,
            environment,
        } = connected;
        let mut aliases = super::discovery::ToolAliases::default();
        aliases.retain(name, &mut catalog);
        LocalServer {
            aliases,
            prompts: connection.take_prompts(),
            metadata: connection.metadata().cloned().unwrap_or(Value::Null),
            catalog,
            connection,
            resources,
            auth,
            environment,
            tools: tools.clone(),
            shutdown_acknowledged: false,
            authority: ServerAuthority {
                resolved: self.resolved.clone(),
                trust: self.trust.clone(),
                location: self.location.into(),
                name: name.into(),
            },
        }
    }

    /// The last checks before the first native or network effect: cancellation, the
    /// caller's durable admission and fresh approval, all inside the startup deadline.
    fn admit_effect(
        &self,
        launch: &Startup<'_>,
        before_spawn: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), LocalLaunchError> {
        if launch.cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            return Err(LocalLaunchError::before_launch("MCP startup cancelled"));
        }
        before_spawn().map_err(LocalLaunchError::before_launch)?;
        // Preparation and durable admission can await or revoke approval.
        authorize_server(self.resolved, self.trust, self.location, launch.name)
            .map_err(LocalLaunchError::before_launch)?;
        if tokio::time::Instant::now() >= launch.deadline {
            return Err(LocalLaunchError::before_launch(
                "MCP launch expired before spawn",
            ));
        }
        Ok(())
    }

    fn sampling(&self) -> Result<bool, LocalLaunchError> {
        McpSettings::from_config(&self.resolved.value)
            .map(|settings| settings.sampling_enabled)
            .map_err(LocalLaunchError::before_launch)
    }

    async fn connect_local(
        &self,
        server: &ServerSelection,
        launch: &Startup<'_>,
        before_spawn: impl FnOnce() -> Result<(), String>,
    ) -> Result<Connected, LocalLaunchError> {
        let McpServer::Local {
            command,
            args,
            env,
            cwd,
            tools,
            ..
        } = &server.definition
        else {
            return Err(LocalLaunchError::before_launch(
                "MCP server requires remote transport",
            ));
        };
        let cwd = self.working_directory(cwd.as_deref())?;
        let (roots, fingerprint) = local_environment(
            self.resolved,
            self.home,
            self.location,
            server.requires_sandbox,
            launch.required_sandbox,
        )
        .map_err(|error| LocalLaunchError::before_launch(error.to_string()))?;
        let sampling = self.sampling()?;
        let mut resources = self
            .prepare(server.requires_sandbox, launch.required_sandbox)
            .await?;
        resources.scratch.cleanup = false;
        let prepared = self
            .wrap(&resources, command, args)
            .and_then(|wrapped| self.admit_effect(launch, before_spawn).map(|()| wrapped));
        let wrapped = match prepared {
            Ok(wrapped) => wrapped,
            Err(error) => return Err(resources.fail(error.diagnostic).await),
        };
        let environment = self.environment(&resources, &wrapped.env, env);
        let process = tokio::time::timeout_at(
            launch.deadline,
            HookCommandProcess::spawn_with_stdin(
                &wrapped.program,
                &wrapped.args,
                self.helper,
                |child| {
                    child
                        .env_clear()
                        .envs(&environment)
                        .current_dir(&cwd)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true);
                },
            ),
        )
        .await;
        let Ok(Ok(process)) = process else {
            return Err(LocalLaunchError {
                kind: LaunchFailure::Failed,
                settled: false,
                diagnostic: "MCP native launch failed; completion is unverified".into(),
                acknowledged: false,
                stderr: StderrCapture::default(),
                scratch: Some(resources.scratch.path.clone()),
            });
        };
        match StdioConnection::connect_with_options(
            process,
            launch.name,
            tools,
            launch.remaining(),
            super::StartupOptions {
                roots: Some(roots),
                sampling,
                cancellation: launch.cancel.cloned(),
            },
        )
        .await
        {
            Ok((connection, catalog)) => Ok(Connected {
                connection: Transport::Stdio(connection),
                catalog,
                resources,
                auth: None,
                environment: Some(fingerprint),
            }),
            Err(error) => Err(resources.fail_connection(error).await),
        }
    }

    fn working_directory(&self, cwd: Option<&Path>) -> Result<PathBuf, LocalLaunchError> {
        let cwd = std::fs::canonicalize(
            cwd.map(|path| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.location.join(path)
                }
            })
            .unwrap_or_else(|| self.location.into()),
        )
        .map_err(|_| LocalLaunchError::before_launch("MCP working directory unavailable"))?;
        if !cwd.is_dir() {
            return Err(LocalLaunchError::before_launch(
                "MCP working directory is not a directory",
            ));
        }
        Ok(cwd)
    }

    /// Remote servers are not processes: no working directory, wrapping or roots. The
    /// network policy applies through an owned loopback proxy, and project trust has
    /// already been checked by `authorize_server` and is rechecked before first contact.
    async fn connect_remote(
        &self,
        server: &ServerSelection,
        launch: &Startup<'_>,
        before_spawn: impl FnOnce() -> Result<(), String>,
    ) -> Result<Connected, LocalLaunchError> {
        let McpServer::Remote {
            url,
            headers,
            tools,
            ..
        } = &server.definition
        else {
            return Err(LocalLaunchError::before_launch(
                "MCP server requires local transport",
            ));
        };
        let url = reqwest::Url::parse(url)
            .map_err(|_| LocalLaunchError::before_launch("invalid MCP remote URL"))?;
        let headers = crate::net_client::headers(
            headers,
            "MCP remote server",
            cyber_core::config::transport_owned_header,
        )
        .map_err(LocalLaunchError::before_launch)?;
        let restricted = server.requires_sandbox || launch.required_sandbox;
        // The command would run outside the sandbox, network proxy and credential masking
        // that a sandbox-selected server or a sandboxed hook demands.
        if restricted && server.definition.headers_command().is_some() {
            return Err(LocalLaunchError::before_launch(
                "MCP headers_command cannot run for a sandboxed remote server",
            ));
        }
        let command = self.header_refresh(server);
        let sampling = self.sampling()?;
        let (mut resources, client) = self.prepare_remote(&url, restricted).await?;
        resources.scratch.cleanup = false;
        if let Err(error) = self.admit_effect(launch, before_spawn) {
            return Err(resources.fail(error.diagnostic).await);
        }
        let refresh_timeout = Duration::from_secs(server.definition.timeout().into());
        let mut auth = RemoteAuth::new(headers, command, client.clone(), refresh_timeout);
        if let Err(failure) = self.prepare_auth(server, &url, &mut auth, launch).await {
            return Err(resources.refuse(failure).await);
        }
        let target = RemoteTarget {
            client,
            url,
            tools,
            sampling,
        };
        match self.open_http(&mut auth, &target, launch).await {
            Ok((connection, catalog)) => Ok(Connected {
                connection: Transport::Http(connection),
                catalog,
                resources,
                auth: Some(auth),
                environment: None,
            }),
            Err(Opened::Connection(error)) => {
                if let McpError::Http(failure) = &error.error {
                    auth.step_up(launch.name, failure);
                }
                let mut error = resources.fail_connection(error).await;
                if error.kind == LaunchFailure::NeedsAuth && auth.unregistered {
                    error.kind = LaunchFailure::NeedsClientRegistration;
                }
                Err(error)
            }
            Err(Opened::Refused(failure)) => Err(resources.refuse(failure).await),
        }
    }

    fn header_refresh(&self, server: &ServerSelection) -> Option<HeaderRefresh> {
        let definition = &server.definition;
        definition.headers_command().map(|command| HeaderRefresh {
            // Static headers are their own layer beneath the bearer and the output.
            command: HeadersCommand::new(
                command,
                reqwest::header::HeaderMap::new(),
                self.location,
                self.helper,
            ),
            output: reqwest::header::HeaderMap::new(),
            every: Duration::from_secs(definition.headers_refresh_seconds().into()),
            timeout: Duration::from_secs(definition.timeout().into()),
            last: tokio::time::Instant::now(),
        })
    }

    /// The credential layers, after durable admission and approval: stored tokens for the
    /// exact URL (none with `oauth: false`), renewed first when expiring within five
    /// minutes, then the `headers_command` output. Approval is rechecked before the token
    /// request and after the command, before first contact with the server.
    async fn prepare_auth(
        &self,
        server: &ServerSelection,
        url: &reqwest::Url,
        auth: &mut RemoteAuth,
        launch: &Startup<'_>,
    ) -> Result<(), AuthFailure> {
        auth.tokens = self.stored_tokens(server, url, launch).await?;
        auth.unregistered = auth.tokens.is_none() && self.unregistered(server, url, launch.name)?;
        if auth
            .tokens
            .as_ref()
            .is_some_and(|tokens| tokens.due() && tokens.refreshable())
        {
            self.renew(auth, launch).await?;
        }
        if let Some(command) = auth.command.as_mut() {
            command
                .run(launch.deadline, launch.cancel)
                .await
                .map_err(AuthFailure::command)?;
            self.reauthorize(
                launch,
                "MCP server authorization changed during headers_command",
            )?;
        }
        auth.publish();
        Ok(())
    }

    /// The stored credential, read inside the startup deadline: an OS keyring read waiting
    /// on the user (a Keychain prompt) settles the server failed at the deadline instead of
    /// stalling its startup.
    async fn stored_tokens(
        &self,
        server: &ServerSelection,
        url: &reqwest::Url,
        launch: &Startup<'_>,
    ) -> Result<Option<TokenSource>, AuthFailure> {
        let Some(store) = self.credentials.clone() else {
            return Ok(None);
        };
        if server.definition.oauth().is_none() {
            return Ok(None);
        }
        let loaded =
            tokio::time::timeout_at(launch.deadline, TokenSource::load(store, launch.name, url));
        match loaded.await {
            Ok(loaded) => loaded.map_err(|error| {
                AuthFailure::failed(format!("MCP OAuth credential is unusable: {error}"))
            }),
            Err(_) => Err(AuthFailure::failed(
                "MCP OAuth credential store timed out".into(),
            )),
        }
    }

    /// Whether `cyber mcp auth` recorded that no client can be registered for `server` at
    /// exactly `url` and no `client_id` is configured. The runtime never runs discovery.
    fn unregistered(
        &self,
        server: &ServerSelection,
        url: &reqwest::Url,
        name: &str,
    ) -> Result<bool, AuthFailure> {
        let (Some(store), Some(settings)) = (&self.credentials, server.definition.oauth()) else {
            return Ok(false);
        };
        if settings.client_id.is_some() {
            return Ok(false);
        }
        store
            .marker(name, url.as_str())
            .map(|marker| marker.needs_client_registration)
            .map_err(|error| {
                AuthFailure::failed(format!("MCP OAuth credential store is unusable: {error}"))
            })
    }

    /// Renew the access token inside the startup deadline after rechecking approval. Any
    /// failure settles `needs_auth`; the refresh is never retried.
    async fn renew(&self, auth: &mut RemoteAuth, launch: &Startup<'_>) -> Result<(), AuthFailure> {
        self.reauthorize(
            launch,
            "MCP server authorization changed before token refresh",
        )?;
        let renewed = tokio::time::timeout_at(launch.deadline, auth.refresh_tokens()).await;
        match renewed {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(AuthFailure::needs_auth(format!(
                "MCP OAuth token refresh failed: {error}"
            ))),
            Err(_) => Err(AuthFailure::needs_auth(
                "MCP OAuth token refresh timed out".into(),
            )),
        }
    }

    fn reauthorize(&self, launch: &Startup<'_>, diagnostic: &str) -> Result<(), AuthFailure> {
        authorize_server(self.resolved, self.trust, self.location, launch.name)
            .map(|_| ())
            .map_err(|_| AuthFailure::failed(diagnostic.into()))
    }

    /// Connect with the composed headers. A 401 for a stored token that has a refresh
    /// token and was not renewed during this connect is retried exactly once after a
    /// refresh, inside the same deadline; a second refusal is final.
    async fn open_http(
        &self,
        auth: &mut RemoteAuth,
        target: &RemoteTarget<'_>,
        launch: &Startup<'_>,
    ) -> Result<(HttpConnection, Vec<DiscoveredTool>), Opened> {
        match self.attempt_http(auth, target, launch).await {
            Err(error) if retry_after_refresh(&error, auth) => {
                self.renew(auth, launch).await.map_err(Opened::Refused)?;
                self.attempt_http(auth, target, launch)
                    .await
                    .map_err(Opened::Connection)
            }
            result => result.map_err(Opened::Connection),
        }
    }

    async fn attempt_http(
        &self,
        auth: &RemoteAuth,
        target: &RemoteTarget<'_>,
        launch: &Startup<'_>,
    ) -> Result<(HttpConnection, Vec<DiscoveredTool>), ConnectionError> {
        HttpConnection::connect_with_options(
            super::HttpTarget {
                client: target.client.clone(),
                url: target.url.clone(),
                headers: auth.headers.clone(),
            },
            launch.name,
            target.tools,
            launch.remaining(),
            super::StartupOptions {
                roots: None,
                sampling: target.sampling,
                cancellation: launch.cancel.cloned(),
            },
            launch.first,
        )
        .await
    }

    /// A sandbox-selected server uses the configured network mode; otherwise the
    /// network is on. Either way traffic leaves only through the owned TCP proxy.
    async fn prepare_remote(
        &self,
        url: &reqwest::Url,
        restricted: bool,
    ) -> Result<(Resources, reqwest::Client), LocalLaunchError> {
        let config = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        let network = if restricted {
            config.network
        } else {
            NetworkMode::On
        };
        let scratch = Scratch::new(self.temp_dir)
            .map_err(|_| LocalLaunchError::before_launch("MCP scratch creation failed"))?;
        let policy = crate::net_client::policy_client(
            url,
            network,
            config.allowed_domains.clone(),
            "MCP remote server",
        )
        .await
        .map_err(LocalLaunchError::before_launch)?;
        Ok((
            Resources {
                config,
                scratch,
                proxy: Some(policy.proxy),
                proxy_acknowledged: false,
            },
            policy.client,
        ))
    }

    async fn prepare(
        &self,
        required: bool,
        required_sandbox: bool,
    ) -> Result<Resources, LocalLaunchError> {
        let (mut config, effective) =
            launch_sandbox(self.resolved, self.home, required, required_sandbox);
        config.policy = effective;
        let scratch = Scratch::new(self.temp_dir)
            .map_err(|_| LocalLaunchError::before_launch("MCP scratch creation failed"))?;
        let proxy = if config.policy != Policy::FullAccess && config.network == NetworkMode::Proxy {
            let domains = config.allowed_domains.clone();
            let decide: Decide = Arc::new(move |host| {
                let allowed = cyber_sandbox::matches_domain(&host, &domains);
                Box::pin(async move { allowed })
            });
            #[cfg(unix)]
            let proxy = if cyber_sandbox::proxy_uses_unix_socket() {
                Proxy::start_unix(decide, &scratch.path.join("proxy.sock")).await
            } else {
                Proxy::start(decide).await
            };
            #[cfg(not(unix))]
            let proxy = Proxy::start(decide).await;
            Some(proxy.map_err(|_| LocalLaunchError::before_launch("MCP proxy creation failed"))?)
        } else {
            None
        };
        let proxy_acknowledged = proxy.is_none();
        Ok(Resources {
            config,
            scratch,
            proxy,
            proxy_acknowledged,
        })
    }

    fn wrap(
        &self,
        resources: &Resources,
        command: &str,
        args: &[String],
    ) -> Result<cyber_sandbox::Wrapped, LocalLaunchError> {
        let writable = [self.location.to_path_buf(), resources.scratch.path.clone()]
            .into_iter()
            .collect::<Vec<_>>();
        let launch = Launch {
            config: resources.config.clone(),
            read_only: writable
                .iter()
                .flat_map(|root| cyber_sandbox::protected_in(root))
                .collect(),
            unreadable: resources.config.unreadable(self.home),
            writable,
            proxy: resources.proxy.as_ref().map(|proxy| proxy.endpoint.clone()),
            helper: self.helper.map(Path::to_path_buf),
        };
        cyber_sandbox::wrap(&launch, command, args)
            .map_err(|error| LocalLaunchError::before_launch(error.to_string()))
    }

    fn environment(
        &self,
        resources: &Resources,
        wrapped: &BTreeMap<String, String>,
        configured: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut credentials = crate::sandboxing::credential_env_names(&self.resolved.value);
        credentials.extend_from_slice(self.credential_env_names);
        let mut env: BTreeMap<_, _> = if resources.config.policy == Policy::FullAccess {
            std::env::vars().collect()
        } else {
            cyber_sandbox::mask_env(std::env::vars(), &resources.config.env_allow, &credentials)
                .into_iter()
                .collect()
        };
        env.extend(configured.clone());
        env.retain(|key, _| !reserved_env(key));
        env.extend(wrapped.clone());
        for key in ["TMPDIR", "TEMP", "TMP"] {
            env.insert(key.into(), resources.scratch.path.display().to_string());
        }
        env
    }
}

fn reserved_env(key: &str) -> bool {
    matches!(
        key.to_ascii_uppercase().as_str(),
        "TMPDIR"
            | "TEMP"
            | "TMP"
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "ALL_PROXY"
            | "NO_PROXY"
            | "CYBER_PROXY_SOCKET"
            | "CYBER_PROXY_PORT"
    )
}

/// A connected transport with its native resources and remote credential layers.
struct Connected {
    connection: Transport,
    catalog: Vec<DiscoveredTool>,
    resources: Resources,
    auth: Option<RemoteAuth>,
    environment: Option<String>,
}

/// The configured sandbox and the policy a server process actually runs under: a
/// sandboxed hook forces workspace-write, and a server `sandbox.apply_to` does not select
/// runs full-access.
fn launch_sandbox(
    resolved: &Resolved,
    home: &Path,
    selected: bool,
    required_sandbox: bool,
) -> (SandboxConfig, Policy) {
    let config = SandboxConfig::resolve(&resolved.value, &resolved.sources, None, home);
    let effective = if required_sandbox {
        Policy::WorkspaceWrite
    } else if selected {
        config.policy
    } else {
        Policy::FullAccess
    };
    (config, effective)
}

/// The roots a local connection advertises (the Location, plus the configured writable
/// roots unless the policy is read-only) and the fingerprint of its connection environment.
fn local_environment(
    resolved: &Resolved,
    home: &Path,
    location: &Path,
    selected: bool,
    required_sandbox: bool,
) -> Result<(super::McpRoots, String), McpError> {
    let (sandbox, effective) = launch_sandbox(resolved, home, selected, required_sandbox);
    let writable = if sandbox.policy == Policy::ReadOnly {
        &[][..]
    } else {
        sandbox.extra_writable.as_slice()
    };
    let roots = super::McpRoots::new(location, writable)?;
    let fingerprint = roots.fingerprint(&sandbox, effective);
    Ok((roots, fingerprint))
}

/// The connection environment a pool connection of `server` would capture now: the
/// fingerprint of its roots and sandbox grant, or `None` for a remote server, which
/// receives neither. A local environment that cannot be captured is an error; callers
/// treat it as changed.
pub(crate) fn environment_fingerprint(
    resolved: &Resolved,
    home: &Path,
    location: &Path,
    server: &ServerSelection,
) -> Result<Option<String>, McpError> {
    if !matches!(server.definition, McpServer::Local { .. }) {
        return Ok(None);
    }
    local_environment(resolved, home, location, server.requires_sandbox, false)
        .map(|(_, fingerprint)| Some(fingerprint))
}

/// The policy client and endpoint one remote connect uses for every attempt.
struct RemoteTarget<'a> {
    client: reqwest::Client,
    url: reqwest::Url,
    tools: &'a McpToolFilter,
    sampling: bool,
}

/// Why a remote connect ended without a connection.
enum Opened {
    Connection(ConnectionError),
    Refused(AuthFailure),
}

/// A credential failure before or between connect attempts.
struct AuthFailure {
    diagnostic: String,
    kind: LaunchFailure,
    acknowledged: bool,
}

impl AuthFailure {
    fn failed(diagnostic: String) -> Self {
        Self {
            diagnostic,
            kind: LaunchFailure::Failed,
            acknowledged: true,
        }
    }

    fn needs_auth(diagnostic: String) -> Self {
        Self {
            kind: LaunchFailure::NeedsAuth,
            ..Self::failed(diagnostic)
        }
    }

    fn command(failure: CommandFailure) -> Self {
        Self {
            acknowledged: failure.acknowledged,
            ..Self::failed(failure.diagnostic)
        }
    }
}

/// An acknowledged 401 that a refresh token may cure, before any renewal this connect.
fn retry_after_refresh(error: &ConnectionError, auth: &RemoteAuth) -> bool {
    error.acknowledged
        && auth.refreshable()
        && !auth.renewed
        && matches!(
            error.error,
            McpError::Http(HttpFailure::Unauthorized { status: 401, .. })
        )
}

/// Per-connect inputs shared by the local and remote branches.
struct Startup<'a> {
    name: &'a str,
    deadline: tokio::time::Instant,
    required_sandbox: bool,
    cancel: Option<&'a tokio_util::sync::CancellationToken>,
    first: RemoteTransport,
}

impl Startup<'_> {
    fn remaining(&self) -> Duration {
        self.deadline
            .saturating_duration_since(tokio::time::Instant::now())
    }
}

struct Resources {
    config: SandboxConfig,
    scratch: Scratch,
    proxy: Option<Proxy>,
    proxy_acknowledged: bool,
}
impl Resources {
    async fn fail(&mut self, diagnostic: String) -> LocalLaunchError {
        let acknowledged = self.stop_proxy().await;
        LocalLaunchError {
            kind: LaunchFailure::Failed,
            diagnostic,
            acknowledged,
            settled: false,
            stderr: StderrCapture::default(),
            scratch: Some(self.scratch.path.clone()),
        }
    }
    async fn refuse(&mut self, failure: AuthFailure) -> LocalLaunchError {
        let mut error = self.fail(failure.diagnostic).await;
        error.kind = failure.kind;
        error.acknowledged &= failure.acknowledged;
        error
    }
    async fn fail_connection(&mut self, error: ConnectionError) -> LocalLaunchError {
        let proxy = self.stop_proxy().await;
        LocalLaunchError {
            kind: if error.needs_auth() {
                LaunchFailure::NeedsAuth
            } else {
                LaunchFailure::Failed
            },
            settled: false,
            diagnostic: error.error.to_string(),
            acknowledged: error.acknowledged && proxy,
            stderr: error.stderr,
            scratch: Some(self.scratch.path.clone()),
        }
    }
    async fn stop_proxy(&mut self) -> bool {
        if let Some(proxy) = self.proxy.take() {
            self.proxy_acknowledged = matches!(
                tokio::time::timeout(Duration::from_secs(2), proxy.shutdown()).await,
                Ok(Ok(()))
            );
        }
        self.proxy_acknowledged
    }
}
struct Scratch {
    path: PathBuf,
    cleanup: bool,
}
impl Scratch {
    fn new(parent: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!("mcp-{}", ulid::Ulid::new()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self {
            path,
            cleanup: true,
        })
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn interrupted_proxy_shutdown_cannot_later_claim_cleanup_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        let config =
            SandboxConfig::resolve(&serde_json::json!({}), &BTreeMap::new(), None, root.path());
        let proxy = Proxy::start(Arc::new(|_| Box::pin(async { false })))
            .await
            .unwrap();
        let mut resources = Resources {
            config,
            scratch: Scratch::new(root.path()).unwrap(),
            proxy: Some(proxy),
            proxy_acknowledged: false,
        };
        let mut shutdown = Box::pin(resources.stop_proxy());
        assert!(futures::poll!(&mut shutdown).is_pending());
        drop(shutdown);
        assert!(resources.proxy.is_none());
        assert!(!resources.stop_proxy().await);
    }
    #[tokio::test]
    async fn prepared_failure_joins_proxy_and_retains_scratch_for_caller_commit() {
        let root = tempfile::tempdir().unwrap();
        let config =
            SandboxConfig::resolve(&serde_json::json!({}), &BTreeMap::new(), None, root.path());
        let proxy = Proxy::start(Arc::new(|_| Box::pin(async { false })))
            .await
            .unwrap();
        let mut resources = Resources {
            config,
            scratch: Scratch::new(root.path()).unwrap(),
            proxy: Some(proxy),
            proxy_acknowledged: false,
        };
        resources.scratch.cleanup = false;
        let error = resources.fail("injected admission failure".into()).await;
        assert!(error.acknowledged);
        assert!(!error.retry_safe());
        assert!(resources.proxy.is_none());
        drop(resources);
        assert!(error.scratch.unwrap().exists());
    }
}
