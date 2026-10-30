//! The plugin host runtime (`plugins-marketplace` → Plugin host process, Host protocol,
//! Restart and backoff, Scoped registrations, Capability declaration and enforcement,
//! User configuration).
//!
//! A loaded plugin whose manifest declares `main` runs in its own host process per
//! Location, started when a Session opens the Location and reconciled before every
//! model step. The core never loads plugin code in-process: it speaks JSON-RPC 2.0 to the
//! host over stdio. `initialize` negotiates the protocol version and returns the tools
//! the plugin contributes; each call is forwarded with `tool/execute`. Every tool is
//! owned by its plugin's entry: disabling, removing, updating or crashing the plugin
//! removes exactly its tools from the next step's list, without a server restart.

mod connection;
mod environment;
mod host;
mod launch;
mod lifetime;
pub(crate) mod network;
mod restart;
mod runtime_search;
pub mod secrets;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cyber_core::config::Resolved;
use cyber_core::plugins::{HostEntry, HostState, Manifest, PluginSettings, PluginState};
use cyber_llm::ToolSpec;
use cyber_server::runtime::Asker;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use connection::{CallError, Connection, Handler, RpcError};
use launch::Denials;
use restart::{AfterCrash, Crashes};

pub use secrets::{
    PluginSecrets, VaultSecrets, hide_computer_artifacts, hide_credential_files, open_secrets,
};

/// The plugin host protocol version this core speaks.
pub const PROTOCOL_VERSION: &str = "1";
/// How long a host may take to answer `initialize`.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a host may take to exit after `shutdown`.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Every plugin host this process runs, by Location and plugin name.
#[derive(Default)]
pub(crate) struct PluginHosts {
    entries: Mutex<BTreeMap<(PathBuf, String), Arc<Entry>>>,
    /// Plugins a crash loop disabled, by Location, name and content digest: they stay off
    /// for the rest of the process even if they are disabled and enabled again.
    disabled: Mutex<HashMap<(PathBuf, String, String), String>>,
    closed: AtomicBool,
    /// The first restart delay in milliseconds; 0 is the 1 s default.
    restart_base_ms: AtomicU64,
    secrets: Mutex<Option<Arc<dyn PluginSecrets>>>,
}

impl PluginHosts {
    fn restart_base(&self) -> Duration {
        match self.restart_base_ms.load(Ordering::Relaxed) {
            0 => restart::BASE,
            ms => Duration::from_millis(ms),
        }
    }
}

/// A loaded plugin with a host entry, as the current configuration resolves it.
#[derive(Debug, Clone)]
pub(crate) struct Hosted {
    pub name: String,
    pub scope: cyber_core::plugins::PluginScope,
    pub version: String,
    pub digest: String,
    pub root: PathBuf,
    pub manifest: Manifest,
    pub host: HostEntry,
}

/// The loaded plugins of `resolved` that run a host, in name order.
pub(crate) fn hosted(resolved: &Resolved) -> Vec<Hosted> {
    let mut out: Vec<Hosted> = resolved
        .plugins
        .iter()
        .filter(|status| status.state == PluginState::Active && status.host.is_some())
        .filter_map(|status| {
            let root = status.root.clone()?;
            let manifest = Manifest::read(&root).ok()?;
            let version = status.version.clone()?;
            if manifest.name != status.name || manifest.version != version {
                return None;
            }
            Some(Hosted {
                name: status.name.clone(),
                scope: status.scope,
                version,
                digest: status.digest.clone()?,
                root,
                host: manifest.host.clone()?,
                manifest,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A tool a host contributed.
#[derive(Debug, Clone)]
pub(crate) struct PluginTool {
    pub spec: ToolSpec,
    pub annotations: Value,
    pub read_only: bool,
    /// Plugin, content digest and the tool's definition: a call advertised for another
    /// registration is stale.
    pub registration: String,
}

/// A running host.
pub(crate) struct Live {
    connection: Arc<Connection>,
    tools: Vec<PluginTool>,
    started: Instant,
}

enum State {
    Starting,
    Ready(Arc<Live>),
    Restarting(String),
    Disabled(String),
    Failed(String),
}

/// One plugin host of one Location.
pub(crate) struct Entry {
    name: String,
    digest: String,
    location: PathBuf,
    state: Mutex<State>,
    stop: CancellationToken,
    denials: Denials,
    crashes: Mutex<Crashes>,
    /// The digest of the non-secret settings the host was last given.
    settings: Mutex<String>,
    /// Askers of this plugin's calls in flight, for `permission/ask`.
    calls: Mutex<HashMap<String, Asker>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Entry {
    fn set(&self, state: State) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }

    fn live(&self) -> Result<Arc<Live>, String> {
        match &*self.state.lock().unwrap_or_else(PoisonError::into_inner) {
            State::Ready(live) => Ok(Arc::clone(live)),
            State::Starting => Err(format!("plugin {} is starting", self.name)),
            State::Restarting(_) => {
                Err(format!("plugin {} is restarting after a crash", self.name))
            }
            State::Disabled(reason) | State::Failed(reason) => Err(reason.clone()),
        }
    }

    /// A host that could not start, to start again: always with `retry_failed`, else
    /// only once its configured settings (`settings`) changed.
    fn retry(&self, settings: &Value, retry_failed: bool) -> bool {
        let failed = matches!(
            *self.state.lock().unwrap_or_else(PoisonError::into_inner),
            State::Failed(_)
        );
        failed
            && (retry_failed
                || *self.settings.lock().unwrap_or_else(PoisonError::into_inner)
                    != settings_digest(settings))
    }

    fn starting(&self) -> bool {
        matches!(
            *self.state.lock().unwrap_or_else(PoisonError::into_inner),
            State::Starting
        )
    }

    fn status(&self, status: &mut cyber_core::plugins::HostStatus) {
        let (state, error, tools) =
            match &*self.state.lock().unwrap_or_else(PoisonError::into_inner) {
                State::Starting => (HostState::Starting, None, Vec::new()),
                State::Ready(live) => (
                    HostState::Running,
                    None,
                    live.tools
                        .iter()
                        .map(|tool| tool.spec.name.clone())
                        .collect(),
                ),
                State::Restarting(error) => {
                    (HostState::Restarting, Some(error.clone()), Vec::new())
                }
                State::Disabled(error) => (HostState::Disabled, Some(error.clone()), Vec::new()),
                State::Failed(error) => (HostState::Failed, Some(error.clone()), Vec::new()),
            };
        status.state = state;
        status.error = error;
        status.tools = tools;
        status.crashes = self
            .crashes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .count();
    }
}

/// What a host run needs from the tool host, without holding it.
pub(crate) struct RunContext {
    pub resolve: Arc<crate::HookConfigFn>,
    pub home: PathBuf,
    pub temp_dir: PathBuf,
    pub helper: Option<PathBuf>,
    pub secrets: Option<Arc<dyn PluginSecrets>>,
    pub restart_base: Duration,
    pub hosts: std::sync::Weak<crate::BuiltinHost>,
}

/// Why a start produced no running host.
enum StartError {
    /// Restarting cannot help: report it and stay down.
    Fatal(String),
    /// The host died or hung while starting; it counts as a crash.
    Crashed(String),
}

/// Start, watch and restart one plugin host until it is stopped or gives up.
async fn run(context: Arc<RunContext>, entry: Arc<Entry>) {
    while !entry.stop.is_cancelled() {
        entry.set(State::Starting);
        let crash = match start(&context, &entry).await {
            Ok((live, started)) => supervise(&entry, live, started).await,
            Err(StartError::Fatal(error)) => {
                cyber_core::log::warn("plugins", &error, json!({"plugin": entry.name}));
                entry.set(State::Failed(error));
                return;
            }
            Err(StartError::Crashed(error)) => Some((error, Duration::ZERO)),
        };
        let Some((error, uptime)) = crash else {
            return;
        };
        if !crashed(&context, &entry, error, uptime).await {
            return;
        }
    }
}

/// Offer a started host until it is stopped (`None`) or exits on its own: then its
/// resources are released and the crash and uptime are returned.
async fn supervise(
    entry: &Entry,
    live: Arc<Live>,
    mut started: launch::Started,
) -> Option<(String, Duration)> {
    entry.set(State::Ready(Arc::clone(&live)));
    let stopped = tokio::select! {
        _ = live.connection.closed.cancelled() => false,
        _ = started.wait() => false,
        _ = entry.stop.cancelled() => true,
    };
    if stopped {
        shutdown(&live, started).await;
        return None;
    }
    live.connection.closed.cancel();
    if started.finish().await.is_err() {
        entry.set(State::Failed(format!(
            "plugin {} cleanup is unconfirmed; resources retained",
            entry.name
        )));
        return None;
    }
    let error = format!("plugin {} crashed", entry.name);
    Some((error, live.started.elapsed()))
}

/// Record a crash; wait out the backoff and report whether to start again.
async fn crashed(context: &RunContext, entry: &Entry, error: String, uptime: Duration) -> bool {
    let after = entry
        .crashes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(Instant::now(), uptime, context.restart_base);
    match after {
        AfterCrash::Disable => {
            let reason = format!("plugin {} disabled after repeated crashes", entry.name);
            cyber_core::log::warn("plugins", &reason, json!({"plugin": entry.name}));
            if let Some(host) = context.hosts.upgrade() {
                host.plugins
                    .disabled
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        (
                            entry.location.clone(),
                            entry.name.clone(),
                            entry.digest.clone(),
                        ),
                        reason.clone(),
                    );
            }
            entry.set(State::Disabled(reason));
            false
        }
        AfterCrash::RestartIn(delay) => {
            cyber_core::log::warn(
                "plugins",
                &error,
                json!({"plugin": entry.name, "restart_ms": delay.as_millis() as u64}),
            );
            entry.set(State::Restarting(error));
            tokio::select! {
                _ = tokio::time::sleep(delay) => true,
                _ = entry.stop.cancelled() => false,
            }
        }
    }
}

/// Ask the host to shut down, then end it.
async fn shutdown(live: &Live, mut started: launch::Started) {
    let _ = live
        .connection
        .request(
            "shutdown",
            json!({}),
            &CancellationToken::new(),
            Some(SHUTDOWN_GRACE),
        )
        .await;
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, started.wait()).await;
    live.connection.closed.cancel();
    let _ = started.finish().await;
}

async fn start(
    context: &RunContext,
    entry: &Arc<Entry>,
) -> Result<(Arc<Live>, launch::Started), StartError> {
    let resolved = (context.resolve)(&entry.location).map_err(StartError::Fatal)?;
    let plugin = hosted(&resolved)
        .into_iter()
        .find(|plugin| plugin.name == entry.name && plugin.digest == entry.digest)
        .ok_or_else(|| StartError::Fatal(format!("plugin {} is no longer loaded", entry.name)))?;
    // Recorded before validation, so a host refused for its settings is retried only
    // once they change.
    let configured = resolved
        .value
        .pointer(&format!("/plugins/{}/config", plugin.name))
        .cloned()
        .unwrap_or_else(|| json!({}));
    *entry
        .settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = settings_digest(&configured);
    let settings =
        settings(context, &plugin, &resolved, &entry.location).map_err(StartError::Fatal)?;
    let launcher = launch::LaunchContext {
        resolved: &resolved,
        location: &entry.location,
        home: &context.home,
        temp_dir: &context.temp_dir,
        helper: context.helper.as_deref(),
    };
    let request = launch::PluginLaunch {
        name: &plugin.name,
        root: &plugin.root,
        host: &plugin.host,
        capabilities: &plugin.manifest.capabilities,
    };
    let mut started = launcher
        .start(&request, &entry.denials)
        .await
        .map_err(StartError::Fatal)?;
    let (Some(stdin), Some(stdout)) = (started.stdin(), started.stdout()) else {
        return Err(StartError::Fatal(format!(
            "plugin {}: host streams unavailable",
            plugin.name
        )));
    };
    if let Some(stderr) = started.stderr() {
        tokio::spawn(log_stderr(plugin.name.clone(), stderr));
    }
    let handler = handler(entry, &plugin, settings.secrets);
    let connection = Connection::start(stdin, stdout, handler);
    // An aborted initialization must close its protocol scope too, even if an
    // in-flight handler temporarily retains another Arc to the connection.
    let initializing = connection.closed.clone().drop_guard();
    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "core": {"name": "cyber", "version": cyber_core::version::build_info().version},
        "plugin": {"name": plugin.name, "version": plugin.version, "root": plugin.root},
        "location": {"directory": entry.location},
        "capabilities": plugin.manifest.capabilities,
        "config": settings.config,
    });
    let result = connection
        .request("initialize", params, &entry.stop, Some(STARTUP_TIMEOUT))
        .await;
    let tools = match result {
        Ok(result) => initialized(&plugin, &result).map_err(StartError::Fatal)?,
        Err(CallError::Rpc(error)) => return Err(StartError::Fatal(refused(&plugin.name, &error))),
        Err(CallError::Crashed) => {
            return Err(StartError::Crashed(format!(
                "plugin {} crashed during initialize",
                plugin.name
            )));
        }
        Err(CallError::TimedOut) => {
            return Err(StartError::Crashed(format!(
                "plugin {} did not answer initialize within {} s",
                plugin.name,
                STARTUP_TIMEOUT.as_secs()
            )));
        }
        Err(CallError::Cancelled) => {
            return Err(StartError::Fatal(format!("plugin {} stopped", plugin.name)));
        }
    };
    let live = Arc::new(Live {
        connection,
        tools,
        started: Instant::now(),
    });
    initializing.disarm();
    Ok((live, started))
}

/// An `initialize` error; a version refusal reports both versions.
fn refused(name: &str, error: &RpcError) -> String {
    let theirs = error
        .data
        .as_ref()
        .and_then(|data| {
            data.get("protocolVersion")
                .or_else(|| data.get("supported"))
        })
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    match theirs {
        Some(theirs) => mismatch(name, &theirs),
        None => format!("plugin {name} initialize failed: {}", error.message),
    }
}

fn mismatch(name: &str, theirs: &str) -> String {
    format!(
        "plugin {name} protocol version mismatch: the plugin speaks {theirs}, cyber speaks {PROTOCOL_VERSION}"
    )
}

/// The tools of an `initialize` result; contributions this release cannot run are
/// reported and ignored.
fn initialized(plugin: &Hosted, result: &Value) -> Result<Vec<PluginTool>, String> {
    let version = match &result["protocolVersion"] {
        Value::String(version) => version.clone(),
        Value::Null => "none".into(),
        other => other.to_string(),
    };
    if version != PROTOCOL_VERSION {
        return Err(mismatch(&plugin.name, &version));
    }
    for kind in ["hooks", "providers", "catalogTransforms"] {
        if result[kind]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            cyber_core::log::warn(
                "plugins",
                &format!(
                    "plugin {}: {kind} contributed over the host protocol are not supported yet",
                    plugin.name
                ),
                json!({"plugin": plugin.name}),
            );
        }
    }
    let mut tools = Vec::new();
    let mut names = BTreeSet::new();
    for tool in result["tools"].as_array().into_iter().flatten() {
        match plugin_tool(plugin, tool) {
            Ok(tool) if names.insert(tool.spec.name.clone()) => tools.push(tool),
            Ok(tool) => warn_tool(plugin, &format!("duplicate tool {}", tool.spec.name)),
            Err(error) => warn_tool(plugin, &error),
        }
    }
    Ok(tools)
}

fn warn_tool(plugin: &Hosted, error: &str) {
    cyber_core::log::warn(
        "plugins",
        &format!(
            "plugin {}: {error}; the tool was not registered",
            plugin.name
        ),
        json!({"plugin": plugin.name}),
    );
}

fn plugin_tool(plugin: &Hosted, tool: &Value) -> Result<PluginTool, String> {
    let name = tool["name"].as_str().ok_or("a tool has no name")?;
    if !valid_tool_name(name) {
        return Err(format!("invalid tool name {name:?}"));
    }
    let description = match &tool["description"] {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => return Err(format!("tool {name}: description must be a string")),
    };
    let input_schema = match tool.get("inputSchema") {
        None => json!({"type": "object"}),
        Some(schema) if schema["type"] == "object" => schema.clone(),
        Some(_) => return Err(format!("tool {name}: inputSchema must be an object schema")),
    };
    let annotations = match tool.get("annotations") {
        None => json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(_) => return Err(format!("tool {name}: annotations must be an object")),
    };
    let read_only = annotations["readOnlyHint"] == true && confined(&plugin.manifest.capabilities);
    let spec = ToolSpec {
        name: name.to_owned(),
        description,
        input_schema,
    };
    let definition = json!({"spec": [&spec.name, &spec.description, &spec.input_schema], "annotations": annotations});
    let registration = format!(
        "plugin:{}:{}:{}",
        plugin.name,
        plugin.digest,
        short_digest(&cyber_core::config::canonical_json(&definition))
    );
    Ok(PluginTool {
        spec,
        annotations,
        read_only,
        registration,
    })
}

/// Whether a plugin's host can have no effect outside its scratch directory: no
/// writable Location, network, shell or prompts. Only then is a tool's own
/// `readOnlyHint` believed (for plan mode and default-mode auto-allow); the host of an
/// `fs.write` plugin can write the workspace whatever its tools claim.
fn confined(capabilities: &cyber_core::plugins::Capabilities) -> bool {
    !capabilities.fs_write
        && !capabilities.shell
        && !capabilities.session_prompt
        && matches!(
            capabilities.network,
            cyber_core::plugins::NetworkCapability::None
        )
}

fn short_digest(text: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))[..16].to_owned()
}

/// `^[A-Za-z][A-Za-z0-9_-]{0,63}$`, and never the MCP namespace.
fn valid_tool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !name.starts_with("mcp__")
        && (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

/// Resolve the plugin's validated settings, reading its secrets from the store.
fn settings(
    context: &RunContext,
    plugin: &Hosted,
    resolved: &Resolved,
    location: &std::path::Path,
) -> Result<PluginSettings, String> {
    let schema = plugin.manifest.user_config.as_ref();
    let secrets = context.secrets.clone();
    let owner = cyber_core::plugins::secret_owner(&plugin.name, plugin.scope, location);
    let lookup = move |key: &str| match &secrets {
        Some(store) => store.get(&owner, key),
        None => Err("no plugin secret store is available".into()),
    };
    cyber_core::plugins::resolve_settings(&plugin.name, schema, &resolved.value, &lookup)
}

fn settings_digest(config: &Value) -> String {
    short_digest(&cyber_core::config::canonical_json(config))
}

/// Answers the host's requests: `log`, `secrets/get`, `permission/ask` and
/// `session/prompt`, each gated by the declared capabilities.
fn handler(entry: &Arc<Entry>, plugin: &Hosted, secrets: BTreeMap<String, String>) -> Handler {
    let weak = Arc::downgrade(entry);
    let name = plugin.name.clone();
    let capabilities = plugin.manifest.capabilities.clone();
    let secrets = Arc::new(secrets);
    Arc::new(move |method, params| {
        let (weak, name, secrets) = (weak.clone(), name.clone(), Arc::clone(&secrets));
        let capabilities = capabilities.clone();
        Box::pin(async move {
            match method.as_str() {
                "log" => {
                    log(&name, &params);
                    Ok(Value::Null)
                }
                "secrets/get" if !capabilities.secrets => Err(undeclared(&name, "secrets")),
                "secrets/get" => {
                    let key = params["key"].as_str().unwrap_or_default();
                    secrets
                        .get(key)
                        .map(|value| json!({"value": value}))
                        .ok_or_else(|| RpcError::new(-32002, format!("no secret {key:?} is set")))
                }
                "permission/ask" => ask(&weak, &name, &params).await,
                "session/prompt" if !capabilities.session_prompt => {
                    Err(undeclared(&name, "session.prompt"))
                }
                "session/prompt" => Err(RpcError::new(
                    -32601,
                    "session/prompt is not supported by this release",
                )),
                _ => Err(RpcError::new(-32601, format!("unknown method {method}"))),
            }
        })
    })
}

fn undeclared(name: &str, capability: &str) -> RpcError {
    RpcError::new(
        -32001,
        format!("plugin {name} does not declare the {capability} capability"),
    )
}

fn log(name: &str, params: &Value) {
    let message = params["message"].as_str().unwrap_or_default();
    let level = params["level"]
        .as_str()
        .and_then(cyber_core::log::Level::parse)
        .unwrap_or(cyber_core::log::Level::Info);
    cyber_core::log::log(
        level,
        "plugins",
        &format!("plugin {name}: {message}"),
        json!({"plugin": name}),
    );
}

/// `permission/ask` during one of this plugin's calls: the user's answer; an unattended
/// run is a denial.
async fn ask(
    entry: &std::sync::Weak<Entry>,
    name: &str,
    params: &Value,
) -> Result<Value, RpcError> {
    let call = params["callId"].as_str().unwrap_or_default();
    let asker = entry
        .upgrade()
        .and_then(|entry| {
            entry
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(call)
                .cloned()
        })
        .ok_or_else(|| RpcError::new(-32003, "permission/ask needs a tool call in progress"))?;
    let resource = params["resource"].as_str().unwrap_or("*").to_owned();
    let reply = asker
        .permission(cyber_server::runtime::PermissionAsk {
            action: format!("plugin:{name}"),
            resources: vec![resource],
            always_patterns: Vec::new(),
            metadata: json!({"plugin": name, "message": params["message"]}),
        })
        .await;
    let granted = matches!(
        reply,
        cyber_server::runtime::PermissionReply::Once
            | cyber_server::runtime::PermissionReply::Always
    );
    Ok(json!({"granted": granted}))
}

async fn log_stderr(name: String, stderr: crate::tools::process::ReadStream) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        cyber_core::log::debug(
            "plugins",
            &format!("plugin {name} stderr: {line}"),
            json!({"plugin": name}),
        );
    }
}

#[cfg(all(test, unix))]
mod settlement_tests;
