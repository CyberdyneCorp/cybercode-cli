//! Shared Location connections with retained startup tasks and native owners.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cyber_core::config::{McpServer, McpSettings};
use cyber_server::runtime::{
    Invocation, McpConnectionPhase, McpConnectionStatus, McpServerStatus, McpStatusUpdate,
    RetrySafety, SessionInfo, ToolDef, ToolOutcome, TurnContext, mcp_connections,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::oauth::McpCredentialStore;
use super::{
    DiscoveredTool, LocalLaunchError, LocalLauncher, OwnedLocalServer, RemoteTransport,
    authorize_server,
};
use crate::host::BuiltinHost;
use crate::tools::ToolError;

mod prompts;
pub(crate) use prompts::RESERVED_SERVER;

/// Shared connections, plus the stored OAuth credentials remote connections send.
#[derive(Default)]
pub(crate) struct Pool(Mutex<State>, OnceLock<Arc<dyn McpCredentialStore>>);
#[derive(Default)]
struct State {
    closed: bool,
    closing: BTreeSet<PathBuf>,
    paused: BTreeSet<PathBuf>,
    entries: BTreeMap<(PathBuf, String), Arc<Entry>>,
}
impl State {
    fn admits_start(&mut self, directory: &Path, reopen: bool) -> bool {
        if self.closed || self.closing.contains(directory) {
            return false;
        }
        if reopen {
            self.paused.remove(directory);
            return true;
        }
        !self.paused.contains(directory)
    }
}

struct Entry {
    name: String,
    digest: String,
    cancel: CancellationToken,
    published: Mutex<Option<Published>>,
    server: tokio::sync::Mutex<Option<OwnedLocalServer>>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    observed: Mutex<Option<McpStatusUpdate>>,
    monitor: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    startup_finished: AtomicBool,
    lost: AtomicBool,
    /// Cancelled only to restart under a changed roots or sandbox grant; status reports
    /// `connecting` until the replacement is admitted or the restart is abandoned.
    replacing: AtomicBool,
    /// The remote transport that last connected; reconnects try it first.
    transport: Mutex<RemoteTransport>,
    /// The Session that admitted this entry; an explicit reconnect re-admits through it.
    info: Option<SessionInfo>,
}

/// Why an explicit reconnect was refused. Messages carry no credentials.
#[derive(Debug, thiserror::Error)]
pub enum McpReconnectError {
    /// The server is absent from the loaded configuration.
    #[error("{0}")]
    NotFound(String),
    /// Trust or the individual digest approval does not authorize the server.
    #[error("{0}")]
    Forbidden(String),
    /// The server is neither settled waiting for the user (`needs_auth`,
    /// `needs_client_registration`) nor running on a credential that was logged out.
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
}

struct Published {
    id: String,
    tools: Vec<DiscoveredTool>,
    prompts: Vec<super::DiscoveredPrompt>,
    instructions: Option<String>,
    /// The server advertised the `resources` capability at initialization.
    resources: bool,
    /// The local connection environment (roots and sandbox grant) the process was
    /// launched with; `None` for a remote connection.
    environment: Option<String>,
}
impl Published {
    fn from_server(server: &OwnedLocalServer) -> Self {
        Self {
            id: server.record().id.clone(),
            environment: server.environment().map(str::to_owned),
            tools: server.tools().to_vec(),
            prompts: server.prompts().to_vec(),
            resources: server.metadata()["capabilities"]["resources"].is_object(),
            instructions: server.metadata()["instructions"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned),
        }
    }
    fn changed(
        &self,
        name: &str,
        owner: &OwnedLocalServer,
    ) -> cyber_server::runtime::McpToolsChanged {
        cyber_server::runtime::McpToolsChanged {
            connection_id: self.id.clone(),
            directory: owner.record().directory.clone(),
            name: name.into(),
            tools: self
                .tools
                .iter()
                .map(|tool| tool.exposed_name.clone())
                .collect(),
            prompts: self
                .prompts
                .iter()
                .map(|prompt| format!("{name}:{}", prompt.name))
                .collect(),
        }
    }
    fn instruction_block(&self, name: &str, visible: &[ToolDef]) -> Option<String> {
        let text = self.instructions.as_ref()?;
        let eligible = self.tools.iter().any(|tool| {
            let binding = tool_binding(&self.id, tool);
            visible.iter().any(|definition| {
                definition.scope == cyber_server::runtime::ToolScope::Mcp
                    && definition.spec.name == tool.exposed_name
                    && definition.registration.as_deref() == Some(binding.as_str())
            })
        });
        eligible.then(|| {
            let text = text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            format!("<server name=\"{name}\">\n{text}\n</server>")
        })
    }
}

impl Entry {
    async fn stop(&self) -> Result<(), String> {
        self.cancel.cancel();
        self.clear_published();
        self.join_startup().await;
        join_retained(&self.monitor).await;
        self.clear_published();
        let mut server = self.server.lock().await;
        if let Some(owner) = server.as_mut() {
            owner
                .shutdown_gracefully()
                .await
                .map_err(|error| error.diagnostic)?;
            server.take();
        }
        Ok(())
    }

    /// The status the latest observed record settled with, if it is settled.
    fn settled(&self) -> Option<McpConnectionStatus> {
        self.observed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|record| record.phase == McpConnectionPhase::Settled)
            .map(|record| record.status)
    }

    /// Settled `needs_auth` or `needs_client_registration`, with startup finished and no
    /// live monitor or native owner.
    fn waiting_for_auth(&self) -> bool {
        self.settled_idle(waits_for_user)
    }

    /// Settled `failed` (a launch or initialization that never ran), with startup
    /// finished and no live monitor or native owner; a failure that retains its owner
    /// is not retried.
    fn failed_idle(&self) -> bool {
        self.settled_idle(|status| status == McpConnectionStatus::Failed)
    }

    /// Settled with a status `accepted` takes, startup finished, the monitor done and no
    /// native owner retained.
    fn settled_idle(&self, accepted: impl FnOnce(McpConnectionStatus) -> bool) -> bool {
        let settled = self.settled().is_some_and(accepted);
        let idle = |task: &tokio::sync::Mutex<Option<JoinHandle<()>>>| {
            task.try_lock()
                .is_ok_and(|task| task.as_ref().is_none_or(JoinHandle::is_finished))
        };
        settled
            && self.startup_finished.load(Ordering::Acquire)
            && idle(&self.monitor)
            && self.server.try_lock().is_ok_and(|server| server.is_none())
    }

    /// Running on an OAuth credential that `cyber mcp logout` removed.
    fn logged_out(&self) -> bool {
        self.server
            .try_lock()
            .is_ok_and(|server| server.as_ref().is_some_and(OwnedLocalServer::logged_out))
    }

    /// Whether the entry is published and `offers` holds for its publication.
    fn offers(&self, offers: impl FnOnce(&Published) -> bool) -> bool {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(offers)
    }

    fn clear_published(&self) {
        *self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    async fn join_startup(&self) {
        join_retained(&self.task).await;
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Settled statuses that only the user can resolve; retrying cannot succeed.
fn waits_for_user(status: McpConnectionStatus) -> bool {
    matches!(
        status,
        McpConnectionStatus::NeedsAuth | McpConnectionStatus::NeedsClientRegistration
    )
}

async fn join_retained(task: &tokio::sync::Mutex<Option<JoinHandle<()>>>) {
    let mut task = task.lock().await;
    if let Some(handle) = task.as_mut() {
        // Disposal releases the lock while retaining the original task for retry.
        let _ = handle.await;
        task.take();
    }
}

/// A new entry whose retained startup task connects, publishes and starts the monitor.
fn spawn_entry(
    host: &Arc<BuiltinHost>,
    executor: &tokio::runtime::Handle,
    name: String,
    digest: String,
    info: &SessionInfo,
) -> Arc<Entry> {
    let entry = Arc::new(Entry {
        name: name.clone(),
        digest,
        cancel: CancellationToken::new(),
        published: Mutex::default(),
        server: tokio::sync::Mutex::new(None),
        task: tokio::sync::Mutex::default(),
        observed: Mutex::default(),
        monitor: tokio::sync::Mutex::default(),
        startup_finished: AtomicBool::new(false),
        lost: AtomicBool::new(false),
        replacing: AtomicBool::new(false),
        transport: Mutex::default(),
        info: Some(info.clone()),
    });
    let owned = entry.clone();
    let host = host.clone();
    let info = info.clone();
    let task = executor.spawn(async move {
        let result = tokio::select! {
            result = connect_entry(&host, &owned, &info) => Some(result),
            _ = owned.cancel.cancelled() => None,
        };
        match result {
            Some(Ok(server)) => {
                publish_server(&host, &owned, server).await;
                start_monitor(&host, &owned, info);
            }
            Some(Err(error)) => cyber_core::log::error(
                "mcp",
                &error.diagnostic,
                json!({"server":name,"acknowledged":error.acknowledged}),
            ),
            None => {}
        }
        owned.startup_finished.store(true, Ordering::Release);
    });
    // The entry is not published until its startup handle is installed.
    *entry.task.try_lock().expect("unpublished MCP entry") = Some(task);
    entry
}

fn start_monitor(host: &Arc<BuiltinHost>, entry: &Arc<Entry>, info: SessionInfo) {
    let host = Arc::downgrade(host);
    let weak = Arc::downgrade(entry);
    let cancel = entry.cancel.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            }
            let Some(entry) = weak.upgrade() else {
                return;
            };
            match check_loss(&host, &entry).await {
                Health::Healthy => {}
                Health::Stop => return,
                Health::Retry => {
                    if !retry_with_backoff(&cancel, || reconnect_once(&host, &entry, &info)).await {
                        return;
                    }
                }
            }
        }
    });
    *entry
        .monitor
        .try_lock()
        .expect("MCP monitor not yet published") = Some(task);
}

enum Health {
    Healthy,
    Retry,
    Stop,
}
enum Attempt {
    Connected,
    Retry,
    Stop,
}

async fn retry_with_backoff<F, Fut>(cancel: &CancellationToken, mut attempt: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Attempt>,
{
    for index in 0..10 {
        let delay = Duration::from_secs((1_u64 << index).min(60));
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            _ = tokio::time::sleep(delay) => {},
        }
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            result = attempt() => result,
        };
        match result {
            Attempt::Connected => return true,
            Attempt::Retry => {}
            Attempt::Stop => return false,
        }
    }
    false
}

async fn reconnect_once(
    host: &std::sync::Weak<BuiltinHost>,
    entry: &Arc<Entry>,
    info: &SessionInfo,
) -> Attempt {
    let Some(host) = host.upgrade() else {
        return Attempt::Stop;
    };
    match connect_entry(&host, entry, info).await {
        Ok(server) => {
            publish_server(&host, entry, server).await;
            Attempt::Connected
        }
        Err(error) => {
            cyber_core::log::error(
                "mcp",
                &error.diagnostic,
                json!({"server":entry.name,"acknowledged":error.acknowledged,"retry_safe":error.retry_safe()}),
            );
            failed_attempt(&error)
        }
    }
}

/// Authentication cannot succeed by retrying, and unsettled failures must not re-admit.
fn failed_attempt(error: &LocalLaunchError) -> Attempt {
    if error.kind().needs_user() || !error.retry_safe() {
        Attempt::Stop
    } else {
        Attempt::Retry
    }
}

/// Publish a (re)connected server, then announce its tools and prompt commands with
/// `mcp.tools.changed.1`: its `connected` status is committed before publication, so
/// clients refetch command listings on this event instead.
async fn publish_server(host: &BuiltinHost, entry: &Entry, server: OwnedLocalServer) {
    entry.lost.store(false, Ordering::Release);
    if let Some(transport) = server.remote_transport() {
        *entry
            .transport
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = transport;
    }
    let published = Published::from_server(&server);
    let changed = published.changed(&entry.name, &server);
    *entry
        .published
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(published);
    *entry.server.lock().await = Some(server);
    if let Some(runtime) = host.runtime() {
        runtime.publish_mcp_tools_changed(changed);
    }
}

async fn connect_entry(
    host: &Arc<BuiltinHost>,
    entry: &Arc<Entry>,
    info: &SessionInfo,
) -> Result<OwnedLocalServer, LocalLaunchError> {
    let config = host
        .hook_config
        .get()
        .ok_or_else(|| LocalLaunchError::before_launch("MCP resolver unavailable"))?;
    let directory = Path::new(&info.directory)
        .canonicalize()
        .map_err(|error| LocalLaunchError::before_launch(error.to_string()))?;
    let resolved = (config.resolve)(&directory).map_err(LocalLaunchError::before_launch)?;
    let selected = authorize_server(&resolved, &config.trust, &directory, &entry.name)
        .map_err(LocalLaunchError::before_launch)?;
    if selected.digest != entry.digest {
        return Err(LocalLaunchError::before_launch(
            "MCP definition changed; close and reopen required",
        ));
    }
    let credentials = host
        .opts
        .models
        .as_ref()
        .map(|models| models.credential_env_names())
        .unwrap_or_default();
    let launcher = LocalLauncher {
        resolved: &resolved,
        trust: &config.trust,
        location: &directory,
        home: &host.opts.home,
        temp_dir: &host.opts.temp_dir,
        helper: host.opts.sandbox_helper.as_deref(),
        credential_env_names: &credentials,
        credentials: host.mcp_credentials(),
    };
    let publish = host.runtime().map(|runtime| runtime.mcp_status_observer());
    let weak = Arc::downgrade(entry);
    let observer: cyber_server::runtime::McpConnectionObserver = Arc::new(move |record, seq| {
        if let Some(entry) = weak.upgrade() {
            *entry
                .observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(record.into());
        }
        if let Some(publish) = &publish {
            publish(record, seq);
        }
    });
    let first = *entry
        .transport
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    launcher
        .connect_owned_observed(
            &entry.name,
            host.opts.store.clone(),
            Some(observer),
            first,
            |id| host.claim_mcp_location(info, id, entry.cancel.child_token()),
        )
        .await
}

async fn check_loss(host: &std::sync::Weak<BuiltinHost>, entry: &Entry) -> Health {
    let Ok(mut server) = entry.server.try_lock() else {
        return Health::Healthy;
    };
    let Some(owner) = server.as_mut() else {
        return Health::Stop;
    };
    let lost = match owner.poll_idle(Duration::from_millis(100)).await {
        Ok(update) if update.closed => true,
        Ok(update) if update.changed() => match refresh_catalog(host, entry, owner, update).await {
            Ok(()) => false,
            Err(error) => {
                cyber_core::log::error("mcp", &error, json!({"server":entry.name}));
                true
            }
        },
        Ok(_) => false,
        Err(error) => {
            cyber_core::log::error("mcp", &error.to_string(), json!({"server":entry.name}));
            true
        }
    };
    if !lost {
        return Health::Healthy;
    }
    entry.lost.store(true, Ordering::Release);
    entry.clear_published();
    cyber_core::log::error("mcp", "MCP connection lost", json!({"server":entry.name}));
    match owner.shutdown().await {
        Ok((record, _)) => {
            if let Err(error) = std::fs::remove_dir_all(owner.scratch_path())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                cyber_core::log::error("mcp", &error.to_string(), json!({"server":entry.name}));
                return Health::Stop;
            }
            server.take();
            // An authorization refusal settled waiting for the user; retrying cannot succeed.
            if waits_for_user(record.status) {
                Health::Stop
            } else {
                Health::Retry
            }
        }
        Err(error) => {
            cyber_core::log::error(
                "mcp",
                &error.diagnostic,
                json!({"server":entry.name,"acknowledged":error.acknowledged}),
            );
            Health::Stop
        }
    }
}

/// Re-list what a `list_changed` notification announced. Resources hold no cache, so
/// their notification only clears; a tools or prompts refresh unpublishes the entry,
/// rechecks the approval digest, re-lists and publishes `mcp.tools.changed.1`.
async fn refresh_catalog(
    host: &std::sync::Weak<BuiltinHost>,
    entry: &Entry,
    owner: &mut OwnedLocalServer,
    update: super::IdleUpdate,
) -> Result<(), String> {
    if update.resources_changed {
        owner
            .clear_resources_changed()
            .map_err(|error| error.to_string())?;
    }
    if !update.tools_changed && !update.prompts_changed {
        return Ok(());
    }
    entry.clear_published();
    let host = host.upgrade().ok_or("MCP host disposed")?;
    let directory = &owner.record().directory;
    let config = host
        .hook_config
        .get()
        .ok_or_else(|| "MCP resolver unavailable".to_string())?;
    let resolved = (config.resolve)(directory)?;
    let selected = authorize_server(&resolved, &config.trust, directory, &entry.name)?;
    if selected.digest != entry.digest {
        return Err("MCP definition changed".into());
    }
    let refresh = refresh_lists(
        owner,
        update,
        Duration::from_secs(selected.definition.timeout().into()),
    );
    tokio::select! {
        result = refresh => result.map_err(|error| error.to_string())?,
        _ = entry.cancel.cancelled() => return Err("MCP refresh interrupted".into()),
    }
    let published = Published::from_server(owner);
    if let Some(runtime) = host.runtime() {
        runtime.publish_mcp_tools_changed(published.changed(&entry.name, owner));
    }
    *entry
        .published
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(published);
    Ok(())
}

async fn refresh_lists(
    owner: &mut OwnedLocalServer,
    update: super::IdleUpdate,
    timeout: Duration,
) -> Result<(), super::McpError> {
    if update.tools_changed {
        owner.refresh_tools(timeout).await?;
    }
    if update.prompts_changed {
        owner.refresh_prompts(timeout).await?;
    }
    Ok(())
}

fn tool_binding(id: &str, tool: &DiscoveredTool) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(cyber_core::config::canonical_json(&tool.definition));
    format!("{id}:{}:{digest:x}", tool.remote_name)
}

impl BuiltinHost {
    /// Side-effect-free configured status and retained ownership observations.
    pub fn mcp_status(&self, directory: &Path) -> Result<Vec<McpServerStatus>, String> {
        let directory = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let config = self
            .hook_config
            .get()
            .ok_or_else(|| "MCP resolver unavailable".to_string())?;
        let resolved = (config.resolve)(&directory)?;
        let settings = McpSettings::from_config(&resolved.value)?;
        let records: BTreeMap<_, _> = mcp_connections(&self.opts.store, &directory)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|record| record.phase != McpConnectionPhase::Settled)
            .map(|record| (record.name.clone(), McpStatusUpdate::from(&record)))
            .collect();
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut names: BTreeSet<_> = settings
            .servers
            .keys()
            .chain(records.keys())
            .cloned()
            .collect();
        names.extend(
            pool.entries
                .keys()
                .filter(|(location, _)| location == &directory)
                .map(|(_, name)| name.clone()),
        );
        Ok(names
            .into_iter()
            .map(|name| {
                let definition = settings.servers.get(&name);
                let entry = pool.entries.get(&(directory.clone(), name.clone()));
                let connection = entry
                    .and_then(|entry| {
                        entry
                            .observed
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone()
                    })
                    .or_else(|| records.get(&name).cloned());
                let problem = status_problem(
                    definition,
                    entry,
                    &connection,
                    pool.closed || pool.closing.contains(&directory),
                    || authorize_server(&resolved, &config.trust, &directory, &name).is_ok(),
                )
                .or_else(|| {
                    // A published connection under a stale roots or sandbox grant serves
                    // nothing until the next refresh restarts it.
                    entry
                        .filter(|entry| self.environment_changed(&resolved, &directory, entry))
                        .map(|_| {
                            (
                                McpConnectionStatus::Connecting,
                                "MCP roots or sandbox grant changed; restart pending".into(),
                            )
                        })
                });
                let (status, error) = match problem {
                    Some((status, error)) => (status, Some(error.into_owned())),
                    None => (
                        connection
                            .as_ref()
                            .map_or(McpConnectionStatus::Connecting, |record| record.status),
                        connection.as_ref().and_then(|record| record.error.clone()),
                    ),
                };
                let tools = if status == McpConnectionStatus::Connected {
                    entry
                        .and_then(|entry| {
                            entry
                                .published
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .as_ref()
                                .map(|published| {
                                    published
                                        .tools
                                        .iter()
                                        .map(|tool| tool.exposed_name.clone())
                                        .collect()
                                })
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                McpServerStatus {
                    name,
                    configured: definition.is_some(),
                    status,
                    error,
                    tools,
                    connection,
                }
            })
            .collect())
    }

    pub(crate) fn start_mcp(&self, info: &SessionInfo) {
        self.admit_mcp(info, true);
    }
    pub(crate) fn refresh_mcp(&self, info: &SessionInfo) {
        self.admit_mcp(info, false);
    }

    fn admit_mcp(&self, info: &SessionInfo, reopen: bool) {
        let Some(config) = self.hook_config.get() else {
            return;
        };
        let Ok(directory) = Path::new(&info.directory).canonicalize() else {
            return;
        };
        let Ok(resolved) = (config.resolve)(&directory) else {
            return;
        };
        let Ok(settings) = McpSettings::from_config(&resolved.value) else {
            return;
        };
        let Some(host) = self.weak.upgrade() else {
            return;
        };
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !pool.admits_start(&directory, reopen) {
            return;
        }
        for (name, definition) in settings.servers {
            if !definition.enabled() {
                continue;
            }
            let key = (directory.clone(), name.clone());
            if let Some(entry) = pool.entries.get(&key) {
                if self.environment_changed(&resolved, &directory, entry) {
                    entry.replacing.store(true, Ordering::Release);
                    entry.cancel.cancel();
                    executor.spawn(replace_entry(host.clone(), key, entry.clone()));
                }
                continue;
            }
            let Ok(selection) = authorize_server(&resolved, &config.trust, &directory, &name)
            else {
                continue;
            };
            let entry = spawn_entry(&host, &executor, name, selection.digest, info);
            pool.entries.insert(key, entry);
        }
    }

    /// Bind the stored MCP OAuth credentials that remote connections send as a bearer.
    pub fn attach_mcp_credentials(&self, store: Arc<dyn McpCredentialStore>) -> Result<(), String> {
        self.mcp
            .1
            .set(store)
            .map_err(|_| "MCP credentials are already attached".into())
    }

    /// The attached MCP OAuth credential store, shared by pool and hook connections.
    pub(crate) fn mcp_credentials(&self) -> Option<Arc<dyn McpCredentialStore>> {
        self.mcp.1.get().cloned()
    }

    /// Reconnect a server whose connection settled `needs_auth` or
    /// `needs_client_registration` and whose monitor stopped, typically after `cyber mcp
    /// auth` stored a token, a server that settled `failed` without retaining a native
    /// owner (the sidebar's `r`), or a running server whose OAuth credential `cyber mcp
    /// logout` removed (which then settles `needs_auth`). Configuration, trust and the digest
    /// approval are checked first; the entry is then stopped and removed and the server is
    /// re-admitted through its Session with a fresh durable owner. The returned status is
    /// observed right after admission (`connecting`); later transitions publish events.
    pub async fn reconnect_mcp(
        &self,
        directory: &Path,
        name: &str,
    ) -> Result<McpServerStatus, McpReconnectError> {
        let directory = directory
            .canonicalize()
            .map_err(|_| McpReconnectError::NotFound("MCP Location is unavailable".into()))?;
        let digest = self.reconnect_selection(&directory, name)?;
        let key = (directory.clone(), name.to_string());
        let entry = self.reconnectable(&key, &digest)?;
        let info = entry.info.clone().ok_or_else(|| {
            McpReconnectError::Conflict("MCP server has no Session to reconnect through".into())
        })?;
        entry.stop().await.map_err(McpReconnectError::Conflict)?;
        let host = self
            .weak
            .upgrade()
            .ok_or_else(|| McpReconnectError::Unavailable("MCP host disposed".into()))?;
        let executor = tokio::runtime::Handle::try_current()
            .map_err(|_| McpReconnectError::Unavailable("MCP runtime unavailable".into()))?;
        {
            let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            let current = pool.entries.get(&key);
            if pool.closed
                || pool.closing.contains(&directory)
                || !current.is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                return Err(McpReconnectError::Conflict(
                    "MCP Location changed during reconnect".into(),
                ));
            }
            let replacement = spawn_entry(&host, &executor, name.into(), digest, &info);
            pool.entries.insert(key, replacement);
        }
        self.mcp_status(&directory)
            .map_err(McpReconnectError::Unavailable)?
            .into_iter()
            .find(|status| status.name == name)
            .ok_or_else(|| McpReconnectError::NotFound("MCP server is not configured".into()))
    }

    /// The currently authorized digest of a configured, enabled server.
    fn reconnect_selection(
        &self,
        directory: &Path,
        name: &str,
    ) -> Result<String, McpReconnectError> {
        let unavailable = |error: String| McpReconnectError::Unavailable(error);
        let config = self
            .hook_config
            .get()
            .ok_or_else(|| unavailable("MCP resolver unavailable".into()))?;
        let resolved = (config.resolve)(directory).map_err(unavailable)?;
        let settings = McpSettings::from_config(&resolved.value).map_err(unavailable)?;
        let definition = settings
            .servers
            .get(name)
            .ok_or_else(|| McpReconnectError::NotFound("MCP server is not configured".into()))?;
        if !definition.enabled() {
            return Err(McpReconnectError::Conflict("MCP server is disabled".into()));
        }
        authorize_server(&resolved, &config.trust, directory, name)
            .map(|selection| selection.digest)
            .map_err(McpReconnectError::Forbidden)
    }

    /// The retained entry, when it settled waiting for the user (neither its startup, its
    /// monitor nor a native owner still running) or runs on a logged-out credential. The
    /// definition must be unchanged, except that `needs_client_registration` is resolved
    /// by configuring `oauth.client_id`, which changes it; `digest` is the currently
    /// authorized one either way.
    fn reconnectable(
        &self,
        key: &(PathBuf, String),
        digest: &str,
    ) -> Result<Arc<Entry>, McpReconnectError> {
        let conflict = |message: &str| McpReconnectError::Conflict(message.into());
        let entry = {
            let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.closed || pool.closing.contains(&key.0) {
                return Err(conflict("MCP Location is closing or requires recovery"));
            }
            pool.entries
                .get(key)
                .cloned()
                .ok_or_else(|| conflict("MCP server has not started for this Location"))?
        };
        let waiting = entry.waiting_for_auth();
        let registering =
            waiting && entry.settled() == Some(McpConnectionStatus::NeedsClientRegistration);
        if entry.digest != digest && !registering {
            return Err(conflict(
                "MCP definition changed; close and reopen the Location",
            ));
        }
        if entry.cancel.is_cancelled() || !(waiting || entry.logged_out() || entry.failed_idle()) {
            return Err(conflict(
                "MCP server is not waiting for authentication or failed",
            ));
        }
        Ok(entry)
    }

    pub(crate) fn mcp_definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let Some(config) = self.hook_config.get() else {
            return Vec::new();
        };
        let Ok(directory) = Path::new(&turn.directory).canonicalize() else {
            return Vec::new();
        };
        let Ok(resolved) = (config.resolve)(&directory) else {
            return Vec::new();
        };
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.closed || pool.closing.contains(&directory) {
            return Vec::new();
        }
        let mut definitions = Vec::new();
        for ((location, _), entry) in &pool.entries {
            if location != &directory || entry.cancel.is_cancelled() {
                continue;
            }
            let Ok(selection) = authorize_server(&resolved, &config.trust, &directory, &entry.name)
            else {
                continue;
            };
            if selection.digest != entry.digest {
                continue;
            }
            let published = entry
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(Published { id, tools, .. }) = published.as_ref().filter(|published| {
                self.environment_current(&resolved, &directory, &selection, published)
            }) {
                definitions.extend(
                    tools
                        .iter()
                        .filter(|tool| {
                            crate::permissions::Mode::parse(&turn.mode)
                                != crate::permissions::Mode::Plan
                                || tool.read_only_hint()
                        })
                        .map(|tool| ToolDef {
                            scope: cyber_server::runtime::ToolScope::Mcp,
                            deferred: false,
                            registration: Some(tool_binding(id, tool)),
                            spec: tool.spec(),
                            // Annotations control permission defaults, not safe automatic retries.
                            retry_safety: RetrySafety::Never,
                            concurrency_safe: false,
                        }),
                );
            }
        }
        definitions
    }

    /// Whether connection `id` of `name` is the live publication: authorized with its
    /// digest and still running under the roots and sandbox grant the configuration selects.
    pub(super) fn mcp_is_published(&self, directory: &Path, name: &str, id: &str) -> bool {
        let Ok(directory) = directory.canonicalize() else {
            return false;
        };
        let Some(config) = self.hook_config.get() else {
            return false;
        };
        let Ok(resolved) = (config.resolve)(&directory) else {
            return false;
        };
        self.live_entry(&directory, &resolved, name)
            .is_some_and(|entry| entry.offers(|published| published.id == id))
    }

    /// Observe instructions only for effective, freshly authorized MCP registrations.
    pub(crate) fn mcp_instructions(
        &self,
        turn: &TurnContext,
        visible: &[ToolDef],
    ) -> Option<String> {
        if !visible
            .iter()
            .any(|tool| tool.scope == cyber_server::runtime::ToolScope::Mcp)
        {
            return None;
        }
        let config = self.hook_config.get()?;
        let directory = Path::new(&turn.directory).canonicalize().ok()?;
        let resolved = (config.resolve)(&directory).ok()?;
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.closed || pool.closing.contains(&directory) {
            return None;
        }
        let mut blocks = Vec::new();
        for ((location, _), entry) in &pool.entries {
            if location != &directory || entry.cancel.is_cancelled() {
                continue;
            }
            let Ok(selection) = authorize_server(&resolved, &config.trust, &directory, &entry.name)
            else {
                continue;
            };
            if selection.digest != entry.digest {
                continue;
            }
            let published = entry
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(published) = &*published else {
                continue;
            };
            if !self.environment_current(&resolved, &directory, &selection, published) {
                continue;
            }
            if let Some(block) = published.instruction_block(&entry.name, visible) {
                blocks.push(block);
            }
        }
        (!blocks.is_empty()).then(|| {
            format!(
                "<mcp_instructions>\n{}\n</mcp_instructions>",
                blocks.join("\n")
            )
        })
    }

    /// Stop shared MCP actors for one canonical Location; unknown effects remain fenced.
    /// Successful close permits a later open to create new connection identities.
    pub async fn close_mcp_location(&self, directory: &Path) -> Result<(), String> {
        let directory = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let entries: Vec<_> = {
            let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            pool.closing.insert(directory.clone());
            pool.entries
                .iter()
                .filter(|((location, _), _)| location == &directory)
                .map(|(key, entry)| (key.clone(), entry.clone()))
                .collect()
        };
        for (_, entry) in &entries {
            entry.cancel.cancel();
        }
        let results =
            futures::future::join_all(entries.iter().map(|(_, entry)| entry.stop())).await;
        let failures: Vec<_> = entries
            .iter()
            .zip(results)
            .filter_map(|((_, entry), result)| {
                result.err().map(|error| format!("{}: {error}", entry.name))
            })
            .collect();
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }
        let records =
            mcp_connections(&self.opts.store, &directory).map_err(|error| error.to_string())?;
        if records
            .iter()
            .any(|record| record.phase != McpConnectionPhase::Settled)
        {
            return Err(
                "MCP Location has unresolved native ownership; recovery is required".into(),
            );
        }
        let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (key, entry) in entries {
            if pool
                .entries
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                pool.entries.remove(&key);
            }
        }
        pool.paused.insert(directory.clone());
        pool.closing.remove(&directory);
        Ok(())
    }

    pub(crate) async fn shutdown_mcp(&self) {
        let entries: Vec<_> = {
            let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            pool.closed = true;
            pool.entries.values().cloned().collect()
        };
        for entry in &entries {
            entry.cancel.cancel();
        }
        let results = futures::future::join_all(entries.iter().map(|entry| entry.stop())).await;
        for (entry, result) in entries.iter().zip(results) {
            if let Err(error) = result {
                cyber_core::log::error("mcp", &error, json!({"server":entry.name}));
            }
        }
    }

    pub(crate) async fn execute_mcp(
        &self,
        inv: Invocation,
        cancel: CancellationToken,
    ) -> ToolOutcome {
        let (entry, tool) = match self.mcp_binding(&inv) {
            Ok(binding) => binding,
            Err(ToolError::Failed(error)) => return ToolOutcome::Failed(error),
            Err(ToolError::Aborted) => return ToolOutcome::Aborted,
        };
        let definition = ToolDef {
            scope: cyber_server::runtime::ToolScope::Mcp,
            deferred: false,
            registration: inv.registration.clone(),
            spec: tool.spec(),
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let metadata = json!({"server":entry.name,"annotations":tool.definition["annotations"]});
        let token_limit = match self
            .hook_config
            .get()
            .ok_or_else(|| "MCP resolver unavailable".to_string())
            .and_then(|config| (config.resolve)(Path::new(&inv.directory)))
            .and_then(|resolved| McpSettings::from_config(&resolved.value))
        {
            Ok(settings) => settings
                .servers
                .get(&entry.name)
                .and_then(McpServer::output_token_limit),
            Err(error) => return ToolOutcome::Failed(error),
        };
        self.execute_registered_with_output_limit(
            inv,
            definition,
            crate::registered::RegisteredOptions {
                read_only: tool.read_only_hint(),
                metadata,
                output_token_limit: token_limit,
            },
            cancel,
            move |inv, cancel| {
                Box::pin(async move {
                    match self
                        .invoke_mcp(&entry, &inv, &cancel)
                        .await
                        .and_then(output)
                    {
                        Ok(outcome) => outcome,
                        Err(ToolError::Failed(error)) => ToolOutcome::Failed(error),
                        Err(ToolError::Aborted) => ToolOutcome::Aborted,
                    }
                })
            },
        )
        .await
    }

    async fn invoke_mcp(
        &self,
        entry: &Entry,
        inv: &Invocation,
        cancel: &CancellationToken,
    ) -> Result<Value, ToolError> {
        let mut server = tokio::select! {
            server = entry.server.lock() => server,
            _ = cancel.cancelled() => return Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => return Err(ToolError::Aborted),
        };
        self.mcp_binding(inv)?;
        let owner = server
            .as_mut()
            .ok_or_else(|| ToolError::Failed(format!("Stale tool call: {}", inv.name)))?;
        let config = self
            .hook_config
            .get()
            .ok_or_else(|| ToolError::Failed("MCP resolver unavailable".into()))?;
        let resolved = (config.resolve)(Path::new(&inv.directory)).map_err(ToolError::Failed)?;
        let timeout = McpSettings::from_config(&resolved.value)
            .map_err(ToolError::Failed)?
            .call_timeout(&entry.name);
        let authorize = || self.mcp_binding(inv).is_ok();
        let sampler = super::sampler::Sampler::new(
            self.weak
                .upgrade()
                .ok_or_else(|| ToolError::Failed("MCP host unavailable".into()))?,
            inv.clone(),
            entry.name.clone(),
            Duration::from_secs(timeout.into()),
        );
        // Elicitation hooks stop with the call or its server, and a cancelled call
        // waits for launched handlers to settle before it is dropped.
        let hook_cancel = cancel.child_token();
        let flight = super::elicitation::HookFlight::default();
        let caller = caller_marker(owner.metadata(), self.runtime().as_ref(), inv);
        let context = super::ElicitationContext::new(&inv.asker, &authorize)
            .with_caller(caller.as_ref())
            .with_sampling(&sampler)
            .with_hooks(super::elicitation::ElicitationHooks {
                host: self,
                inv,
                cancel: &hook_cancel,
                flight: &flight,
            });
        let result = {
            let call = owner.call_exposed_tool_with_elicitation(
                &inv.name,
                inv.input.clone(),
                Duration::from_secs(timeout.into()),
                &context,
            );
            tokio::pin!(call);
            tokio::select! {
                result = &mut call => result.map_err(|error| ToolError::Failed(error.to_string())),
                _ = cancel.cancelled() => {
                    flight.settle(call.as_mut()).await;
                    Err(ToolError::Aborted)
                }
                _ = entry.cancel.cancelled() => {
                    hook_cancel.cancel();
                    flight.settle(call.as_mut()).await;
                    Err(ToolError::Aborted)
                }
            }
        };
        let sampling_settlement = sampler.finish().await;
        end_unresolved(entry, owner, || {
            self.entry_authorized(Path::new(&inv.directory), entry)
        })
        .await;
        inv.asker
            .cancel_questions()
            .await
            .map_err(|_| ToolError::Failed("MCP question cleanup failed".into()))?;
        drop(server);
        sampling_settlement.map_err(|error| ToolError::Failed(error.into()))?;
        result
    }

    /// Whether the pool is shut down or `directory` is closing, so nothing there connects.
    pub(crate) fn mcp_location_closed(&self, directory: &Path) -> bool {
        let Ok(directory) = directory.canonicalize() else {
            return true;
        };
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        pool.closed || pool.closing.contains(&directory)
    }

    /// Servers in `directory` whose published, freshly authorized connection advertised
    /// `resources`.
    pub(crate) fn mcp_resource_servers(&self, directory: &Path) -> Vec<String> {
        let Ok(directory) = directory.canonicalize() else {
            return Vec::new();
        };
        let names: Vec<String> = {
            let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            pool.entries
                .keys()
                .filter(|(location, _)| location == &directory)
                .map(|(_, name)| name.clone())
                .collect()
        };
        names
            .into_iter()
            .filter(|name| self.resource_entry(&directory, name).is_ok())
            .collect()
    }

    /// One resource RPC on a server's shared connection. Authorization is looked up
    /// fresh before and after taking the connection, the configured call timeout bounds
    /// it, and a request left unresolved ends the connection as loss. The RPC runs
    /// [`detached`], so a caller dropped mid-request still settles it.
    pub(crate) async fn mcp_resource(
        &self,
        directory: &Path,
        server: &str,
        request: super::ResourceRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<super::ResourceReply, ToolError> {
        let host = self
            .weak
            .upgrade()
            .ok_or_else(|| unavailable_resources(server))?;
        let (directory, server) = (directory.to_path_buf(), server.to_owned());
        let request = super::resources::OwnedResourceRequest::of(request);
        let cancel = cancel.clone();
        detached(async move {
            host.settled_resource(&directory, &server, request.as_request(), &cancel)
                .await
        })
        .await
    }

    async fn settled_resource(
        &self,
        directory: &Path,
        server: &str,
        request: super::ResourceRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<super::ResourceReply, ToolError> {
        let entry = self.resource_entry(directory, server)?;
        let mut guard = tokio::select! {
            guard = entry.server.lock() => guard,
            _ = cancel.cancelled() => return Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => return Err(ToolError::Aborted),
        };
        if !Arc::ptr_eq(&self.resource_entry(directory, server)?, &entry) {
            return Err(unavailable_resources(server));
        }
        let owner = guard
            .as_mut()
            .ok_or_else(|| unavailable_resources(server))?;
        let timeout = self.mcp_call_timeout(directory, server)?;
        let result = tokio::select! {
            result = owner.resource(request, timeout) => result.map_err(resource_error),
            _ = cancel.cancelled() => Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => Err(ToolError::Aborted),
        };
        end_unresolved(&entry, owner, || self.entry_authorized(directory, &entry)).await;
        result
    }

    /// Whether `entry`'s definition is still authorized with the digest it started with,
    /// read fresh. Unlike [`Self::live_entry`] a closing Location does not matter here.
    fn entry_authorized(&self, directory: &Path, entry: &Entry) -> bool {
        let (Some(config), Ok(directory)) = (self.hook_config.get(), directory.canonicalize())
        else {
            return false;
        };
        (config.resolve)(&directory).is_ok_and(|resolved| {
            authorize_server(&resolved, &config.trust, &directory, &entry.name)
                .is_ok_and(|selection| selection.digest == entry.digest)
        })
    }

    fn mcp_call_timeout(&self, directory: &Path, server: &str) -> Result<Duration, ToolError> {
        let config = self
            .hook_config
            .get()
            .ok_or_else(|| ToolError::Failed("MCP resolver unavailable".into()))?;
        let resolved = (config.resolve)(directory).map_err(ToolError::Failed)?;
        let settings = McpSettings::from_config(&resolved.value).map_err(ToolError::Failed)?;
        Ok(Duration::from_secs(settings.call_timeout(server).into()))
    }

    /// The live entry for `server` when it is published with `resources`, its Location is
    /// open and its definition is still authorized with the digest it started with.
    fn resource_entry(&self, directory: &Path, server: &str) -> Result<Arc<Entry>, ToolError> {
        let directory = directory
            .canonicalize()
            .map_err(|_| unavailable_resources(server))?;
        let config = self
            .hook_config
            .get()
            .ok_or_else(|| unavailable_resources(server))?;
        let resolved = (config.resolve)(&directory).map_err(ToolError::Failed)?;
        self.live_entry(&directory, &resolved, server)
            .filter(|entry| entry.offers(|published| published.resources))
            .ok_or_else(|| unavailable_resources(server))
    }

    /// The entry for `server` in the canonical `directory` when its Location is open, its
    /// connection is published and not lost, and its definition is still authorized with
    /// the digest it started with.
    fn live_entry(
        &self,
        directory: &Path,
        resolved: &cyber_core::config::Resolved,
        server: &str,
    ) -> Option<Arc<Entry>> {
        let config = self.hook_config.get()?;
        let entry = {
            let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.closed || pool.closing.contains(directory) {
                return None;
            }
            pool.entries
                .get(&(directory.to_path_buf(), server.to_string()))
                .cloned()?
        };
        let authorized =
            authorize_server(resolved, &config.trust, directory, server).is_ok_and(|selection| {
                selection.digest == entry.digest
                    && entry.offers(|published| {
                        self.environment_current(resolved, directory, &selection, published)
                    })
            });
        let live =
            authorized && !entry.cancel.is_cancelled() && !entry.lost.load(Ordering::Acquire);
        live.then_some(entry)
    }

    /// Whether `published` still runs under the roots and sandbox grant that the current
    /// configuration selects for `selection`. A remote connection, which receives neither,
    /// always does; a local environment that cannot be captured now has changed.
    fn environment_current(
        &self,
        resolved: &cyber_core::config::Resolved,
        directory: &Path,
        selection: &super::ServerSelection,
        published: &Published,
    ) -> bool {
        let Some(captured) = published.environment.as_deref() else {
            return true;
        };
        super::launch::environment_fingerprint(resolved, &self.opts.home, directory, selection)
            .is_ok_and(|current| current.as_deref() == Some(captured))
    }

    /// Whether `entry` is published, still authorized with its digest, and running under a
    /// roots and sandbox grant the configuration no longer selects.
    fn environment_changed(
        &self,
        resolved: &cyber_core::config::Resolved,
        directory: &Path,
        entry: &Entry,
    ) -> bool {
        let Some(config) = self.hook_config.get() else {
            return false;
        };
        if entry.cancel.is_cancelled() {
            return false;
        }
        authorize_server(resolved, &config.trust, directory, &entry.name).is_ok_and(|selection| {
            selection.digest == entry.digest
                && entry.offers(|published| {
                    !self.environment_current(resolved, directory, &selection, published)
                })
        })
    }

    pub(super) fn authorize_mcp_sampling(&self, inv: &Invocation) -> Result<(), ToolError> {
        self.mcp_binding(inv).map(|_| ())
    }

    fn mcp_binding(&self, inv: &Invocation) -> Result<(Arc<Entry>, DiscoveredTool), ToolError> {
        let stale = || ToolError::Failed(format!("Stale tool call: {}", inv.name));
        let directory = Path::new(&inv.directory)
            .canonicalize()
            .map_err(|_| stale())?;
        let config = self.hook_config.get().ok_or_else(stale)?;
        let resolved = (config.resolve)(&directory).map_err(ToolError::Failed)?;
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.closed || pool.closing.contains(&directory) {
            return Err(stale());
        }
        for ((location, _), entry) in &pool.entries {
            if location != &directory || entry.cancel.is_cancelled() {
                continue;
            }
            let published = entry
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(current) = &*published else {
                continue;
            };
            let Some(tool) = current
                .tools
                .iter()
                .find(|tool| tool.exposed_name == inv.name)
            else {
                continue;
            };
            if inv.registration.as_deref() != Some(tool_binding(&current.id, tool).as_str()) {
                return Err(stale());
            }
            let selection = authorize_server(&resolved, &config.trust, &directory, &entry.name)
                .map_err(ToolError::Failed)?;
            if selection.digest != entry.digest {
                return Err(stale());
            }
            if !self.environment_current(&resolved, &directory, &selection, current) {
                return Err(ToolError::Failed(SANDBOX_CHANGED.into()));
            }
            return Ok((entry.clone(), tool.clone()));
        }
        Err(stale())
    }
}

/// Why a local registration is refused after its roots or sandbox grant changed.
const SANDBOX_CHANGED: &str = "MCP sandbox configuration changed; reconnecting";

/// Replace a local connection whose roots or sandbox grant changed (its entry is already
/// cancelled, so nothing new reaches it): stop and settle it, then re-admit the server
/// through the Session that admitted it, so a fresh connection captures the current
/// configuration. A Location closed or changed meanwhile keeps the stopped entry.
async fn replace_entry(host: Arc<BuiltinHost>, key: (PathBuf, String), entry: Arc<Entry>) {
    if !restart_entry(&host, key, &entry).await {
        // The old entry stays, cancelled, and now reports why it is not running.
        entry.replacing.store(false, Ordering::Release);
    }
}

/// Stop `entry` and admit its replacement; false when the restart was abandoned.
async fn restart_entry(
    host: &Arc<BuiltinHost>,
    key: (PathBuf, String),
    entry: &Arc<Entry>,
) -> bool {
    if let Err(error) = entry.stop().await {
        cyber_core::log::error("mcp", &error, json!({"server":entry.name}));
        return false;
    }
    let Some(info) = entry.info.clone() else {
        return false;
    };
    let digest = match host.reconnect_selection(&key.0, &key.1) {
        Ok(digest) => digest,
        Err(error) => {
            cyber_core::log::info("mcp", &error.to_string(), json!({"server":entry.name}));
            return false;
        }
    };
    let Ok(executor) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    let mut pool = host.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
    let current = pool.entries.get(&key);
    if pool.closed
        || pool.closing.contains(&key.0)
        || !current.is_some_and(|current| Arc::ptr_eq(current, entry))
    {
        return false;
    }
    let replacement = spawn_entry(host, &executor, key.1.clone(), digest, &info);
    pool.entries.insert(key, replacement);
    true
}

/// Run a shared-connection RPC in its own task. A caller dropped mid-request (a
/// disconnected HTTP client, an interrupted `cyber exec`) cannot leave the request
/// unsettled: the task still awaits the reply, timeout or cancellation and ends an
/// unresolved request as loss before releasing the connection.
async fn detached<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, ToolError>> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::spawn(work).await.unwrap_or_else(|error| {
        cyber_core::log::error("mcp", &error.to_string(), json!({}));
        Err(ToolError::Failed("MCP request task failed".into()))
    })
}

/// How long an interrupted request's `notifications/cancelled` may delay its shutdown.
const CANCEL_NOTICE: Duration = Duration::from_secs(1);

/// End a connection whose request is unresolved: unpublish it as lost, tell the server the
/// request is cancelled (best effort, bounded, never settlement, and only while
/// `authorized` still holds) and shut it down.
async fn end_unresolved(
    entry: &Entry,
    owner: &mut OwnedLocalServer,
    authorized: impl FnOnce() -> bool,
) {
    if !owner.unresolved() {
        return;
    }
    entry.lost.store(true, Ordering::Release);
    entry.clear_published();
    if authorized() {
        notify_cancelled(entry, owner).await;
    }
    if let Err(error) = owner.shutdown().await {
        cyber_core::log::error(
            "mcp",
            &error.diagnostic,
            json!({"server":entry.name,"acknowledged":error.acknowledged}),
        );
    }
}

/// Send the unresolved request's `notifications/cancelled` within [`CANCEL_NOTICE`]; a
/// failure is logged with typed, credential-free diagnostics only.
async fn notify_cancelled(entry: &Entry, owner: &mut OwnedLocalServer) {
    let notice = tokio::time::timeout(CANCEL_NOTICE, owner.cancel_pending(CANCEL_NOTICE)).await;
    let error = match notice {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(_) => super::McpError::Timeout,
    };
    cyber_core::log::info(
        "mcp",
        "MCP cancellation notice was not delivered",
        json!({"server":entry.name,"error":error.to_string()}),
    );
}

fn unavailable_resources(server: &str) -> ToolError {
    ToolError::Failed(format!(
        "MCP server {server} is not connected or offers no resources"
    ))
}

/// Errors carry only typed facts: an oversized remote body is the transport limit, and a
/// server rejection reports its JSON-RPC code, never its text.
fn resource_error(error: super::McpError) -> ToolError {
    ToolError::Failed(match error {
        super::McpError::Http(super::HttpFailure::Oversized) => {
            "MCP resource exceeds transport limit (1 MiB response)".into()
        }
        error => error.to_string(),
    })
}

fn status_problem(
    definition: Option<&McpServer>,
    entry: Option<&Arc<Entry>>,
    connection: &Option<McpStatusUpdate>,
    closing: bool,
    authorized: impl FnOnce() -> bool,
) -> Option<(McpConnectionStatus, std::borrow::Cow<'static, str>)> {
    let failed = |message: &'static str| Some((McpConnectionStatus::Failed, message.into()));
    let Some(definition) = definition else {
        return failed("MCP server is absent from loaded configuration");
    };
    if !definition.enabled() {
        return Some((
            McpConnectionStatus::Disabled,
            "MCP server is disabled".into(),
        ));
    }
    if !authorized() {
        return failed("MCP server definition is not authorized");
    }
    if closing {
        return failed("MCP Location is closing or requires recovery");
    }
    if entry.is_some_and(|entry| entry.replacing.load(Ordering::Acquire)) {
        return Some((
            McpConnectionStatus::Connecting,
            "MCP server is restarting under a changed roots or sandbox grant".into(),
        ));
    }
    if entry.is_some_and(|entry| entry.cancel.is_cancelled()) {
        return failed(
            if connection
                .as_ref()
                .is_some_and(|record| record.phase == McpConnectionPhase::Settled)
            {
                "MCP connection stopped; reopen or reconnect is required"
            } else {
                "MCP native settlement requires retry or recovery"
            },
        );
    }
    let Some(entry) = entry else {
        return failed(if connection.is_some() {
            "MCP ownership has no retained runtime actor; recovery is required"
        } else {
            "MCP server has not started for this Location"
        });
    };
    if definition.digest(&entry.name).ok().as_ref() != Some(&entry.digest) {
        return failed("MCP definition changed; close and reopen the Location");
    }
    if entry.lost.load(Ordering::Acquire)
        && connection.as_ref().is_some_and(|record| {
            matches!(
                record.phase,
                McpConnectionPhase::Running | McpConnectionPhase::Unknown
            )
        })
    {
        return failed("MCP connection lost; native settlement requires retry or recovery");
    }
    if let Some(waiting) = connection.as_ref().and_then(user_action) {
        return Some(waiting);
    }
    if connection
        .as_ref()
        .is_some_and(|record| record.phase == McpConnectionPhase::Settled)
    {
        return failed("MCP connection stopped; reconnect pending or exhausted");
    }
    if connection.is_none() && entry.startup_finished.load(Ordering::Acquire) {
        return failed("MCP startup failed; inspect local logs");
    }
    None
}

/// The public status and next step of a connection settled waiting for the user.
fn user_action(
    record: &McpStatusUpdate,
) -> Option<(McpConnectionStatus, std::borrow::Cow<'static, str>)> {
    if record.phase != McpConnectionPhase::Settled || !waits_for_user(record.status) {
        return None;
    }
    let name = &record.name;
    let message = if record.status == McpConnectionStatus::NeedsClientRegistration {
        format!(
            "MCP server {name} needs OAuth client registration; its authorization server offers \
             no dynamic registration, so register a client there, set oauth.client_id, then run \
             `cyber mcp auth {name}`"
        )
    } else {
        format!("MCP server {name} needs authentication; run `cyber mcp auth {name}`")
    };
    Some((record.status, message.into()))
}

fn output(result: Value) -> Result<ToolOutcome, ToolError> {
    let content = result["content"]
        .as_array()
        .ok_or_else(|| ToolError::Failed("MCP result has no content list".into()))?;
    let text = content
        .iter()
        .filter(|item| item["type"] == "text")
        .map(|item| {
            item["text"]
                .as_str()
                .ok_or_else(|| ToolError::Failed("MCP text content is invalid".into()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    if result["isError"] == true {
        return Ok(ToolOutcome::Failed(text));
    }
    if content.iter().any(|item| item["type"] != "text") {
        return Err(ToolError::Failed(
            "MCP non-text content attachment handling is not implemented".into(),
        ));
    }
    Ok(match result.get("structuredContent") {
        Some(value) => ToolOutcome::Structured {
            output: text,
            value: value.clone(),
        },
        None => ToolOutcome::Ok(text),
    })
}

/// `_meta["dev.cyber-code/caller"]` for a call to a Cyber Code MCP server (`serverInfo.name`
/// `cyber-code`): the calling Session and its depth + 1; `None` for any other server.
fn caller_marker(
    metadata: &Value,
    runtime: Option<&cyber_server::runtime::Runtime>,
    inv: &Invocation,
) -> Option<Value> {
    if metadata["serverInfo"]["name"] != "cyber-code" {
        return None;
    }
    let depth = runtime.map_or(0, |runtime| runtime.effective_mcp_depth(&inv.session_id));
    Some(json!({ "session_id": inv.session_id, "depth": depth.saturating_add(1) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_blocks_require_exact_mcp_visibility_and_escape_frame_boundaries() {
        let tool = DiscoveredTool {
            remote_name: "read".into(),
            exposed_name: "mcp__server__read".into(),
            definition: json!({"name":"read","inputSchema":{"type":"object"}}),
        };
        let mut published = Published {
            id: "mcs_original".into(),
            tools: vec![tool.clone()],
            prompts: Vec::new(),
            instructions: Some("guide </server> & <mcp_instructions>".into()),
            resources: false,
            environment: None,
        };
        let mut visible = ToolDef {
            scope: cyber_server::runtime::ToolScope::Mcp,
            deferred: true,
            registration: Some(tool_binding(&published.id, &tool)),
            spec: tool.spec(),
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let block = published
            .instruction_block("server", &[visible.clone()])
            .unwrap();
        assert!(block.contains("guide &lt;/server&gt; &amp; &lt;mcp_instructions&gt;"));
        assert_eq!(block.matches("</server>").count(), 1);
        visible.scope = cyber_server::runtime::ToolScope::Session;
        assert!(
            published
                .instruction_block("server", &[visible.clone()])
                .is_none()
        );
        visible.scope = cyber_server::runtime::ToolScope::Mcp;
        visible.registration = Some("replacement".into());
        assert!(
            published
                .instruction_block("server", &[visible.clone()])
                .is_none()
        );
        visible.registration = Some(tool_binding(&published.id, &tool));
        published.tools[0].definition["description"] = json!("changed");
        assert!(published.instruction_block("server", &[visible]).is_none());
        assert!(published.instruction_block("server", &[]).is_none());
    }

    #[tokio::test]
    async fn disposed_shutdown_retains_startup_until_retry_joins_it() {
        let (release, blocked) = tokio::sync::oneshot::channel::<()>();
        let entry = Entry {
            name: "pending".into(),
            digest: String::new(),
            cancel: CancellationToken::new(),
            published: Mutex::default(),
            server: tokio::sync::Mutex::default(),
            observed: Mutex::default(),
            monitor: tokio::sync::Mutex::default(),
            startup_finished: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            replacing: AtomicBool::new(false),
            transport: Mutex::default(),
            info: None,
            task: tokio::sync::Mutex::new(Some(tokio::spawn(async move {
                let _ = blocked.await;
            }))),
        };
        let mut first = Box::pin(entry.join_startup());
        assert!(futures::poll!(first.as_mut()).is_pending());
        drop(first);
        assert!(entry.task.lock().await.as_ref().is_some());
        let mut retry = Box::pin(entry.join_startup());
        assert!(futures::poll!(retry.as_mut()).is_pending());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), retry)
            .await
            .unwrap();
        assert!(entry.task.lock().await.is_none());
        entry.join_startup().await;
    }
    #[tokio::test(start_paused = true)]
    async fn retry_driver_obeys_exponential_cap_and_ten_attempt_limit() {
        let start = tokio::time::Instant::now();
        let mut at = Vec::new();
        assert!(
            !retry_with_backoff(&CancellationToken::new(), || {
                at.push(start.elapsed().as_secs());
                std::future::ready(Attempt::Retry)
            })
            .await
        );
        assert_eq!(at, vec![1, 3, 7, 15, 31, 63, 123, 183, 243, 303]);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_driver_stops_on_success_or_unsafe_failure() {
        for success in [true, false] {
            let start = tokio::time::Instant::now();
            let mut calls = 0;
            let result = retry_with_backoff(&CancellationToken::new(), || {
                calls += 1;
                std::future::ready(if success {
                    Attempt::Connected
                } else {
                    Attempt::Stop
                })
            })
            .await;
            assert_eq!(result, success);
            assert_eq!(calls, 1);
            assert_eq!(start.elapsed().as_secs(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn authentication_failure_stops_reconnection_after_one_attempt() {
        let mut error = LocalLaunchError::before_launch("authorization required (HTTP 401)");
        error.settled = true;
        assert!(matches!(failed_attempt(&error), Attempt::Retry));
        for kind in [
            super::super::LaunchFailure::NeedsAuth,
            super::super::LaunchFailure::NeedsClientRegistration,
        ] {
            error.kind = kind;
            let mut calls = 0;
            let connected = retry_with_backoff(&CancellationToken::new(), || {
                calls += 1;
                std::future::ready(failed_attempt(&error))
            })
            .await;
            assert!(!connected);
            assert_eq!(calls, 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_driver_cancellation_prevents_attempts_and_disposes_pending_admission() {
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            retry_with_backoff(&token, || async {
                panic!("backoff cancellation must prevent admission")
            })
            .await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(!task.await.unwrap());
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let mut entered = Some(entered);
        let task = tokio::spawn(async move {
            retry_with_backoff(&token, || {
                entered.take().unwrap().send(()).unwrap();
                std::future::pending::<Attempt>()
            })
            .await
        });
        started.await.unwrap();
        cancel.cancel();
        assert!(!task.await.unwrap());
    }
}
