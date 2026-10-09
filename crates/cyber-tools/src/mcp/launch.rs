//! Configuration-driven local MCP launch under caller-retained durable ownership.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::{McpServer, McpToolFilter, Resolved};
use cyber_core::trust::TrustStore;
use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig};
use serde_json::Value;

use super::{McpError, StderrCapture, StdioConnection, authorize_server};
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

#[derive(Debug, thiserror::Error)]
#[error("{diagnostic} (native termination acknowledged: {acknowledged})")]
pub struct LocalLaunchError {
    pub diagnostic: String,
    pub acknowledged: bool,
    pub stderr: StderrCapture,
    pub scratch: Option<PathBuf>,
}

impl LocalLaunchError {
    fn before_launch(diagnostic: impl Into<String>) -> Self {
        Self {
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
    connection: StdioConnection,
    resources: Resources,
    tools: McpToolFilter,
    shutdown_acknowledged: bool,
    authority: ServerAuthority,
    catalog: Vec<super::DiscoveredTool>,
    metadata: Value,
    aliases: super::discovery::ToolAliases,
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

    pub async fn refresh_tools(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.authorize()?;
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
        self.authorize()?;
        let remote = self
            .catalog
            .iter()
            .find(|tool| tool.exposed_name == name)
            .map(|tool| tool.remote_name.clone())
            .ok_or_else(|| McpError::StaleTool(name.into()))?;
        self.call_tool(&remote, arguments, timeout).await
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

    pub fn unresolved(&self) -> bool {
        self.connection.unresolved()
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.authorize()?;
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
        self.connection.call_tool(name, arguments, timeout).await
    }

    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.connection.cancel_pending(timeout).await
    }

    /// Cleanup is authorized only after both the native tree and local proxy settle.
    pub async fn shutdown(&mut self) -> (bool, StderrCapture) {
        let (native, stderr) = self.connection.shutdown().await;
        let proxy = self.resources.stop_proxy().await;
        let acknowledged = native && proxy;
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
        let server = authorize_server(self.resolved, self.trust, self.location, name)
            .map_err(LocalLaunchError::before_launch)?;
        let McpServer::Local {
            command,
            args,
            env,
            cwd,
            timeout,
            tools,
            ..
        } = &server.definition
        else {
            return Err(LocalLaunchError::before_launch(
                "MCP server requires remote transport",
            ));
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs((*timeout).into());
        let cwd = std::fs::canonicalize(
            cwd.as_ref()
                .map(|path| {
                    if path.is_absolute() {
                        path.clone()
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
        let mut resources = self.prepare(server.requires_sandbox).await?;
        let wrapped = self.wrap(&resources, command, args)?;
        let environment = self.environment(&resources, &wrapped.env, env);
        before_spawn().map_err(LocalLaunchError::before_launch)?;
        // Preparation and durable admission can await or revoke approval.
        authorize_server(self.resolved, self.trust, self.location, name)
            .map_err(LocalLaunchError::before_launch)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(LocalLaunchError::before_launch(
                "MCP launch expired before spawn",
            ));
        }
        resources.scratch.cleanup = false;
        let process = tokio::time::timeout_at(
            deadline,
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
        let process = match process {
            Ok(Ok(process)) => process,
            _ => {
                return Err(LocalLaunchError {
                    diagnostic: "MCP native launch failed; completion is unverified".into(),
                    acknowledged: false,
                    stderr: StderrCapture::default(),
                    scratch: Some(resources.scratch.path.clone()),
                });
            }
        };
        match StdioConnection::connect_with_tools(
            process,
            name,
            tools,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
        {
            Ok((connection, mut catalog)) => {
                let mut aliases = super::discovery::ToolAliases::default();
                aliases.retain(name, &mut catalog);
                Ok(LocalServer {
                    aliases,
                    metadata: connection.metadata().cloned().unwrap_or(Value::Null),
                    catalog,
                    connection,
                    resources,
                    tools: tools.clone(),
                    shutdown_acknowledged: false,
                    authority: ServerAuthority {
                        resolved: self.resolved.clone(),
                        trust: self.trust.clone(),
                        location: self.location.into(),
                        name: name.into(),
                    },
                })
            }
            Err(error) => {
                let proxy = resources.stop_proxy().await;
                let acknowledged = error.acknowledged && proxy;
                Err(LocalLaunchError {
                    diagnostic: error.error.to_string(),
                    acknowledged,
                    stderr: error.stderr,
                    scratch: Some(resources.scratch.path.clone()),
                })
            }
        }
    }

    async fn prepare(&self, required: bool) -> Result<Resources, LocalLaunchError> {
        let mut config = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        if !required {
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

struct Resources {
    config: SandboxConfig,
    scratch: Scratch,
    proxy: Option<Proxy>,
    proxy_acknowledged: bool,
}
impl Resources {
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
}
