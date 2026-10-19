//! The tool host's side of plugin hosts: start and reconcile them per Location, offer
//! their tools and forward calls.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_server::runtime::{
    Invocation, RetrySafety, ToolDef, ToolOutcome, ToolScope, TurnContext,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::connection::CallError;
use super::{Entry, Hosted, PluginSecrets, PluginTool, RunContext, State, hosted};
use crate::host::BuiltinHost;
use crate::registered::RegisteredOptions;

/// A plugin tool offered for a Location, with the entry that runs it.
pub(crate) struct Offered {
    entry: Arc<Entry>,
    tool: PluginTool,
}

impl BuiltinHost {
    /// Bind the store plugin secrets are read from (the OS keyring in production;
    /// embedders and tests bind their own). Hosts started later read from it.
    pub fn attach_plugin_secrets(&self, store: Arc<dyn PluginSecrets>) {
        *self
            .plugins
            .secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(store);
    }

    /// Shorten the first restart delay (1 s); for tests of the restart policy.
    pub fn set_plugin_restart_base(&self, base: Duration) {
        self.plugins
            .restart_base_ms
            .store(base.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// Start hosts of newly loaded plugins and stop those no longer loaded (disabled,
    /// removed, updated or untrusted), so exactly their tools disappear. A host that
    /// could not start is tried again when its settings change and, with
    /// `retry_failed` (the Location opened for a Session or a new prompt), in any case.
    pub(crate) fn reconcile_plugins(&self, directory: &str, retry_failed: bool) {
        let Some(config) = self.hook_config.get() else {
            return;
        };
        let Ok(location) = Path::new(directory).canonicalize() else {
            return;
        };
        if self.plugins.closed.load(Ordering::SeqCst) {
            return;
        }
        // An unresolvable configuration loads no plugin: fail closed.
        let wanted = (config.resolve)(&location)
            .map(|resolved| (hosted(&resolved), resolved))
            .ok();
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut entries = self
            .plugins
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let plugins: &[Hosted] = wanted.as_ref().map_or(&[], |(plugins, _)| plugins);
        entries.retain(|(at, name), entry| {
            let keep = at != &location
                || plugins
                    .iter()
                    .any(|plugin| &plugin.name == name && plugin.digest == entry.digest);
            if !keep {
                entry.stop.cancel();
            }
            keep
        });
        let Some((plugins, resolved)) = &wanted else {
            return;
        };
        for plugin in plugins {
            let key = (location.clone(), plugin.name.clone());
            let settings = raw_settings(&resolved.value, &plugin.name);
            match entries.get(&key) {
                Some(entry) if !entry.retry(&settings, retry_failed) => {
                    notify_settings(entry, settings, &executor);
                }
                _ => {
                    let entry = self.spawn_plugin(&location, plugin, &config.resolve, &executor);
                    if let Some(old) = entries.insert(key, entry) {
                        old.stop.cancel();
                    }
                }
            }
        }
    }

    fn spawn_plugin(
        &self,
        location: &Path,
        plugin: &Hosted,
        resolve: &Arc<crate::HookConfigFn>,
        executor: &tokio::runtime::Handle,
    ) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            name: plugin.name.clone(),
            digest: plugin.digest.clone(),
            location: location.to_path_buf(),
            state: Mutex::new(State::Starting),
            stop: CancellationToken::new(),
            denials: Default::default(),
            crashes: Default::default(),
            settings: Mutex::default(),
            calls: Mutex::default(),
            task: Mutex::default(),
        });
        let disabled = self
            .plugins
            .disabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(
                location.to_path_buf(),
                plugin.name.clone(),
                plugin.digest.clone(),
            ))
            .cloned();
        if let Some(reason) = disabled {
            entry.set(State::Disabled(reason));
            return entry;
        }
        let context = Arc::new(RunContext {
            resolve: Arc::clone(resolve),
            home: self.opts.home.clone(),
            temp_dir: self.opts.temp_dir.clone(),
            helper: self.opts.sandbox_helper.clone(),
            secrets: self
                .plugins
                .secrets
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            restart_base: self.plugins.restart_base(),
            hosts: self.weak.clone(),
        });
        let task = executor.spawn(super::run(context, Arc::clone(&entry)));
        *entry.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
        entry
    }

    /// Wait until every plugin host of the Location has started or given up, so the
    /// first Turn sees their tools.
    pub(crate) async fn wait_plugins(&self, directory: &str, cancel: CancellationToken) {
        let Ok(location) = Path::new(directory).canonicalize() else {
            return;
        };
        let deadline =
            tokio::time::Instant::now() + super::STARTUP_TIMEOUT + Duration::from_secs(1);
        while tokio::time::Instant::now() < deadline && !cancel.is_cancelled() {
            let starting = self
                .plugins
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|((at, _), entry)| at == &location && entry.starting());
            if !starting {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Stop every plugin host and wait for them to exit.
    pub(crate) async fn shutdown_plugins(&self) {
        self.plugins.closed.store(true, Ordering::SeqCst);
        let entries: Vec<Arc<Entry>> = std::mem::take(
            &mut *self
                .plugins
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
        .into_values()
        .collect();
        let mut tasks = Vec::new();
        for entry in entries {
            entry.stop.cancel();
            if let Some(task) = entry
                .task
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                tasks.push(task);
            }
        }
        let all = futures::future::join_all(tasks);
        let _ = tokio::time::timeout(super::SHUTDOWN_GRACE * 2, all).await;
    }

    /// The plugin tools a Location offers now: running hosts of plugins the current
    /// configuration still loads. A name a built-in tool or an earlier plugin uses is
    /// skipped.
    fn offered_plugin_tools(&self, directory: &str) -> Vec<Offered> {
        let Some(config) = self.hook_config.get() else {
            return Vec::new();
        };
        let Ok(location) = Path::new(directory).canonicalize() else {
            return Vec::new();
        };
        let Ok(resolved) = (config.resolve)(&location) else {
            return Vec::new();
        };
        let loaded: BTreeSet<(String, String)> = hosted(&resolved)
            .into_iter()
            .map(|plugin| (plugin.name, plugin.digest))
            .collect();
        let entries = self
            .plugins
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut taken: BTreeSet<String> = self.builtin_tool_names();
        let mut out = Vec::new();
        for ((at, name), entry) in entries.iter() {
            if at != &location || !loaded.contains(&(name.clone(), entry.digest.clone())) {
                continue;
            }
            let Ok(live) = entry.live() else {
                continue;
            };
            for tool in &live.tools {
                if taken.insert(tool.spec.name.clone()) {
                    out.push(Offered {
                        entry: Arc::clone(entry),
                        tool: tool.clone(),
                    });
                }
            }
        }
        out
    }

    pub(crate) fn plugin_definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let plan = crate::permissions::Mode::parse(&turn.mode) == crate::permissions::Mode::Plan;
        self.offered_plugin_tools(&turn.directory)
            .into_iter()
            .filter(|offered| !plan || offered.tool.read_only)
            .map(|offered| definition(&offered.tool))
            .collect()
    }

    /// The offered plugin tool a call names, if any.
    pub(crate) fn plugin_tool(&self, inv: &Invocation) -> Option<Offered> {
        self.offered_plugin_tools(&inv.directory)
            .into_iter()
            .find(|offered| offered.tool.spec.name == inv.name)
    }

    /// Run a plugin tool call through the registered-tool path (schema, hooks,
    /// permissions, output budget), forwarding it with `tool/execute`.
    pub(crate) async fn execute_plugin(
        &self,
        inv: Invocation,
        offered: Offered,
        cancel: CancellationToken,
    ) -> ToolOutcome {
        let Offered { entry, tool } = offered;
        let metadata = json!({"source": "plugin", "plugin": entry.name, "server": entry.name});
        // Network reach is the plugin's declared capability, enforced by its sandbox, so a
        // plugin tool is never gated as open-world.
        let mut annotations = tool.annotations.clone();
        annotations["openWorldHint"] = Value::Bool(false);
        let options = RegisteredOptions {
            read_only: tool.read_only,
            metadata,
            output_token_limit: None,
            annotations,
            network_target: None,
        };
        let definition = definition(&tool);
        self.execute_registered_with_output_limit(
            inv,
            definition,
            options,
            cancel,
            move |inv, cancel| Box::pin(call(entry, inv, cancel)),
        )
        .await
    }

    /// Overlay each running host's status on the configuration's plugin statuses.
    pub(crate) fn plugin_host_status(
        &self,
        location: &Path,
        plugins: &mut [cyber_core::plugins::PluginStatus],
    ) {
        let location: PathBuf = location
            .canonicalize()
            .unwrap_or_else(|_| location.to_path_buf());
        let entries = self
            .plugins
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for status in plugins {
            let Some(host) = status.host.as_mut() else {
                continue;
            };
            let entry = entries
                .get(&(location.clone(), status.name.clone()))
                .filter(|entry| status.digest.as_deref() == Some(entry.digest.as_str()));
            if let Some(entry) = entry {
                entry.status(host);
            }
        }
    }
}

fn definition(tool: &PluginTool) -> ToolDef {
    ToolDef {
        scope: ToolScope::Plugin,
        deferred: false,
        registration: Some(tool.registration.clone()),
        spec: tool.spec.clone(),
        retry_safety: RetrySafety::Never,
        concurrency_safe: false,
    }
}

/// `plugins.<name>.config` as configured (`{}` when absent).
fn raw_settings(config: &Value, name: &str) -> Value {
    config
        .pointer(&format!("/plugins/{name}/config"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// Tell a running host its non-secret settings changed (`config/changed`).
fn notify_settings(entry: &Arc<Entry>, current: Value, executor: &tokio::runtime::Handle) {
    let digest = super::settings_digest(&current);
    let Ok(live) = entry.live() else {
        return;
    };
    {
        let mut known = entry
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *known == digest {
            return;
        }
        *known = digest;
    }
    executor.spawn(async move {
        let _ = live
            .connection
            .notify("config/changed", json!({"config": current}))
            .await;
    });
}

/// Forward one call to the plugin's running host.
async fn call(entry: Arc<Entry>, inv: Invocation, cancel: CancellationToken) -> ToolOutcome {
    let live = match entry.live() {
        Ok(live) => live,
        Err(error) => return ToolOutcome::Failed(error),
    };
    entry
        .calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(inv.call_id.clone(), inv.asker.clone());
    let params = json!({
        "name": inv.name,
        "input": inv.input,
        "callId": inv.call_id,
        "sessionId": inv.session_id,
        "attempt": inv.attempt,
        "operationKey": inv.operation_key,
    });
    let result = live
        .connection
        .request("tool/execute", params, &cancel, None)
        .await;
    entry
        .calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&inv.call_id);
    let outcome = match result {
        Ok(result) => tool_result(&result),
        Err(CallError::Crashed) => ToolOutcome::Failed(format!("plugin {} crashed", entry.name)),
        Err(CallError::Rpc(error)) => ToolOutcome::Failed(error.message),
        Err(CallError::Cancelled) => ToolOutcome::Aborted,
        Err(CallError::TimedOut) => {
            ToolOutcome::Failed(format!("plugin {} did not answer", entry.name))
        }
    };
    with_denials(&entry, outcome)
}

/// A `tool/execute` result: `content` as a string or text parts, `isError`, and an
/// optional `structured` value.
fn tool_result(result: &Value) -> ToolOutcome {
    let text = match &result["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if result["isError"] == true {
        return ToolOutcome::Failed(text);
    }
    match result.get("structured") {
        Some(value) if !value.is_null() => ToolOutcome::Structured {
            output: text,
            value: value.clone(),
        },
        _ => ToolOutcome::Ok(text),
    }
}

/// Append a warning naming the plugin for each host its sandbox refused during the call.
fn with_denials(entry: &Entry, outcome: ToolOutcome) -> ToolOutcome {
    let hosts: Vec<String> =
        std::mem::take(&mut *entry.denials.lock().unwrap_or_else(PoisonError::into_inner));
    if hosts.is_empty() {
        return outcome;
    }
    let warning = format!(
        "\n[sandbox] plugin {} was denied network access to {}: it does not declare the network capability for it",
        entry.name,
        hosts.join(", ")
    );
    match outcome {
        ToolOutcome::Ok(text) => ToolOutcome::Ok(text + &warning),
        ToolOutcome::Failed(text) => ToolOutcome::Failed(text + &warning),
        ToolOutcome::Structured { output, value } => ToolOutcome::Structured {
            output: output + &warning,
            value,
        },
        other => other,
    }
}
