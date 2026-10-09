//! Shared Location connections with retained startup tasks and native owners.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_core::config::{McpServer, McpSettings};
use cyber_server::runtime::{
    Invocation, RetrySafety, SessionInfo, ToolDef, ToolOutcome, TurnContext,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{DiscoveredTool, LocalLauncher, OwnedLocalServer, authorize_server};
use crate::host::BuiltinHost;
use crate::tools::ToolError;

#[derive(Default)]
pub(crate) struct Pool(Mutex<State>);
#[derive(Default)]
struct State {
    closed: bool,
    entries: BTreeMap<(PathBuf, String), Arc<Entry>>,
}
struct Entry {
    name: String,
    digest: String,
    cancel: CancellationToken,
    published: Mutex<Option<(String, Vec<DiscoveredTool>)>>,
    server: tokio::sync::Mutex<Option<OwnedLocalServer>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl BuiltinHost {
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
        if pool.closed {
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
                task: Mutex::default(),
            });
            let owned = entry.clone();
            let host = host.clone();
            let info = info.clone();
            let resolved = resolved.clone();
            let trust = config.trust.clone();
            let directory = directory.clone();
            let task = executor.spawn(async move {
                let credentials = host
                    .opts
                    .models
                    .as_ref()
                    .map(|models| models.credential_env_names())
                    .unwrap_or_default();
                let launcher = LocalLauncher {
                    resolved: &resolved,
                    trust: &trust,
                    location: &directory,
                    home: &host.opts.home,
                    temp_dir: &host.opts.temp_dir,
                    helper: host.opts.sandbox_helper.as_deref(),
                    credential_env_names: &credentials,
                };
                let connect = launcher.connect_owned(&name, host.opts.store.clone(), |id| {
                    host.claim_mcp_location(&info, id, owned.cancel.child_token())
                });
                let result = tokio::select! {
                    result = connect => Some(result),
                    _ = owned.cancel.cancelled() => None,
                };
                match result {
                    Some(Ok(server)) => {
                        *owned
                            .published
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) =
                            Some((server.record().id.clone(), server.tools().to_vec()));
                        *owned.server.lock().await = Some(server);
                    }
                    Some(Err(error)) => cyber_core::log::error(
                        "mcp",
                        &error.diagnostic,
                        json!({"server":name,"acknowledged":error.acknowledged}),
                    ),
                    None => {}
                }
            });
            *entry.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
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
        if pool.closed {
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
            if let Some((id, tools)) = &*published {
                definitions.extend(
                    tools
                        .iter()
                        .filter(|tool| {
                            crate::permissions::Mode::parse(&turn.mode)
                                != crate::permissions::Mode::Plan
                                || tool.read_only_hint()
                        })
                        .map(|tool| ToolDef {
                            registration: Some(format!("{id}:{}", tool.remote_name)),
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

    pub(crate) async fn shutdown_mcp(&self) {
        let entries: Vec<_> = {
            let mut pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
            pool.closed = true;
            pool.entries.values().cloned().collect()
        };
        for entry in &entries {
            entry.cancel.cancel();
        }
        for entry in entries {
            *entry
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            let task = entry
                .task
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(task) = task {
                let _ = task.await;
            }
            let mut server = entry.server.lock().await;
            if let Some(owner) = server.as_mut() {
                match owner.shutdown().await {
                    Ok(_) => {
                        server.take();
                    }
                    Err(error) => cyber_core::log::error(
                        "mcp",
                        &error.diagnostic,
                        json!({"server":entry.name,"acknowledged":error.acknowledged}),
                    ),
                }
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
            registration: inv.registration.clone(),
            spec: tool.spec(),
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let metadata = json!({"server":entry.name,"annotations":tool.definition["annotations"]});
        self.execute_registered(
            inv,
            definition,
            tool.read_only_hint(),
            metadata,
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
            .tool_timeout;
        let result = tokio::select! {
            result = owner.call_exposed_tool(&inv.name, inv.input.clone(), Duration::from_secs(timeout.into())) => result.map_err(|error| ToolError::Failed(error.to_string())),
            _ = cancel.cancelled() => Err(ToolError::Aborted),
        };
        if owner.unresolved() {
            entry.cancel.cancel();
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
        drop(server);
        result
    }

    fn mcp_binding(&self, inv: &Invocation) -> Result<(Arc<Entry>, DiscoveredTool), ToolError> {
        let stale = || ToolError::Failed(format!("Stale tool call: {}", inv.name));
        let directory = Path::new(&inv.directory)
            .canonicalize()
            .map_err(|_| stale())?;
        let config = self.hook_config.get().ok_or_else(stale)?;
        let resolved = (config.resolve)(&directory).map_err(ToolError::Failed)?;
        let pool = self.mcp.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.closed {
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
            let Some((id, tools)) = &*published else {
                continue;
            };
            let Some(tool) = tools.iter().find(|tool| tool.exposed_name == inv.name) else {
                continue;
            };
            if inv.registration.as_deref() != Some(&format!("{id}:{}", tool.remote_name)) {
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
