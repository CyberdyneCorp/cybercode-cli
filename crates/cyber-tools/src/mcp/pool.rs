//! Shared Location connections with retained startup tasks and native owners.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_core::config::{McpServer, McpSettings};
use cyber_server::runtime::{
    Invocation, McpConnectionPhase, McpConnectionStatus, McpServerStatus, McpStatusUpdate,
    RetrySafety, SessionInfo, ToolDef, ToolOutcome, TurnContext, mcp_connections,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{DiscoveredTool, LocalLaunchError, LocalLauncher, OwnedLocalServer, authorize_server};
use crate::host::BuiltinHost;
use crate::tools::ToolError;

#[derive(Default)]
pub(crate) struct Pool(Mutex<State>);
#[derive(Default)]
struct State {
    closed: bool,
    closing: BTreeSet<PathBuf>,
    entries: BTreeMap<(PathBuf, String), Arc<Entry>>,
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
}

struct Published {
    id: String,
    tools: Vec<DiscoveredTool>,
    instructions: Option<String>,
}
impl Published {
    fn from_server(server: &OwnedLocalServer) -> Self {
        Self {
            id: server.record().id.clone(),
            tools: server.tools().to_vec(),
            instructions: server.metadata()["instructions"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned),
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

async fn join_retained(task: &tokio::sync::Mutex<Option<JoinHandle<()>>>) {
    let mut task = task.lock().await;
    if let Some(handle) = task.as_mut() {
        // Disposal releases the lock while retaining the original task for retry.
        let _ = handle.await;
        task.take();
    }
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
            publish_server(entry, server).await;
            Attempt::Connected
        }
        Err(error) => {
            cyber_core::log::error(
                "mcp",
                &error.diagnostic,
                json!({"server":entry.name,"acknowledged":error.acknowledged,"retry_safe":error.retry_safe()}),
            );
            if error.retry_safe() {
                Attempt::Retry
            } else {
                Attempt::Stop
            }
        }
    }
}

async fn publish_server(entry: &Entry, server: OwnedLocalServer) {
    entry.lost.store(false, Ordering::Release);
    *entry
        .published
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(Published::from_server(&server));
    *entry.server.lock().await = Some(server);
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
    launcher
        .connect_owned_observed(&entry.name, host.opts.store.clone(), Some(observer), |id| {
            host.claim_mcp_location(info, id, entry.cancel.child_token())
        })
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
        Ok(update) if update.tools_changed => match refresh_catalog(host, entry, owner).await {
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
        Ok(_) => {
            if let Err(error) = std::fs::remove_dir_all(owner.scratch_path())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                cyber_core::log::error("mcp", &error.to_string(), json!({"server":entry.name}));
                return Health::Stop;
            }
            server.take();
            Health::Retry
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

async fn refresh_catalog(
    host: &std::sync::Weak<BuiltinHost>,
    entry: &Entry,
    owner: &mut OwnedLocalServer,
) -> Result<(), String> {
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
    let refresh = owner.refresh_tools(Duration::from_secs(selected.definition.timeout().into()));
    tokio::select! {
        result = refresh => result.map_err(|error| error.to_string())?,
        _ = entry.cancel.cancelled() => return Err("MCP refresh interrupted".into()),
    }
    *entry
        .published
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(Published::from_server(owner));
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
                );
                let (status, error) = match problem {
                    Some((status, error)) => (status, Some(error.into())),
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
        if pool.closed || pool.closing.contains(&directory) {
            return;
        }
        for (name, definition) in settings.servers {
            if !matches!(definition, McpServer::Local { .. }) || !definition.enabled() {
                continue;
            }
            let key = (directory.clone(), name.clone());
            if pool.entries.contains_key(&key) {
                continue;
            }
            let Ok(selection) = authorize_server(&resolved, &config.trust, &directory, &name)
            else {
                continue;
            };
            let entry = Arc::new(Entry {
                name: name.clone(),
                digest: selection.digest,
                cancel: CancellationToken::new(),
                published: Mutex::default(),
                server: tokio::sync::Mutex::new(None),
                task: tokio::sync::Mutex::default(),
                observed: Mutex::default(),
                monitor: tokio::sync::Mutex::default(),
                startup_finished: AtomicBool::new(false),
                lost: AtomicBool::new(false),
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
                        publish_server(&owned, server).await;
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
            pool.entries.insert(key, entry);
        }
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
            if let Some(Published { id, tools, .. }) = &*published {
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

    pub(super) fn mcp_is_published(&self, directory: &Path, name: &str, id: &str) -> bool {
        let Ok(directory) = directory.canonicalize() else {
            return false;
        };
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.closed || pool.closing.contains(&directory) {
            return false;
        }
        pool.entries
            .get(&(directory, name.into()))
            .is_some_and(|entry| {
                !entry.cancel.is_cancelled()
                    && !entry.lost.load(Ordering::Acquire)
                    && entry
                        .published
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_ref()
                        .is_some_and(|published| published.id == id)
            })
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
        let context =
            super::ElicitationContext::new(&inv.asker, &authorize).with_sampling(&sampler);
        let result = tokio::select! {
            result = owner.call_exposed_tool_with_elicitation(&inv.name, inv.input.clone(), Duration::from_secs(timeout.into()), &context) => result.map_err(|error| ToolError::Failed(error.to_string())),
            _ = cancel.cancelled() => Err(ToolError::Aborted),
            _ = entry.cancel.cancelled() => Err(ToolError::Aborted),
        };
        let sampling_settlement = sampler.finish().await;
        if owner.unresolved() {
            entry.lost.store(true, Ordering::Release);
            *entry
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            if let Err(error) = owner.shutdown().await {
                cyber_core::log::error(
                    "mcp",
                    &error.diagnostic,
                    json!({"server":entry.name,"acknowledged":error.acknowledged}),
                );
            }
        }
        inv.asker
            .cancel_questions()
            .await
            .map_err(|_| ToolError::Failed("MCP question cleanup failed".into()))?;
        drop(server);
        sampling_settlement.map_err(|error| ToolError::Failed(error.into()))?;
        result
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
            let Some(Published { id, tools, .. }) = &*published else {
                continue;
            };
            let Some(tool) = tools.iter().find(|tool| tool.exposed_name == inv.name) else {
                continue;
            };
            if inv.registration.as_deref() != Some(tool_binding(id, tool).as_str()) {
                return Err(stale());
            }
            let selection = authorize_server(&resolved, &config.trust, &directory, &entry.name)
                .map_err(ToolError::Failed)?;
            if selection.digest != entry.digest {
                return Err(stale());
            }
            return Ok((entry.clone(), tool.clone()));
        }
        Err(stale())
    }
}

fn status_problem(
    definition: Option<&McpServer>,
    entry: Option<&Arc<Entry>>,
    connection: &Option<McpStatusUpdate>,
    closing: bool,
    authorized: impl FnOnce() -> bool,
) -> Option<(McpConnectionStatus, &'static str)> {
    let failed = |message| Some((McpConnectionStatus::Failed, message));
    let Some(definition) = definition else {
        return failed("MCP server is absent from loaded configuration");
    };
    if !definition.enabled() {
        return Some((McpConnectionStatus::Disabled, "MCP server is disabled"));
    }
    if !authorized() {
        return failed("MCP server definition is not authorized");
    }
    if matches!(definition, McpServer::Remote { .. }) {
        return failed("Remote MCP transport is not available");
    }
    if closing {
        return failed("MCP Location is closing or requires recovery");
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
            instructions: Some("guide </server> & <mcp_instructions>".into()),
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
