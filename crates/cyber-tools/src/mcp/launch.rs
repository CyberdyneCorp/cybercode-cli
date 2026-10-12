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

use super::headers_command::HeadersCommand;
use super::{
    ConnectionError, DiscoveredTool, HeaderSet, HttpConnection, IdleUpdate, McpError,
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
}

/// Why a launch failed, so owners can choose the durable status and retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LaunchFailure {
    #[default]
    Failed,
    /// The remote server refused authorization (HTTP 401/403). Nothing else was sent,
    /// no transport fallback ran, and reconnecting cannot succeed without credentials.
    NeedsAuth,
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
    metadata: Value,
    aliases: super::discovery::ToolAliases,
    refresh: Option<HeaderRefresh>,
    /// A header command whose termination was not acknowledged; shutdown cannot settle.
    command_unsettled: bool,
}

/// Periodic `headers_command` re-runs for a connected remote server.
struct HeaderRefresh {
    command: HeadersCommand,
    headers: HeaderSet,
    every: Duration,
    timeout: Duration,
    last: tokio::time::Instant,
}

impl HeaderRefresh {
    fn due(&self) -> bool {
        self.last.elapsed() >= self.every
    }
}

/// A stdio process or a remote Streamable HTTP session. The HTTP arm owns no process, so
/// it never touches process-group grace or native tree state.
enum Transport {
    Stdio(StdioConnection),
    Http(HttpConnection),
}

impl Transport {
    fn clear_tools_changed(&mut self) {
        match self {
            Self::Stdio(connection) => connection.clear_tools_changed(),
            Self::Http(connection) => connection.clear_tools_changed(),
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

    fn auth_refusal(&self) -> Option<super::HttpFailure> {
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

impl LocalServer {
    pub fn tools(&self) -> &[super::DiscoveredTool] {
        &self.catalog
    }

    pub fn metadata(&self) -> &Value {
        &self.metadata
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
        self.connection.clear_tools_changed();
        self.catalog.clear();
        self.catalog = self
            .connection
            .discover_tools(&self.authority.name, &self.tools, timeout)
            .await?;
        self.aliases.retain(&self.authority.name, &mut self.catalog);
        Ok(())
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
        authorize_server(
            &self.authority.resolved,
            &self.authority.trust,
            &self.authority.location,
            &self.authority.name,
        )
        .map(|_| ())
        .map_err(|_| McpError::Protocol("MCP server authorization is no longer valid"))
    }

    pub fn scratch_path(&self) -> &Path {
        &self.resources.scratch.path
    }

    /// The pool monitor's idle tick. A due `headers_command` re-runs first, so the idle
    /// ping never carries headers past their refresh window; a failed refresh is loss and
    /// reconnecting re-runs the command.
    pub async fn poll_idle(&mut self, timeout: Duration) -> Result<super::IdleUpdate, McpError> {
        self.refresh_headers().await?;
        self.connection.poll_idle(timeout).await
    }

    /// Re-run a due `headers_command` and swap the bridge's headers in place. Every request
    /// path calls this first, so a busy server whose monitor tick is skipped still refreshes.
    /// A failed run leaves the refresh due, so no later request reuses the stale headers.
    async fn refresh_headers(&mut self) -> Result<(), McpError> {
        if !self.refresh.as_ref().is_some_and(HeaderRefresh::due) {
            return Ok(());
        }
        self.authorize()?;
        let Some(refresh) = self.refresh.as_mut() else {
            return Ok(());
        };
        let started = tokio::time::Instant::now();
        match refresh
            .command
            .headers(started + refresh.timeout, None)
            .await
        {
            Ok(headers) => {
                refresh.headers.replace(headers);
                refresh.last = started;
                Ok(())
            }
            Err(failure) => {
                self.command_unsettled |= !failure.acknowledged;
                cyber_core::log::error(
                    "mcp",
                    &failure.diagnostic,
                    serde_json::json!({"server":self.authority.name,"acknowledged":failure.acknowledged}),
                );
                Err(McpError::Protocol("MCP headers_command refresh failed"))
            }
        }
    }

    pub async fn disconnected(&mut self) -> Result<bool, McpError> {
        self.connection.disconnected().await
    }

    pub fn unresolved(&self) -> bool {
        self.connection.unresolved()
    }

    /// The remote authorization refusal (HTTP 401/403) that ended an established
    /// connection, which settles `needs_auth` instead of a retryable failure. With a
    /// `headers_command` the refusal may only mean its credential expired, so it is loss:
    /// the reconnect re-runs the command and settles `needs_auth` if that is refused too.
    pub fn auth_refusal(&self) -> Option<super::HttpFailure> {
        if self.refresh.is_some() {
            return None;
        }
        self.connection.auth_refusal()
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
        self.connection
            .call_tool_with_elicitation(name, arguments, timeout, context)
            .await
    }

    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
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
        let acknowledged = native && proxy && !self.command_unsettled;
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
            McpServer::Remote { .. } => self.connect_remote(&server, &launch, before_spawn).await?,
        };
        Ok(self.server(name, server.definition.tools(), connected))
    }

    fn server(&self, name: &str, tools: &McpToolFilter, connected: Connected) -> LocalServer {
        let Connected {
            connection,
            mut catalog,
            resources,
            refresh,
        } = connected;
        let mut aliases = super::discovery::ToolAliases::default();
        aliases.retain(name, &mut catalog);
        LocalServer {
            aliases,
            metadata: connection.metadata().cloned().unwrap_or(Value::Null),
            catalog,
            connection,
            resources,
            refresh,
            command_unsettled: false,
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
        let roots_config = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        let writable = if roots_config.policy == Policy::ReadOnly {
            &[][..]
        } else {
            roots_config.extra_writable.as_slice()
        };
        let roots = super::McpRoots::new(self.location, writable)
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
                refresh: None,
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
        let command = server.definition.headers_command().map(|command| {
            HeadersCommand::new(command, headers.clone(), self.location, self.helper)
        });
        let sampling = self.sampling()?;
        let (mut resources, client) = self.prepare_remote(&url, restricted).await?;
        resources.scratch.cleanup = false;
        if let Err(error) = self.admit_effect(launch, before_spawn) {
            return Err(resources.fail(error.diagnostic).await);
        }
        let headers = match self
            .initial_headers(command.as_ref(), headers, launch)
            .await
        {
            Ok(headers) => HeaderSet::new(headers),
            Err(failure) => {
                let mut error = resources.fail(failure.diagnostic).await;
                error.acknowledged &= failure.acknowledged;
                return Err(error);
            }
        };
        let refresh = command.map(|command| HeaderRefresh {
            command,
            headers: headers.clone(),
            every: Duration::from_secs(server.definition.headers_refresh_seconds().into()),
            timeout: Duration::from_secs(server.definition.timeout().into()),
            last: tokio::time::Instant::now(),
        });
        match HttpConnection::connect_with_options(
            super::HttpTarget {
                client,
                url,
                headers,
            },
            launch.name,
            tools,
            launch.remaining(),
            super::StartupOptions {
                roots: None,
                sampling,
                cancellation: launch.cancel.cloned(),
            },
            launch.first,
        )
        .await
        {
            Ok((connection, catalog)) => Ok(Connected {
                connection: Transport::Http(connection),
                catalog,
                resources,
                refresh,
            }),
            Err(error) => Err(resources.fail_connection(error).await),
        }
    }

    /// Static headers, or the `headers_command` output merged over them. The command runs
    /// after durable admission and approval, and approval is rechecked before first contact.
    async fn initial_headers(
        &self,
        command: Option<&HeadersCommand>,
        headers: reqwest::header::HeaderMap,
        launch: &Startup<'_>,
    ) -> Result<reqwest::header::HeaderMap, super::headers_command::CommandFailure> {
        let Some(command) = command else {
            return Ok(headers);
        };
        let headers = command.headers(launch.deadline, launch.cancel).await?;
        authorize_server(self.resolved, self.trust, self.location, launch.name).map_err(|_| {
            super::headers_command::CommandFailure {
                diagnostic: "MCP server authorization changed during headers_command".into(),
                acknowledged: true,
            }
        })?;
        Ok(headers)
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
        let mut config = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        if required_sandbox {
            config.policy = Policy::WorkspaceWrite;
        } else if !required {
            config.policy = Policy::FullAccess;
        }
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

/// A connected transport with its native resources and optional header refresh.
struct Connected {
    connection: Transport,
    catalog: Vec<DiscoveredTool>,
    resources: Resources,
    refresh: Option<HeaderRefresh>,
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
