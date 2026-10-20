//! The built-in tool host (`tool-registry`, `builtin-tools`, `permissions-modes`).
mod command_mcp;
mod command_skills;
mod files;
mod isolation;
mod path_skills;
mod remote_sources;
pub(crate) mod review;
mod skill_catalog;
mod template_expand;
mod workflow_agents;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cyber_core::env::EnvSource;
use cyber_core::hooks::HookAction;
use cyber_core::skills::{self, Discovery, SkillScope};
use cyber_server::runtime::{
    CallState, Invocation, ModelResolver, PermissionAsk, PermissionReply, Reconciliation, Runtime,
    ToolDef, ToolHost, ToolOutcome, TurnContext, WeakRuntime,
};
use cyber_store::Store;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::budget::Budget;
use crate::permissions::{self, Decision, Effect, Mode, Policy, Request, saved};
use crate::tools::{self, Tool, ToolError};

/// Resolved configuration for a Location: the document and the source of each value.
pub type ConfigFn =
    dyn Fn(&Path) -> Result<(Value, BTreeMap<String, String>), String> + Send + Sync;

pub struct HostOptions {
    pub store: Arc<Store>,
    /// Managed overflow files (`<data>/tool-output`).
    pub tool_output_dir: PathBuf,
    /// Directories outside the Location that tools may use without asking.
    pub allowed_dirs: Vec<PathBuf>,
    pub home: PathBuf,
    pub shell: String,
    pub config: Arc<ConfigFn>,
    /// `~/.config/cyber`, for user skills.
    pub global_config_dir: PathBuf,
    /// Process environment, for web search credentials.
    pub env: Arc<dyn EnvSource + Send + Sync>,
    /// Models for hidden calls such as `webfetch` summaries; `None` disables them.
    pub models: Option<Arc<dyn ModelResolver>>,
    /// Root of the per-Session temporary directories (TMPDIR for sandboxed commands).
    pub temp_dir: PathBuf,
    /// The `--sandbox` flag, which alone may select `full-access`.
    pub sandbox_policy: Option<String>,
    /// Sandbox helper: Linux proxy bridging and Windows process ownership.
    pub sandbox_helper: Option<PathBuf>,
    /// What may enable `bypass` besides an enforced sandbox.
    pub isolation: IsolationOptions,
}

pub use isolation::{IsolationOptions, PermissionTest, PermissionTestReport};

pub struct BuiltinHost {
    pub(crate) weak: std::sync::Weak<Self>,
    pub(crate) opts: HostOptions,
    tools: Vec<Box<dyn Tool>>,
    reads: Mutex<HashMap<String, HashSet<PathBuf>>>,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    runtime: OnceLock<WeakRuntime>,
    pub(crate) hook_config: OnceLock<crate::tool_hooks::HooksConfig>,
    /// Handler digests one invocation approved; never persisted.
    pub(crate) hook_invocation_trust: OnceLock<cyber_core::trust::HookInvocationTrust>,
    /// Aggregated skip notices already shown, by owner and Session (and event).
    pub(crate) hook_notices: Mutex<HashSet<String>>,
    /// Tool-event hook effects awaiting their call's settlement, by (Session, call).
    pub(crate) hook_effects: crate::tool_hooks::PendingEffects,
    /// MCP annotations of registered calls in flight, by (Session, call), for hook
    /// envelopes.
    pub(crate) tool_annotations: Mutex<HashMap<(String, String), Value>>,
    pub(crate) search_cooldowns: tools::websearch::Cooldowns,
    /// Domains approved for sandboxed network access, by Session.
    network: Mutex<HashMap<String, Arc<Mutex<HashSet<String>>>>>,
    pub(crate) subagents: crate::subagents::Slots,
    pub(crate) mcp: crate::mcp::pool::Pool,
    /// Plugin host processes, by Location and plugin.
    pub(crate) plugins: crate::plugin_host::PluginHosts,
    /// Per-Session browser servers and the dev servers they own.
    pub(crate) browsers: crate::browser::Browsers,
    pub(crate) lsp: OnceLock<crate::lsp::Locations>,
    /// Per-command limit for template shell injections.
    pub(crate) injection_timeout_ms: std::sync::atomic::AtomicU64,
    /// The remote skill cache; without it, `https://` skill entries are reported unavailable.
    pub(crate) remote_skills: OnceLock<crate::remote_skills::RemoteSkills>,
    /// Hook handlers of connected clients (`client-sdk` → Hook callbacks).
    pub(crate) client_hooks: OnceLock<Arc<dyn cyber_server::runtime::ClientHooks>>,
}

impl BuiltinHost {
    pub fn lsp_checkout_claim(self: &Arc<Self>) -> crate::lsp::CheckoutClaim {
        let host = Arc::downgrade(self);
        Arc::new(move |expected, owner, cancel| {
            let host = host.clone();
            Box::pin(async move {
                let host = host.upgrade().ok_or(crate::lsp::LspError::Protocol(
                    "checkout claim host unavailable",
                ))?;
                host.claim_lsp_checkouts(expected, owner, cancel)
                    .await
                    .map_err(|_| crate::lsp::LspError::Protocol("managed checkout claim failed"))
            })
        })
    }
    pub fn attach_lsp(&self, factory: crate::lsp::PoolFactory) -> Result<(), String> {
        self.attach_lsp_locations(crate::lsp::Locations::new(factory))
    }

    pub fn attach_lsp_locations(&self, locations: crate::lsp::Locations) -> Result<(), String> {
        self.lsp
            .set(locations)
            .map_err(|_| "Language services already attached".into())
    }

    pub fn lsp_status(
        &self,
        location: &Path,
    ) -> Result<Vec<crate::lsp::ServerStatus>, crate::lsp::LspError> {
        self.lsp
            .get()
            .ok_or(crate::lsp::LspError::Protocol(
                "Language services unavailable",
            ))?
            .status(location)
    }

    pub fn warm_lsp(&self, location: &Path, file: PathBuf, text: String) {
        if let Some(locations) = self.lsp.get() {
            let _ = locations.warm(location, file, text);
        }
    }
    pub async fn transition_lsp_location(
        &self,
        location: &Path,
        reload: bool,
    ) -> Result<(), crate::lsp::LspError> {
        let locations = self.lsp.get().ok_or(crate::lsp::LspError::Protocol(
            "Language services unavailable",
        ))?;
        if reload {
            locations.reload_location(location).await?;
        } else {
            locations.close_location(location).await?;
        }
        Ok(())
    }
    pub fn new(opts: HostOptions) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            weak: weak.clone(),
            mcp: Default::default(),
            plugins: Default::default(),
            browsers: Default::default(),
            lsp: OnceLock::new(),
            opts,
            tools: tools::all(),
            reads: Mutex::default(),
            locks: Mutex::default(),
            runtime: OnceLock::new(),
            hook_config: OnceLock::new(),
            hook_invocation_trust: OnceLock::new(),
            hook_notices: Mutex::default(),
            hook_effects: Default::default(),
            tool_annotations: Mutex::default(),
            search_cooldowns: tools::websearch::Cooldowns::default(),
            network: Mutex::default(),
            subagents: crate::subagents::Slots::default(),
            injection_timeout_ms: std::sync::atomic::AtomicU64::new(
                template_expand::INJECTION_TIMEOUT.as_millis() as u64,
            ),
            remote_skills: OnceLock::new(),
            client_hooks: OnceLock::new(),
        })
    }

    /// Every built-in tool name, offered or not, and the runtime's `return_result`.
    pub(crate) fn builtin_tool_names(&self) -> std::collections::BTreeSet<String> {
        let mut names: std::collections::BTreeSet<String> =
            self.tools.iter().map(|tool| tool.def().spec.name).collect();
        names.insert("return_result".into());
        names
    }

    pub(crate) fn read_only_tool(&self, name: &str) -> bool {
        self.tools.iter().map(|tool| tool.def()).any(|def| {
            def.spec.name == name
                && def.retry_safety == cyber_server::runtime::RetrySafety::ReadOnly
        })
    }

    fn settle_output(&self, ctx: &Ctx<'_>, output: String, keep_tail: bool) -> ToolOutcome {
        let value = if ctx.inv.name == "agent" {
            match serde_json::from_str::<Value>(&output) {
                Ok(result) if result.get("result").is_some() => result.get("result").cloned(),
                Ok(result) if result["state"] == "running" => None,
                _ if ctx.inv.input.get("output_schema").is_some() => {
                    return ToolOutcome::Crashed(
                        "Structured agent result was not preserved".into(),
                    );
                }
                _ => None,
            }
        } else {
            None
        };
        match self.budget(&ctx.location).apply(output, keep_tail) {
            Ok(output) => match value {
                Some(value) => ToolOutcome::Structured { output, value },
                None => match ctx
                    .loaded_skill
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                {
                    Some(activation) => ToolOutcome::SkillLoaded { output, activation },
                    None => ToolOutcome::Ok(output),
                },
            },
            Err(error) => ToolOutcome::Crashed(error),
        }
    }

    /// Late-bind the runtime for tools that act on the Session (plan mode switches).
    pub fn attach(&self, runtime: Runtime) {
        let _ = self.runtime.set(runtime.downgrade());
    }

    pub(crate) fn clear_session_reads(&self, session: &str) {
        self.reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session);
    }

    pub(crate) fn runtime(&self) -> Option<Runtime> {
        self.runtime.get().and_then(WeakRuntime::upgrade)
    }

    /// `CYBER_MCP_DEPTH` for a process started for the Session: its run's `mcp_depth`
    /// (or its ancestors', for a child Session) + 1 (`mcp-server` recursion guard).
    pub(crate) fn process_mcp_depth(&self, session_id: &str) -> String {
        let depth = self
            .runtime()
            .map_or(0, |runtime| runtime.effective_mcp_depth(session_id));
        depth.saturating_add(1).to_string()
    }

    fn agent_profile(
        &self,
        directory: &Path,
        agent: &str,
    ) -> Result<cyber_core::config::AgentProfile, String> {
        let (config, _) = (self.opts.config)(directory)?;
        Self::profile_from(&config, agent)
    }

    fn profile_from(
        config: &Value,
        agent: &str,
    ) -> Result<cyber_core::config::AgentProfile, String> {
        let mut profiles = cyber_core::config::resolve_agents(config)?;
        let available = profiles
            .values()
            .filter(|profile| !profile.hidden)
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        profiles
            .remove(agent)
            .ok_or_else(|| format!("Unknown agent {agent:?}. Available: {available}"))
    }

    /// Apply profile visibility to built-in or client-provided Turn definitions.
    pub fn filter_agent_tools(&self, turn: &TurnContext, tools: Vec<ToolDef>) -> Vec<ToolDef> {
        let Ok(profile) = self.agent_profile(Path::new(&turn.directory), &turn.agent) else {
            return Vec::new();
        };
        tools
            .into_iter()
            .filter(|tool| profile_allows_tool(&profile, &tool.spec.name))
            .collect()
    }

    /// Regenerate the `agent` description from the Turn's current profiles and rules.
    fn with_agent_catalogue(
        &self,
        turn: &TurnContext,
        rules: &[permissions::Rule],
        tools: Vec<ToolDef>,
    ) -> Vec<ToolDef> {
        if !tools.iter().any(|tool| tool.spec.name == "agent") {
            return tools;
        }
        let config = (self.opts.config)(Path::new(&turn.directory)).map(|(config, _)| config);
        tools
            .into_iter()
            .filter_map(|tool| match (&config, tool.spec.name.as_str()) {
                (_, name) if name != "agent" => Some(tool),
                (Ok(config), _) => tools::agent::with_catalogue(tool, config, rules),
                (Err(_), _) => None,
            })
            .collect()
    }

    /// Recheck visibility before a call can reach either execution host.
    pub fn check_agent_tool(&self, call: &Invocation) -> Result<(), String> {
        let profile = self.agent_profile(Path::new(&call.directory), &call.agent)?;
        if !profile_allows_tool(&profile, &call.name) {
            return Err(format!(
                "Tool {:?} is unavailable for agent {:?}",
                call.name, call.agent
            ));
        }
        Ok(())
    }

    fn rules_for(
        &self,
        location: &Path,
        primary_agent: bool,
        config: &Value,
        sources: &BTreeMap<String, String>,
    ) -> Vec<permissions::Rule> {
        // Skill directories are readable without prompting (`skills-commands`).
        let mut allowed = self.opts.allowed_dirs.clone();
        allowed.extend(
            self.skills_for(location, config, sources)
                .skills
                .into_values()
                .filter_map(|s| s.directory().map(Path::to_path_buf)),
        );
        let mut rules = permissions::defaults(&allowed, primary_agent);
        for scope in ["project", "global"] {
            rules.push(permissions::Rule::new(
                "memory",
                scope,
                Effect::Allow,
                "default",
            ));
        }
        rules.extend(permissions::parse_rules(&config["permissions"], sources));
        rules
    }

    /// Skills a user can invoke as `/name`, including embedded source identity.
    pub fn skill_commands(&self, location: &Path) -> Vec<skills::Skill> {
        self.skills(location)
            .skills
            .into_values()
            .filter(|s| s.user_invocable)
            .collect()
    }

    /// A user-invocable skill's body with arguments substituted (`skills-commands` → Skills as
    /// user commands).
    pub fn expand_skill(&self, location: &Path, name: &str, arguments: &str) -> Option<String> {
        let skill = self
            .skills(location)
            .skills
            .remove(name)
            .filter(|s| s.user_invocable)?;
        Some(skills::expand(&skill.body, arguments))
    }

    pub fn command_registry(&self, location: &Path) -> cyber_core::commands::Commands {
        let (config, sources) = match (self.opts.config)(location) {
            Ok(config) => config,
            Err(_) => {
                return cyber_core::commands::Commands {
                    issues: vec![cyber_core::commands::CommandIssue {
                        path: location.into(),
                        reason: "command configuration is unavailable",
                    }],
                    ..Default::default()
                };
            }
        };
        cyber_core::commands::discover(
            &cyber_core::commands::CommandScope {
                location: location.into(),
                home: self.opts.home.clone(),
                global_config_dir: self.opts.global_config_dir.clone(),
                project: !self.opts.env.flag("CYBER_DISABLE_PROJECT_CONFIG"),
                compat: true,
                plugins: cyber_core::plugins::component_dirs(&config, "commands"),
            },
            &config,
            &sources,
        )
    }

    /// Skills have later precedence; configured reserved names use the project namespace.
    pub fn expand_command(&self, location: &Path, name: &str, arguments: &str) -> Option<String> {
        if cyber_core::commands::RESERVED.contains(&name) {
            return None;
        }
        let (skill, registry) = self.command_sources(location, name);
        skill
            .map(|skill| skills::expand(&skill.body, arguments))
            .or_else(|| {
                registry
                    .entries
                    .get(name)
                    .map(|command| command.expand(arguments))
            })
    }

    /// Files and directories under a Location whose relative path contains every
    /// whitespace-separated word of `query` (case-insensitive), honoring ignore files.
    pub fn find_files(&self, location: &Path, query: &str, limit: usize) -> Vec<String> {
        let words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let walker = ignore::WalkBuilder::new(location)
            .hidden(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        let mut hits: Vec<String> = walker
            .flatten()
            .filter_map(|e| {
                let rel = e
                    .path()
                    .strip_prefix(location)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                let lower = rel.to_lowercase();
                (!rel.is_empty() && words.iter().all(|w| lower.contains(w.as_str()))).then(|| {
                    if e.file_type().is_some_and(|t| t.is_dir()) {
                        format!("{rel}/")
                    } else {
                        rel
                    }
                })
            })
            .take(limit * 4)
            .collect();
        // Shorter paths first: closer matches for a mention.
        hits.sort_by_key(|h| (h.len(), h.clone()));
        hits.truncate(limit);
        hits
    }

    pub(crate) fn network_approvals(&self, session: &str) -> Arc<Mutex<HashSet<String>>> {
        let mut map = self.network.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry(session.to_string()).or_default())
    }

    /// Skills visible from a Location, honoring `skills.compat`, `compat.<tool>` and `skills.paths`.
    pub(crate) fn skills(&self, location: &Path) -> Discovery {
        let (config, sources) = (self.opts.config)(location).unwrap_or_default();
        self.skills_for(location, &config, &sources)
    }

    /// Local entries resolve against the project root (`~/` against home); `https://`
    /// entries contribute their cached skills at the same precedence position.
    fn skills_for(
        &self,
        location: &Path,
        config: &Value,
        sources: &BTreeMap<String, String>,
    ) -> Discovery {
        let settings = &config["skills"];
        let (entries, pointer) = if settings.is_array() {
            (settings, "/skills")
        } else {
            (&settings["paths"], "/skills/paths")
        };
        let root = cyber_core::config::project_root(location);
        let mut extra = Vec::new();
        let mut notes = Vec::new();
        for (index, entry) in entries.as_array().into_iter().flatten().enumerate() {
            let Some(entry) = entry.as_str() else {
                continue;
            };
            if entry.contains("://") {
                let layer = sources.get(&format!("{pointer}/{index}"));
                let remote = self.remote_skill_dirs(location, config, sources, entry, layer);
                extra.extend(remote.0);
                notes.extend(remote.1);
                continue;
            }
            extra.push(match entry.strip_prefix("~/") {
                Some(rest) => self.opts.home.join(rest),
                None => root.join(entry),
            });
        }
        let mut found = skills::discover(&SkillScope {
            location: location.to_path_buf(),
            home: self.opts.home.clone(),
            global_config_dir: self.opts.global_config_dir.clone(),
            compat: skills::SkillCompat::from_config(config),
            extra,
            plugins: skills::PluginSkills::from_config(config),
        });
        found.diagnostics.extend(notes);
        found
    }

    fn websearch_available(&self, location: &Path) -> bool {
        !tools::websearch::backends(&self.config_for(location), &*self.opts.env).is_empty()
    }

    fn listing_budget(&self, location: &Path) -> usize {
        let config = self.config_for(location);
        let tokens = config
            .pointer("/skills/listing_budget_tokens")
            .and_then(Value::as_u64);
        tokens.unwrap_or(2000) as usize * 4
    }

    pub(crate) fn config_for(&self, location: &Path) -> Value {
        (self.opts.config)(location)
            .map(|(c, _)| c)
            .unwrap_or(Value::Null)
    }

    /// Agent rules follow config; the Session ruleset is evaluated last.
    pub(crate) fn session_rules(
        &self,
        location: &Path,
        agent: Option<&str>,
        session: &Value,
    ) -> Result<Vec<permissions::Rule>, String> {
        let (config, sources) = (self.opts.config)(location)?;
        let profile = agent
            .map(|name| Self::profile_from(&config, name))
            .transpose()?;
        let primary = profile
            .as_ref()
            .is_none_or(|profile| profile.primary_capable());
        let mut rules = self.rules_for(location, primary, &config, &sources);
        if let Some(profile) = profile {
            let mut extra = permissions::parse_rules(&profile.permissions, &BTreeMap::new());
            for rule in &mut extra {
                rule.source = format!("agent:{}", profile.name);
            }
            rules.extend(extra);
        }
        let mut extra = permissions::parse_rules(session, &BTreeMap::new());
        for rule in &mut extra {
            rule.source = "session".into();
        }
        rules.extend(extra);
        if agent == Some("reviewer") {
            let mut immutable =
                permissions::parse_rules(&serde_json::json!({"edit":"deny"}), &BTreeMap::new());
            for rule in &mut immutable {
                rule.source = "builtin:reviewer".into();
            }
            rules.extend(immutable);
        }
        Ok(rules)
    }

    /// Why a `workflow.run` rule of the owner Session denies `module`, for runs started or
    /// resumed over the API (`workflows` → Permissions for runs). The client approves the
    /// run by asking for it, so only a deny refuses; it holds for the bare name and for the
    /// origin-qualified resource.
    pub fn workflow_run_denial(
        &self,
        owner: &TurnContext,
        module: &cyber_workflow::WorkflowModule,
    ) -> Result<Option<String>, String> {
        let rules = self.session_rules(
            Path::new(&owner.directory),
            Some(&owner.agent),
            &owner.rules,
        )?;
        let resources = [
            module.meta.name.clone(),
            permissions::workflow_resource(module),
        ];
        let denied = resources.iter().any(|resource| {
            permissions::evaluate(&rules, permissions::WORKFLOW_RUN, resource).0 == Effect::Deny
        });
        Ok(denied.then(|| {
            format!(
                "Permission denied: workflow.run is denied for {}",
                module.meta.name
            )
        }))
    }

    pub(crate) async fn policy(&self, inv: &Invocation) -> Result<Policy, String> {
        self.policy_for(inv, Some(&inv.agent)).await
    }

    pub(crate) async fn policy_for(
        &self,
        inv: &Invocation,
        agent: Option<&str>,
    ) -> Result<Policy, String> {
        let location = PathBuf::from(&inv.directory);
        let root = cyber_core::config::project_root(&location);
        let mut rules = self.session_rules(&location, agent, &inv.rules)?;
        let inherited = if agent.is_some() {
            self.inherited_permissions(&inv.session_id).await?
        } else {
            Default::default()
        };
        rules.extend(inherited.rules);
        let ownership = inherited.ownership;
        let mut parent_modes = inherited.modes;
        if agent == Some("reviewer") {
            parent_modes.push(Mode::Plan);
        }
        let mut saved = saved::rules(&self.opts.store, &root).unwrap_or_default();
        for rule in self.skill_scope_rules(inv).await? {
            if rule.effect == Effect::Deny {
                rules.push(rule);
            } else {
                saved.push(rule);
            }
        }
        Ok(Policy {
            rules,
            saved,
            mode: Mode::parse(&inv.mode),
            parent_modes,
            plan_file: location
                .join(".cyber/plans")
                .join(format!("{}.md", inv.session_id)),
            location,
            home: self.opts.home.clone(),
            ownership,
        })
    }

    pub(crate) fn budget(&self, location: &Path) -> Budget {
        let config = self.config_for(location);
        let get = |k: &str, d: usize| {
            config
                .pointer(&format!("/tool_output/{k}"))
                .and_then(Value::as_u64)
                .map_or(d, |v| v as usize)
        };
        Budget {
            max_lines: get("max_lines", 2000),
            max_bytes: get("max_bytes", 51_200),
            dir: self.opts.tool_output_dir.clone(),
        }
    }

    pub(crate) fn mark_read(&self, session: &str, path: &Path) {
        self.reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session.into())
            .or_default()
            .insert(path.into());
    }

    pub(crate) fn was_read(&self, session: &str, path: &Path) -> bool {
        self.reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .is_some_and(|s| s.contains(path))
    }

    /// Writes to one canonical path are serialized within the server.
    pub(crate) fn path_lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(path.into())
                .or_default(),
        )
    }
}

impl ToolHost for BuiltinHost {
    fn open_location(&self, info: &cyber_server::runtime::SessionInfo) {
        self.start_mcp(info);
        self.reconcile_plugins(&info.directory, true);
        self.start_browser(info);
    }

    fn refresh_location(&self, info: &cyber_server::runtime::SessionInfo) {
        self.refresh_mcp(info);
        self.reconcile_plugins(&info.directory, false);
        self.start_browser(info);
    }

    /// Required MCP servers, then every plugin host of the Location and the Session's
    /// browser settling their start.
    fn wait_for_required_mcp(
        &self,
        info: &cyber_server::runtime::SessionInfo,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), cyber_server::runtime::RuntimeError>> {
        let directory = info.directory.clone();
        let session = info.id.clone();
        Box::pin(async move {
            self.wait_required_mcp(Path::new(&directory), cancel.clone())
                .await?;
            self.wait_plugins(&directory, cancel.clone()).await;
            self.wait_browser(&session, cancel).await;
            Ok(())
        })
    }

    fn lifecycle_hooks(
        &self,
        event: cyber_server::runtime::LifecycleHook,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::LifecycleVerdict, String>> {
        Box::pin(self.runtime_lifecycle_hooks(event, cancel))
    }

    fn take_hook_effects(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> cyber_server::runtime::HookEffects {
        self.take_tool_hook_effects(session_id, call_id)
    }

    fn file_decisions<'a>(
        &'a self,
        turn: &'a TurnContext,
        action: &'a str,
        paths: &'a [PathBuf],
    ) -> BoxFuture<'a, Vec<cyber_server::runtime::FileDecision>> {
        Box::pin(self.decide_files(turn, action, paths))
    }

    fn authorize_file<'a>(
        &'a self,
        turn: &'a TurnContext,
        action: &'a str,
        path: &'a Path,
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(self.authorize_browse(turn, action, path, asker, cancel))
    }

    fn resolve_prompt_resources<'a>(
        &'a self,
        turn: &'a TurnContext,
        parts: &'a [cyber_llm::Content],
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(self.resolve_mentions(turn, parts, asker, cancel))
    }

    fn expand_prompt_template<'a>(
        &'a self,
        turn: &'a TurnContext,
        template: &'a cyber_server::runtime::PromptTemplate,
        parts: &'a [cyber_llm::Content],
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<cyber_llm::Content>, String>> {
        Box::pin(self.expand_template(turn, template, parts, asker, cancel))
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let lsp = async {
                if let Some(locations) = self.lsp.get() {
                    match locations.close().await {
                        Ok(settlements) if settlements.iter().all(|s| s.acknowledged) => {}
                        _ => cyber_core::log::error(
                            "lsp",
                            "Language service termination is unverified",
                            serde_json::json!({}),
                        ),
                    }
                }
            };
            tokio::join!(
                self.shutdown_mcp(),
                lsp,
                self.shutdown_plugins(),
                self.shutdown_browsers()
            );
        })
    }

    fn end_session(&self, session_id: &str) -> BoxFuture<'_, ()> {
        let session_id = session_id.to_string();
        Box::pin(async move { self.end_browser_session(&session_id).await })
    }
    /// A config that fails to load turns `CLAUDE.md` off rather than on: `config_for`'s
    /// `Null` fallback would read as the default `true` and override an opt-out.
    fn claude_compat(&self, directory: &str) -> Option<bool> {
        let compat = (self.opts.config)(Path::new(directory))
            .is_ok_and(|(config, _)| cyber_core::config::claude_compat(&config));
        Some(compat)
    }

    fn instruction_read_gate(
        &self,
        directory: &str,
    ) -> Option<cyber_server::runtime::instructions::ReadGate> {
        let location = Path::new(directory);
        let config = (self.opts.config)(location).ok();
        Some(permissions::instruction_read_gate(
            config.as_ref().map(|(config, sources)| (config, sources)),
            location,
        ))
    }

    fn finalize_skill_output(
        &self,
        directory: &str,
        output: String,
        reminder: &str,
    ) -> Result<String, String> {
        self.budget(Path::new(directory))
            .apply_with_reminder(output, reminder)
    }

    fn session_budget(
        &self,
        directory: &str,
    ) -> Result<Option<cyber_core::budget::Budget>, String> {
        let (config, _) = (self.opts.config)(Path::new(directory))?;
        cyber_core::budget::Budget::from_config(&config, "session")
    }

    fn admit_mode(&self, directory: &str, agent: &str, mode: &str) -> Result<(), String> {
        self.mode_admission(Path::new(directory), agent, mode)
    }

    fn config_digest(&self, directory: &str) -> Option<String> {
        use sha2::Digest;
        // An unresolvable configuration has no digest; the next resolvable one is
        // compared with the last good one.
        let resolved = (self.opts.config)(Path::new(directory)).ok()?;
        let bytes = serde_json::to_vec(&resolved).ok()?;
        Some(format!("sha256:{:x}", sha2::Sha256::digest(bytes)))
    }

    fn loop_settings(
        &self,
        directory: &str,
    ) -> Result<
        (
            cyber_core::loops::LoopSettings,
            Option<cyber_core::budget::Budget>,
        ),
        String,
    > {
        let (config, _) = (self.opts.config)(Path::new(directory))?;
        let budget = cyber_core::budget::Budget::from_config(&config, "loop")?;
        Ok((
            cyber_core::loops::LoopSettings::from_config(&config),
            budget,
        ))
    }

    fn check(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
        timeout_ms: u64,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::CheckOutcome, String>> {
        let inv = user_invocation(directory, session_id, command);
        let command = command.to_string();
        Box::pin(async move {
            let ctx = self.user_ctx(&inv, cancel).await?;
            tools::bash::run_check(&ctx, &command, timeout_ms)
                .await
                .map_err(user_error)
        })
    }

    fn loop_command_plan<'a>(
        &'a self,
        turn: &'a TurnContext,
        name: &'a str,
        arguments: &'a str,
    ) -> BoxFuture<
        'a,
        Result<Option<cyber_server::runtime::CommandPlan>, cyber_server::runtime::CommandPlanError>,
    > {
        Box::pin(self.command_plan(turn, name, arguments))
    }

    fn daily_budget(&self, directory: &str) -> Result<Option<cyber_core::budget::Budget>, String> {
        let (config, _) = (self.opts.config)(Path::new(directory))?;
        cyber_core::budget::Budget::from_config(&config, "daily")
    }

    fn messaging_settings(
        &self,
        directory: Option<&str>,
    ) -> Result<cyber_core::config::MessagingSettings, String> {
        tools::messaging::settings(self, directory.map(Path::new))
    }

    fn team_settings(
        &self,
        directory: &str,
    ) -> Result<Option<cyber_core::teams::TeamSettings>, String> {
        let location = Path::new(directory);
        let (config, sources) = (self.opts.config)(location)?;
        if !cyber_core::features::Features::from_config(&config, &sources)
            .enabled(cyber_core::features::TEAMS)
        {
            return Ok(None);
        }
        let mut settings = cyber_core::teams::TeamSettings::from_config(&config)?;
        settings.budget = cyber_core::budget::Budget::from_config(&config, "team")?;
        Ok(Some(settings))
    }

    fn team_editor(&self, directory: &str, agent: &str) -> bool {
        self.team_editor_agent(directory, agent)
    }

    fn create_team_session<'a>(
        &'a self,
        lead: &'a cyber_server::runtime::SessionInfo,
        request: cyber_server::runtime::CreateSession,
        worktree: &'a cyber_server::runtime::TeamWorktree,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<cyber_server::runtime::SessionInfo, String>> {
        Box::pin(async move {
            self.create_team_worktree_session(lead, request, worktree, cancel)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn permission_effect(
        &self,
        turn: &TurnContext,
        action: &str,
        resource: &str,
    ) -> Result<cyber_server::runtime::RuleEffect, String> {
        use cyber_server::runtime::RuleEffect;
        let rules =
            self.session_rules(Path::new(&turn.directory), Some(&turn.agent), &turn.rules)?;
        Ok(
            match permissions::boundary_evaluate(&rules, action, resource) {
                Effect::Allow => RuleEffect::Allow,
                Effect::Ask => RuleEffect::Ask,
                Effect::Deny => RuleEffect::Deny,
            },
        )
    }

    fn request_overlay(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_llm::catalog::RequestOverlay, String> {
        self.agent_inference(turn).map(|options| options.request)
    }

    fn agent_inference(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_server::runtime::AgentInference, String> {
        let profile = self.agent_profile(Path::new(&turn.directory), &turn.agent)?;
        if profile.hidden {
            return Err(format!(
                "Agent {:?} is hidden and cannot drive a Session",
                turn.agent
            ));
        }
        Ok(cyber_server::runtime::AgentInference {
            permission_mode: profile.permission_mode,
            steps: profile
                .steps
                .or_else(|| (profile.mode == "subagent").then_some(50)),
            model: profile.model,
            variant: profile.variant,
            request: cyber_llm::catalog::RequestOverlay::from_config(Some(&profile.request)),
        })
    }

    fn claim_location<'a>(
        &'a self,
        info: &'a cyber_server::runtime::SessionInfo,
        creating: bool,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<cyber_server::runtime::LocationLease, String>> {
        Box::pin(self.claim_worktree_location(info, creating, cancel))
    }

    fn prepare_child_continuation<'a>(
        &'a self,
        parent: &'a cyber_server::runtime::SessionInfo,
        child: &'a cyber_server::runtime::SessionState,
        owner: &'a cyber_server::runtime::ChildExecution,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Box<dyn cyber_server::runtime::ChildContinuation>>, String>>
    {
        Box::pin(async move {
            let Some(managed) = child.child_worktree().cloned() else {
                return Ok(None);
            };
            let info = &child.info;
            let inv = Invocation {
                registration: None,
                session_id: info.id.clone(),
                directory: parent.directory.clone(),
                agent: info.agent.clone(),
                mode: info.mode.clone(),
                rules: info.rules.clone(),
                message_id: String::new(),
                call_id: cyber_core::ids::new_id("call"),
                name: "agent".into(),
                input: serde_json::json!({"operation":"continue","resume":info.id}),
                attempt: 1,
                operation_key: cyber_core::ids::new_id("op"),
                asker: cyber_server::runtime::Asker::detached(),
            };
            let worktree = self
                .resume_child_worktree_authorized(&inv, cancel, owner, info, managed, true)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Some(
                Box::new(worktree) as Box<dyn cyber_server::runtime::ChildContinuation>
            ))
        })
    }

    fn deferred_tool_settings(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_core::config::DeferredToolSettings, String> {
        let (config, _) = (self.opts.config)(Path::new(&turn.directory))?;
        cyber_core::config::DeferredToolSettings::from_config(&config)
    }

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let Ok(rules) =
            self.session_rules(Path::new(&turn.directory), Some(&turn.agent), &turn.rules)
        else {
            return Vec::new();
        };
        let mode = Mode::parse(&turn.mode);
        let resources = std::cell::OnceCell::new();
        let messaging = std::cell::OnceCell::new();
        let mut tools: Vec<_> = self
            .tools
            .iter()
            .filter_map(|t| {
                let definition = t.def();
                if definition.spec.name != "memory" {
                    return Some(definition);
                }
                let settings = tools::memory::settings(self, Path::new(&turn.directory)).ok()?;
                settings
                    .enabled
                    .then(|| tools::memory::definition(settings.generate && mode != Mode::Plan))
            })
            .filter(|d| offered(&d.spec.name, turn.prefers_apply_patch, mode, d.retry_safety))
            .filter(|d| tools::worktree_offered(&d.spec.name, turn.worktree))
            .filter(|d| !fully_denied_scoped(&rules, tools::action_of(&d.spec.name), &d.spec.name))
            .filter(|d| {
                d.spec.name != "websearch" || self.websearch_available(Path::new(&turn.directory))
            })
            .filter(|d| {
                !tools::messaging::TOOLS.contains(&d.spec.name.as_str())
                    || *messaging
                        .get_or_init(|| tools::messaging::enabled(self, Path::new(&turn.directory)))
            })
            .filter(|d| {
                // `loops-scheduling`: only while an iteration of an active loop runs.
                d.spec.name != tools::loops::NAME
                    || self
                        .runtime()
                        .is_some_and(|runtime| runtime.loop_iteration_running(&turn.session_id))
            })
            .filter(|d| {
                // `agent-teams`: only while `teams` is on, and to a lead or a non-member.
                tools::team::offered(
                    self,
                    &d.spec.name,
                    &turn.session_id,
                    Path::new(&turn.directory),
                )
            })
            .filter(|d| {
                d.spec.name != "powershell"
                    || tools::powershell::installed(Path::new(&turn.directory)).is_some()
            })
            .filter(|d| {
                !tools::mcp::RESOURCE_TOOLS.contains(&d.spec.name.as_str())
                    || *resources.get_or_init(|| self.mcp_resources_offered(turn, &rules))
            })
            .collect();
        tools.extend(
            self.mcp_definitions(turn)
                .into_iter()
                .chain(self.browser_definitions(turn))
                .chain(self.plugin_definitions(turn))
                .filter(|d| !fully_denied(&rules, &d.spec.name)),
        );
        let tools = self.filter_agent_tools(turn, tools);
        self.with_agent_catalogue(turn, &rules, tools)
    }

    fn reconcile(&self, directory: &str, call: &CallState) -> BoxFuture<'_, Reconciliation> {
        let result = crate::reconcile::reconcile(Path::new(directory), &self.opts.home, call);
        Box::pin(async move { result })
    }

    fn context_sources(&self, turn: &TurnContext) -> BTreeMap<String, String> {
        self.context_sources_for_tools(turn, &self.definitions(turn))
    }

    fn context_observations(
        &self,
        turn: &TurnContext,
    ) -> BTreeMap<String, cyber_server::runtime::ContextObservation> {
        self.context_observations_for_tools(turn, &self.definitions(turn))
    }

    fn context_observations_owned<'a>(
        &'a self,
        turn: &'a TurnContext,
    ) -> BoxFuture<'a, BTreeMap<String, cyber_server::runtime::ContextObservation>> {
        Box::pin(async move {
            self.context_observations_owned_for_tools(turn, &self.definitions(turn))
                .await
        })
    }

    fn execute(&self, inv: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async move {
            if inv.name.starts_with("mcp__") {
                return match self.browser_binding(&inv) {
                    Some(binding) => self.execute_browser(inv, binding, cancel).await,
                    None => self.execute_mcp(inv, cancel).await,
                };
            }
            let mut inv = inv;
            if let Err(error) = inv.asker.validate_auto_override(&inv.name, &inv.input) {
                return ToolOutcome::Failed(error.to_string());
            }
            let Some(tool) = self.tools.iter().find(|t| t.def().spec.name == inv.name) else {
                if let Some(offered) = self.plugin_tool(&inv) {
                    return self.execute_plugin(inv, offered, cancel).await;
                }
                return ToolOutcome::Failed(format!("Unknown tool: {}", inv.name));
            };
            if let Err(error) = self.check_skill_tool(&inv).await {
                return ToolOutcome::Failed(error);
            }
            if let Err(error) = self.check_agent_tool(&inv) {
                return ToolOutcome::Failed(error);
            }
            if let Err(message) = crate::schema::validate(&tool.def().spec.input_schema, &inv.input)
            {
                return ToolOutcome::Failed(message);
            }
            let hook_decision = match self
                .pre_tool_hooks(&mut inv, &tool.def().spec.input_schema, cancel.clone())
                .await
            {
                Ok(allowed) => allowed,
                Err(ToolError::Failed(error)) => return ToolOutcome::Failed(error),
                Err(ToolError::Aborted) => return ToolOutcome::Aborted,
            };
            let started = std::time::Instant::now();
            let ctx = match self
                .call_ctx(&inv, hook_decision, cancel.clone(), DenialHooks::armed())
                .await
            {
                Ok(ctx) => ctx,
                Err(error) => return ToolOutcome::Failed(error),
            };
            let outcome = self.run_builtin(tool.as_ref(), &ctx).await;
            let Some(input) = ctx.denial.take_retry(&outcome) else {
                return self.settle_call(&ctx, outcome, started).await;
            };
            // A PermissionDenied retry is re-validated and re-evaluated exactly once;
            // its denial is final. Changed input passes PreToolUse again first.
            let schema = &tool.def().spec.input_schema;
            let retried = self
                .retried_invocation(&inv, schema, input, hook_decision, cancel.clone())
                .await;
            let (retried, decision) = match retried {
                Ok(retried) => retried,
                Err(ToolError::Failed(error)) => {
                    return self
                        .settle_call(&ctx, ToolOutcome::Failed(error), started)
                        .await;
                }
                Err(ToolError::Aborted) => {
                    return self.settle_call(&ctx, ToolOutcome::Aborted, started).await;
                }
            };
            let ctx = match self
                .call_ctx(&retried, decision, cancel, DenialHooks::default())
                .await
            {
                Ok(ctx) => ctx,
                Err(error) => return ToolOutcome::Failed(error),
            };
            let outcome = self.run_builtin(tool.as_ref(), &ctx).await;
            self.settle_call(&ctx, outcome, started).await
        })
    }

    fn subtask(
        &self,
        turn: TurnContext,
        prompt: String,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        self.subtask_with_agent(turn, prompt, None, cancel)
    }

    fn subtask_with_agent(
        &self,
        turn: TurnContext,
        prompt: String,
        agent: Option<String>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        self.subtask_request(
            turn,
            cyber_server::runtime::UserSubtask {
                skill_command: None,
                admission_id: None,
                prompt,
                agent,
                attachments: Vec::new(),
                max_steps: None,
                hook_context: None,
            },
            cancel,
        )
    }

    fn durable_user_delegation(&self) -> bool {
        true
    }

    fn subtask_request(
        &self,
        turn: TurnContext,
        request: cyber_server::runtime::UserSubtask,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        Box::pin(async move {
            let cyber_server::runtime::UserSubtask {
                skill_command,
                admission_id,
                prompt,
                agent,
                attachments,
                max_steps,
                hook_context,
            } = request;
            if max_steps == Some(0) {
                return Err("Subtask max_steps must be positive".into());
            }
            if let Some(name) = &agent {
                let (config, _) = (self.opts.config)(Path::new(&turn.directory))?;
                let profiles = cyber_core::config::resolve_agents(&config)?;
                if !profiles
                    .get(name)
                    .is_some_and(|profile| profile.subagent_capable())
                {
                    return Err(format!("Agent {name:?} cannot run a user subagent"));
                }
            }
            // A named @agent delegation is a fresh child; a forwarded
            // `context: fork` skill always forks, even with a declared agent.
            let fork = agent.is_none() || skill_command.as_ref().is_some_and(|skill| skill.fork);
            let mut input = serde_json::json!({"prompt":prompt,"fork":fork,"background":true,"attachments":attachments,"max_steps":max_steps});
            if let Some(agent) = agent {
                input["agent"] = agent.into();
            }
            if let Some(skill) = &skill_command {
                input["description"] = format!("Run {} skill task", skill.activation.name).into();
            }
            if let Some(model) = skill_command
                .as_ref()
                .and_then(|skill| skill.model.as_ref())
            {
                input["model"] = model.clone().into();
            }
            if let Some(schema) = skill_command
                .as_ref()
                .and_then(|skill| skill.output_schema.as_ref())
            {
                input["output_schema"] = schema.clone();
            }
            let inv = Invocation {
                registration: None,
                session_id: turn.session_id,
                directory: turn.directory,
                agent: turn.agent,
                mode: turn.mode,
                message_id: String::new(),
                call_id: cyber_core::ids::new_id("call"),
                name: "agent".into(),
                input,
                attempt: 1,
                operation_key: admission_id.unwrap_or_else(|| cyber_core::ids::new_id("op")),
                asker: cyber_server::runtime::Asker::detached(),
                rules: turn.rules,
            };
            self.check_agent_tool(&inv)?;
            let ctx = Ctx {
                skill_paths: Default::default(),
                loaded_skill: Default::default(),
                compiler_feedback: Default::default(),
                hook_decision: None,
                denial: Default::default(),
                host: self,
                policy: self.policy(&inv).await?,
                location: PathBuf::from(&inv.directory),
                inv: &inv,
                cancel,
            };
            let output = tools::agent::run_with_skill(
                &ctx,
                tools::agent::SpawnSource::User,
                skill_command,
                hook_context,
            )
            .await
            .map_err(|e| match e {
                ToolError::Failed(message) => message,
                ToolError::Aborted => "Subtask interrupted".into(),
            })?;
            let result: Value = serde_json::from_str(&output).map_err(|e| e.to_string())?;
            let job = result["job_id"]
                .as_str()
                .ok_or("Subtask did not return a Job")?;
            self.runtime()
                .ok_or("Subtask requires the runtime")?
                .job(job)
                .map_err(|e| e.to_string())
        })
    }

    fn spawn_workflow_agent(
        &self,
        request: cyber_server::runtime::WorkflowAgentRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::WorkflowAgentOutput, cyber_workflow::Diagnostic>>
    {
        Box::pin(self.workflow_agent(request, cancel))
    }

    fn workflow_agent_writes(&self, directory: &str, agent: Option<&str>) -> bool {
        self.workflow_writer(directory, agent)
    }

    fn shell(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.shell_owned(directory, session_id, command, CancellationToken::new())
    }

    fn shell_owned(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        let inv = user_invocation(directory, session_id, command);
        let command = command.to_string();
        Box::pin(async move {
            let ctx = self.user_ctx(&inv, cancel).await?;
            let output = tools::bash::run_user(&ctx, &command)
                .await
                .map_err(user_error)?;
            self.budget(&ctx.location).apply(output, true)
        })
    }

    fn shell_background(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        let inv = user_invocation(directory, session_id, command);
        let command = command.to_string();
        Box::pin(async move {
            let ctx = self.user_ctx(&inv, CancellationToken::new()).await?;
            tools::bash::run_user_background(&ctx, &command)
                .await
                .map_err(user_error)
        })
    }
}

/// A user shell command (`!` or `&`): the `user` agent in `bypass`, never asking.
fn user_invocation(directory: &str, session_id: &str, command: &str) -> Invocation {
    Invocation {
        registration: None,
        session_id: session_id.into(),
        directory: directory.into(),
        agent: "user".into(),
        mode: "bypass".into(),
        message_id: String::new(),
        call_id: cyber_core::ids::new_id("call"),
        name: "bash".into(),
        input: serde_json::json!({ "command": command }),
        attempt: 1,
        operation_key: String::new(),
        asker: cyber_server::runtime::Asker::detached(),
        rules: Value::Null,
    }
}

fn user_error(error: ToolError) -> String {
    match error {
        ToolError::Failed(message) => message,
        ToolError::Aborted => "aborted".into(),
    }
}

impl BuiltinHost {
    async fn user_ctx<'a>(
        &'a self,
        inv: &'a Invocation,
        cancel: CancellationToken,
    ) -> Result<Ctx<'a>, String> {
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: self.policy_for(inv, None).await?,
            location: PathBuf::from(&inv.directory),
            inv,
            cancel,
        })
    }
}

/// Which tools a Turn offers: `apply_patch` or `edit`/`write` by model, read-only tools
/// plus the plan-file write in `plan` mode.
fn offered(
    name: &str,
    prefers_patch: bool,
    mode: Mode,
    safety: cyber_server::runtime::RetrySafety,
) -> bool {
    let by_model = match name {
        "apply_patch" => prefers_patch,
        "edit" => !prefers_patch,
        "write" => !prefers_patch || mode == Mode::Plan,
        _ => true,
    };
    let by_mode = match (mode, name) {
        (Mode::Plan, "write" | "plan_exit" | "todo" | "skill" | "notify") => true,
        (Mode::Plan, "plan_enter") => false,
        (Mode::Plan, _) => safety == cyber_server::runtime::RetrySafety::ReadOnly,
        (_, "plan_exit") => false,
        _ => true,
    };
    by_model && by_mode
}

pub(crate) fn profile_allows_tool(profile: &cyber_core::config::AgentProfile, name: &str) -> bool {
    let matches = |patterns: &[String]| {
        patterns
            .iter()
            .any(|pattern| cyber_core::wildcard::matches(pattern, name))
    };
    !profile.hidden
        && !matches(&profile.tools.deny)
        && profile
            .tools
            .allow
            .as_ref()
            .is_none_or(|allow| matches(allow))
}

impl BuiltinHost {
    /// Some freshly authorized, connected server in the Turn's Location advertises
    /// resources and is not wholly denied by a `read` rule for `mcp:<server>:*`.
    fn mcp_resources_offered(&self, turn: &TurnContext, rules: &[permissions::Rule]) -> bool {
        self.mcp_resource_servers(Path::new(&turn.directory))
            .iter()
            .any(|server| !resource_server_denied(rules, server))
    }
}

/// The deciding `read` rule for `mcp:<server>:*` denies and covers every URI of it.
fn resource_server_denied(rules: &[permissions::Rule], server: &str) -> bool {
    let prefix = format!("mcp:{server}:");
    matches!(
        permissions::evaluate(rules, "read", &format!("{prefix}*")),
        (Effect::Deny, Some(rule))
            if rule.resource.ends_with('*') && cyber_core::wildcard::matches(&rule.resource, &prefix)
    )
}

/// Hidden when the last rule matching the action with resource `*` denies it.
pub(crate) fn fully_denied(rules: &[permissions::Rule], action: &str) -> bool {
    fully_denied_scoped(rules, action, action)
}

fn fully_denied_scoped(rules: &[permissions::Rule], action: &str, tool: &str) -> bool {
    matches!(permissions::evaluate_scoped(rules, action, "*", Some(tool)), (Effect::Deny, Some(rule)) if rule.resource == "*")
}

impl BuiltinHost {
    /// Current policy and the call context for one attempt of a built-in call.
    async fn call_ctx<'a>(
        &'a self,
        inv: &'a Invocation,
        hook_decision: Option<HookAction>,
        cancel: CancellationToken,
        denial: DenialHooks,
    ) -> Result<Ctx<'a>, String> {
        let policy = self.policy(inv).await?;
        if let Some(reason) = policy.skill_tool_denial(&inv.name) {
            return Err(reason);
        }
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            host: self,
            policy,
            location: PathBuf::from(&inv.directory),
            inv,
            cancel,
            hook_decision,
            denial,
        })
    }

    async fn run_builtin(&self, tool: &dyn Tool, ctx: &Ctx<'_>) -> ToolOutcome {
        let keep_tail = matches!(ctx.inv.name.as_str(), "bash" | "powershell");
        match tool.run(ctx).await {
            Ok(output) => self.settle_output(
                ctx,
                output,
                keep_tail
                    || ctx
                        .compiler_feedback
                        .load(std::sync::atomic::Ordering::Acquire),
            ),
            Err(ToolError::Failed(message)) => ToolOutcome::Failed(message),
            Err(ToolError::Aborted) => ToolOutcome::Aborted,
        }
    }

    /// Post-tool hooks and skill suggestions for the attempt that settled the call.
    async fn settle_call(
        &self,
        ctx: &Ctx<'_>,
        outcome: ToolOutcome,
        started: std::time::Instant,
    ) -> ToolOutcome {
        match self
            .post_tool_hooks(ctx.inv, &outcome, started, ctx.cancel.clone())
            .await
        {
            Ok(()) => self.path_skill_suggestions(ctx, outcome),
            Err(ToolError::Aborted) => self.path_skill_suggestions(ctx, ToolOutcome::Aborted),
            Err(ToolError::Failed(error)) => self.path_skill_suggestions(
                ctx,
                ToolOutcome::Failed(format!("Tool settled; post-hook failed: {error}")),
            ),
        }
    }

    /// The re-issued invocation of a PermissionDenied retry, validated like any input,
    /// and the hook decision its authorization starts from. Changed input is new input:
    /// PreToolUse runs on it like on a first attempt, so a guard's deny, block, ask or
    /// rewrite applies to what actually runs. A hook allow never carries to a retry.
    pub(crate) async fn retried_invocation(
        &self,
        inv: &Invocation,
        schema: &Value,
        input: Value,
        original: Option<HookAction>,
        cancel: CancellationToken,
    ) -> Result<(Invocation, Option<HookAction>), ToolError> {
        let invalid = |error: String| ToolError::Failed(error);
        crate::schema::validate(schema, &input)
            .map_err(|_| invalid("hook produced invalid tool input".into()))?;
        inv.asker
            .validate_auto_override(&inv.name, &input)
            .map_err(|error| invalid(error.to_string()))?;
        let changed = input != inv.input;
        let mut retried = inv.clone();
        retried.input = input;
        let mut asks = original == Some(HookAction::Ask);
        if changed {
            asks |=
                self.pre_tool_hooks(&mut retried, schema, cancel).await? == Some(HookAction::Ask);
        }
        self.check_skill_tool(&retried).await.map_err(invalid)?;
        self.check_agent_tool(&retried).map_err(invalid)?;
        Ok((retried, asks.then_some(HookAction::Ask)))
    }
}

/// Everything a tool needs for one call.
pub(crate) struct Ctx<'a> {
    pub skill_paths: Mutex<Vec<PathBuf>>,
    pub loaded_skill: Mutex<Option<skills::SkillActivation>>,
    pub compiler_feedback: std::sync::atomic::AtomicBool,
    pub host: &'a BuiltinHost,
    pub inv: &'a Invocation,
    pub policy: Policy,
    pub location: PathBuf,
    pub cancel: CancellationToken,
    pub hook_decision: Option<cyber_core::hooks::HookAction>,
    pub denial: DenialHooks,
}

/// PermissionDenied for one tool-call attempt. Only a call's first attempt is armed,
/// so the event runs at most once per call; a hook's `retry` waits here for the
/// caller to re-validate and re-issue it.
#[derive(Default)]
pub(crate) struct DenialHooks {
    armed: std::sync::atomic::AtomicBool,
    retry: Mutex<Option<Value>>,
}

impl DenialHooks {
    pub(crate) fn armed() -> Self {
        Self {
            armed: std::sync::atomic::AtomicBool::new(true),
            retry: Mutex::default(),
        }
    }

    /// The input to re-issue once, kept only when the attempt failed.
    pub(crate) fn take_retry(&self, outcome: &ToolOutcome) -> Option<Value> {
        let retry = self
            .retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        retry.filter(|_| matches!(outcome, ToolOutcome::Failed(_)))
    }
}

impl Ctx<'_> {
    pub fn note_skill_path(&self, path: &Path) {
        self.skill_paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.into());
    }

    /// Resolve a tool path against the Location (`~/` expands to home), lexically normalized.
    pub fn resolve(&self, path: &str) -> PathBuf {
        resolve_path(&self.location, &self.host.opts.home, path)
    }

    /// The permission resource for a path: Location-relative inside, absolute outside.
    pub fn resource(&self, path: &Path) -> String {
        path.strip_prefix(&self.location)
            .map_or_else(|_| permissions::slash(path), permissions::slash)
    }

    /// Ask for `external_directory` for any path whose canonical form leaves the Location.
    pub async fn check_external(&self, paths: &[PathBuf]) -> Result<(), ToolError> {
        let location =
            std::fs::canonicalize(&self.location).unwrap_or_else(|_| self.location.clone());
        let mut dirs: Vec<String> = Vec::new();
        for path in paths {
            let canonical = canonical(path);
            if canonical.starts_with(&location) || canonical.starts_with(&self.location) {
                continue;
            }
            let dir = if canonical.is_dir() {
                canonical.clone()
            } else {
                canonical
                    .parent()
                    .map_or(canonical.clone(), Path::to_path_buf)
            };
            let pattern = format!("{}/*", permissions::slash(&dir));
            if !dirs.contains(&pattern) {
                dirs.push(pattern);
            }
        }
        if dirs.is_empty() {
            return Ok(());
        }
        let req = Request {
            action: "external_directory".into(),
            resources: dirs.clone(),
            read_only: true,
            ..Request::default()
        };
        self.authorize(req, dirs, Value::Null).await
    }

    /// Decide, asking the user when the policy says `ask`.
    /// Refuse `req` when a rule denies it, without asking: the deny part of
    /// [`Self::authorize`].
    pub async fn refuse_denied(&self, mut req: Request) -> Result<(), ToolError> {
        req.tool = Some(self.inv.name.clone());
        match self.policy.rule_denies(&req) {
            Some(reason) => Err(self.denied(&req, "rule", deny_message(&reason)).await),
            None => Ok(()),
        }
    }

    pub async fn authorize(
        &self,
        mut req: Request,
        always: Vec<String>,
        mut metadata: Value,
    ) -> Result<(), ToolError> {
        req.tool = Some(self.inv.name.clone());
        let decision = self.policy.decide(&req);
        if let Decision::Deny(reason) = decision {
            return Err(self.denied(&req, "rule", deny_message(&reason)).await);
        }
        if self.hook_decision == Some(cyber_core::hooks::HookAction::Ask) {
            // Force the manual route without skipping current/ancestor auto blocks.
            self.auto_gate(&req, &mut metadata, false).await?;
            return self.ask(req, always, metadata).await;
        }
        let needs_approval = (decision == Decision::Ask
            && self.hook_decision != Some(cyber_core::hooks::HookAction::Allow))
            || self.policy.forces_classifier(&req);
        if self.auto_gate(&req, &mut metadata, needs_approval).await? || !needs_approval {
            return Ok(());
        }
        self.ask(req, always, metadata).await
    }

    /// Current and ancestor auto-mode review; an auto block is a denial boundary.
    async fn auto_gate(
        &self,
        req: &Request,
        metadata: &mut Value,
        needs_approval: bool,
    ) -> Result<bool, ToolError> {
        match self
            .review_auto_permission(req, metadata, needs_approval)
            .await
        {
            Err(ToolError::Failed(message)) if message.starts_with("Blocked by auto mode") => {
                Err(self.denied(req, "auto", message).await)
            }
            other => other,
        }
    }

    /// The denial error, after PermissionDenied hooks on an armed attempt; a merged
    /// `retry` is kept for the caller, never applied here.
    async fn denied(&self, req: &Request, source: &str, message: String) -> ToolError {
        if !self
            .denial
            .armed
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return ToolError::Failed(message);
        }
        let denial = crate::tool_hooks::PermissionDenial {
            source,
            reason: &message,
            action: &req.action,
            resources: &req.resources,
        };
        match Box::pin(
            self.host
                .permission_denied_hooks(self.inv, &denial, self.cancel.clone()),
        )
        .await
        {
            Ok(Some(input)) => {
                *self
                    .denial
                    .retry
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(input);
            }
            Ok(None) | Err(ToolError::Failed(_)) => {}
            Err(ToolError::Aborted) => return ToolError::Aborted,
        }
        ToolError::Failed(message)
    }

    async fn ask(
        &self,
        req: Request,
        always: Vec<String>,
        metadata: Value,
    ) -> Result<(), ToolError> {
        let ask = PermissionAsk {
            action: req.action.clone(),
            resources: req.resources.clone(),
            always_patterns: always.clone(),
            metadata,
        };
        // Bound stack usage in nested tool and child-setup futures.
        if Box::pin(
            self.host
                .permission_request_hooks(self.inv, &ask, self.cancel.clone()),
        )
        .await?
            == Some(cyber_core::hooks::HookAction::Allow)
        {
            return Ok(());
        }
        Box::pin(self.host.notification_hooks(
            self.inv,
            "permission_prompt",
            format!(
                "Permission requested: {} {}",
                req.action,
                req.resources.join(", ")
            ),
            self.cancel.clone(),
        ))
        .await;
        let reply = tokio::select! {
            _ = self.cancel.cancelled() => return Err(ToolError::Aborted),
            reply = self.inv.asker.permission(ask) => reply,
        };
        match reply {
            PermissionReply::Once => Ok(()),
            PermissionReply::Always => {
                let root = cyber_core::config::project_root(&self.location);
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                saved::save(
                    &self.host.opts.store,
                    &root,
                    &req.action,
                    &always,
                    &self.inv.session_id,
                )
                .map_err(|e| ToolError::Failed(format!("could not save the approval: {e}")))
            }
            PermissionReply::Reject { message: Some(m) } => Err(self
                .denied(&req, "user", format!("Rejected by user: {m}"))
                .await),
            PermissionReply::Reject { message: None } => {
                Err(self.denied(&req, "user", "Rejected by user".into()).await)
            }
            PermissionReply::Unattended => {
                let message = match self.host.runtime() {
                    Some(runtime)
                        if runtime
                            .session_workflow_run(&self.inv.session_id)
                            .await
                            .is_some() =>
                    {
                        cyber_server::runtime::NO_WORKFLOW_APPROVER.to_owned()
                    }
                    _ => unattended(self.policy.mode),
                };
                Err(self.denied(&req, "unattended", message).await)
            }
        }
    }
}

fn deny_message(reason: &str) -> String {
    if reason.starts_with("Plan mode")
        || reason.starts_with("Not pre-approved")
        || reason.starts_with("Refused:")
        || reason.starts_with(cyber_server::runtime::teams::OWNED_PREFIX)
    {
        reason.to_string()
    } else {
        format!("Permission denied: {reason}")
    }
}

/// `permissions-modes` → Non-interactive behavior.
fn unattended(mode: Mode) -> String {
    match mode {
        Mode::Auto => {
            "Blocked by auto mode: explicit approval required and no approver is attached".into()
        }
        _ => "Denied: no interactive approver is attached to this session".into(),
    }
}

/// Resolve a tool path against a Location (`~/` expands to home), lexically normalized.
pub(crate) fn resolve_path(location: &Path, home: &Path, path: &str) -> PathBuf {
    let p = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        location.join(path)
    };
    crate::bash_analysis::normalize(&p)
}

const SKILLS_KEY: &str = "core/skills";
/// Skills the selected agent's profile preloads (`agents-subagents` → `skills`).
const AGENT_SKILLS_KEY: &str = "core/agent-skills";
const MCP_INSTRUCTIONS_KEY: &str = "core/mcp-instructions";
/// The key used before `system-context` named it `core/mcp-instructions`.
const LEGACY_MCP_INSTRUCTIONS_KEY: &str = "mcp/instructions";

/// Canonicalize the longest existing ancestor so symlinks cannot hide an escape.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

impl BuiltinHost {
    /// Context sources projected from an already effective tool catalog.
    pub fn context_sources_for_tools(
        &self,
        turn: &TurnContext,
        visible: &[ToolDef],
    ) -> BTreeMap<String, String> {
        let location = Path::new(&turn.directory);
        let budget = self.listing_budget(location);
        // The selected agent's profile and the Session ruleset decide what is listed and
        // preloaded; an unresolvable agent gets neither.
        let rules = self
            .session_rules(location, Some(&turn.agent), &turn.rules)
            .ok();
        let found = self.skills(location);
        let mut sources = BTreeMap::new();
        if let Some(rules) = &rules {
            if let Some(text) = skill_catalog::skill_listing(&found, rules, budget) {
                sources.insert(SKILLS_KEY.to_string(), text);
            }
            if let Some(text) = self.preloaded_skills(location, &turn.agent, &found, rules) {
                sources.insert(AGENT_SKILLS_KEY.to_string(), text);
            }
        }
        if let Some(instructions) = self.mcp_instructions(turn, visible) {
            sources.insert(MCP_INSTRUCTIONS_KEY.into(), instructions);
        }
        // `cross-session-messaging` → Permission boundaries stay per session: the guidance
        // rides with the built-in messaging tools, never with a client tool of that name.
        if visible.iter().any(|d| {
            d.spec.name == "send_message" && d.scope == cyber_server::runtime::ToolScope::Builtin
        }) {
            sources.insert(
                tools::messaging::GUIDANCE_KEY.into(),
                tools::messaging::GUIDANCE.into(),
            );
        }
        if let Some(text) = tools::team::guidance(self, &turn.session_id) {
            sources.insert(tools::team::GUIDANCE_KEY.into(), text);
        }
        if visible.iter().any(|d| {
            d.spec.name == tools::loops::NAME
                && d.scope == cyber_server::runtime::ToolScope::Builtin
        }) {
            sources.insert(
                tools::loops::GUIDANCE_KEY.into(),
                tools::loops::GUIDANCE.into(),
            );
        }
        sources
    }

    /// The bodies of the skills the agent's profile preloads, as the skill tool frames them.
    fn preloaded_skills(
        &self,
        location: &Path,
        agent: &str,
        found: &Discovery,
        rules: &[permissions::Rule],
    ) -> Option<String> {
        let profile = self.agent_profile(location, agent).ok()?;
        let (frames, _) = skill_catalog::preload(found, rules, &profile.skills);
        (!frames.is_empty()).then(|| {
            format!(
                "Skills preloaded for this agent:\n\n{}",
                frames.join("\n\n")
            )
        })
    }

    /// Typed observations include explicit withdrawal of absent MCP instructions.
    pub fn context_observations_for_tools(
        &self,
        turn: &TurnContext,
        visible: &[ToolDef],
    ) -> BTreeMap<String, cyber_server::runtime::ContextObservation> {
        use cyber_server::runtime::ContextObservation;
        let mut sources: BTreeMap<_, _> = self
            .context_sources_for_tools(turn, visible)
            .into_iter()
            .map(|(key, value)| (key, ContextObservation::Value(value)))
            .collect();
        for key in [
            SKILLS_KEY,
            AGENT_SKILLS_KEY,
            MCP_INSTRUCTIONS_KEY,
            tools::messaging::GUIDANCE_KEY,
            tools::loops::GUIDANCE_KEY,
            tools::team::GUIDANCE_KEY,
        ] {
            sources
                .entry(key.into())
                .or_insert(ContextObservation::Absent);
        }
        // Sessions started before the rename withdraw the old key deterministically.
        sources.insert(
            LEGACY_MCP_INSTRUCTIONS_KEY.into(),
            ContextObservation::Absent,
        );
        let agent = match self.agent_profile(Path::new(&turn.directory), &turn.agent) {
            Ok(profile) => match profile.system.filter(|text| !text.is_empty()) {
                Some(text) => ContextObservation::Value(text),
                None => ContextObservation::Absent,
            },
            Err(error) => ContextObservation::Unavailable(error),
        };
        sources.insert("core/agent".into(), agent);
        sources
    }
}
