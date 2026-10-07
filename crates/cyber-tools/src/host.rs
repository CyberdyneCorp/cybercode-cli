//! The built-in tool host (`tool-registry`, `builtin-tools`, `permissions-modes`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cyber_core::env::EnvSource;
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
}

pub struct BuiltinHost {
    pub(crate) opts: HostOptions,
    tools: Vec<Box<dyn Tool>>,
    reads: Mutex<HashMap<String, HashSet<PathBuf>>>,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    runtime: OnceLock<WeakRuntime>,
    pub(crate) search_cooldowns: tools::websearch::Cooldowns,
    /// Domains approved for sandboxed network access, by Session.
    network: Mutex<HashMap<String, Arc<Mutex<HashSet<String>>>>>,
}

impl BuiltinHost {
    pub fn new(opts: HostOptions) -> Arc<Self> {
        Arc::new(Self {
            opts,
            tools: tools::all(),
            reads: Mutex::default(),
            locks: Mutex::default(),
            runtime: OnceLock::new(),
            search_cooldowns: tools::websearch::Cooldowns::default(),
            network: Mutex::default(),
        })
    }

    /// Late-bind the runtime for tools that act on the Session (plan mode switches).
    pub fn attach(&self, runtime: Runtime) {
        let _ = self.runtime.set(runtime.downgrade());
    }

    pub(crate) fn runtime(&self) -> Option<Runtime> {
        self.runtime.get().and_then(WeakRuntime::upgrade)
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

    fn rules(&self, location: &Path) -> Vec<permissions::Rule> {
        let (config, sources) = (self.opts.config)(location).unwrap_or_default();
        self.rules_for(location, true, &config, &sources)
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
            self.skills_for(location, config)
                .skills
                .into_values()
                .map(|s| s.base),
        );
        let mut rules = permissions::defaults(&allowed, primary_agent);
        rules.extend(permissions::parse_rules(&config["permissions"], sources));
        rules
    }

    /// Skills a user can invoke as `/name` from a Location: `(name, description, argument hint)`.
    pub fn skill_commands(&self, location: &Path) -> Vec<(String, String, Option<String>)> {
        self.skills(location)
            .skills
            .into_values()
            .filter(|s| s.user_invocable)
            .map(|s| (s.name, s.description, s.argument_hint))
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

    /// Skills visible from a Location, honoring `skills.compat` and `skills.paths`.
    pub(crate) fn skills(&self, location: &Path) -> Discovery {
        let config = self.config_for(location);
        self.skills_for(location, &config)
    }

    fn skills_for(&self, location: &Path, config: &Value) -> Discovery {
        let settings = &config["skills"];
        let entries = if settings.is_array() {
            settings
        } else {
            &settings["paths"]
        };
        let root = cyber_core::config::project_root(location);
        let extra = entries
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|e| !e.starts_with("https://") && !e.starts_with("http://"))
            .map(|e| match e.strip_prefix("~/") {
                Some(rest) => self.opts.home.join(rest),
                None => root.join(e),
            })
            .collect();
        skills::discover(&SkillScope {
            location: location.to_path_buf(),
            home: self.opts.home.clone(),
            global_config_dir: self.opts.global_config_dir.clone(),
            compat: settings
                .get("compat")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            extra,
        })
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
        Ok(rules)
    }

    pub(crate) async fn policy(&self, inv: &Invocation) -> Result<Policy, String> {
        self.policy_for(inv, Some(&inv.agent)).await
    }

    async fn policy_for(&self, inv: &Invocation, agent: Option<&str>) -> Result<Policy, String> {
        let location = PathBuf::from(&inv.directory);
        let root = cyber_core::config::project_root(&location);
        let mut rules = self.session_rules(&location, agent, &inv.rules)?;
        let inherited = if agent.is_some() {
            self.inherited_permissions(&inv.session_id).await?
        } else {
            Default::default()
        };
        rules.extend(inherited.rules);
        Ok(Policy {
            rules,
            saved: saved::rules(&self.opts.store, &root).unwrap_or_default(),
            mode: Mode::parse(&inv.mode),
            parent_modes: inherited.modes,
            plan_file: location
                .join(".cyber/plans")
                .join(format!("{}.md", inv.session_id)),
            location,
            home: self.opts.home.clone(),
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

/// `<available_skills>` for permitted, model-invocable skills within a character budget.
fn skill_listing(found: &Discovery, rules: &[permissions::Rule], budget: usize) -> Option<String> {
    let listed: Vec<&skills::Skill> = found
        .skills
        .values()
        .filter(|s| !s.disable_model_invocation)
        .filter(|s| permissions::evaluate(rules, "skill", &s.name).0 != Effect::Deny)
        .collect();
    if listed.is_empty() {
        return None;
    }
    let full: Vec<String> = listed
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect();
    let fits = full.iter().map(|l| l.len() + 1).sum::<usize>() <= budget;
    let lines = if fits {
        full
    } else {
        shortened(&listed, budget)
    };
    Some(format!(
        "Skills you can load with the skill tool:\n<available_skills>\n{}\n</available_skills>",
        lines.join("\n")
    ))
}

/// Over budget: descriptions cut to 120 characters, then the remaining names without them.
fn shortened(listed: &[&skills::Skill], budget: usize) -> Vec<String> {
    let mut used = 0;
    listed
        .iter()
        .map(|s| {
            let short: String = s.description.chars().take(120).collect();
            let line = format!("- {}: {short}", s.name);
            used += line.len() + 1;
            if used <= budget {
                line
            } else {
                format!("- {}", s.name)
            }
        })
        .collect()
}

impl ToolHost for BuiltinHost {
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

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let Ok(rules) =
            self.session_rules(Path::new(&turn.directory), Some(&turn.agent), &turn.rules)
        else {
            return Vec::new();
        };
        let mode = Mode::parse(&turn.mode);
        let tools = self
            .tools
            .iter()
            .map(|t| t.def())
            .filter(|d| offered(&d.spec.name, turn.prefers_apply_patch, mode, d.retry_safety))
            .filter(|d| !fully_denied(&rules, tools::action_of(&d.spec.name)))
            .filter(|d| {
                d.spec.name != "websearch" || self.websearch_available(Path::new(&turn.directory))
            })
            .filter(|d| {
                d.spec.name != "powershell"
                    || tools::powershell::installed(Path::new(&turn.directory)).is_some()
            })
            .collect();
        self.filter_agent_tools(turn, tools)
    }

    fn reconcile(&self, directory: &str, call: &CallState) -> BoxFuture<'_, Reconciliation> {
        let result = crate::reconcile::reconcile(Path::new(directory), &self.opts.home, call);
        Box::pin(async move { result })
    }

    fn context_sources(&self, turn: &TurnContext) -> BTreeMap<String, String> {
        let location = Path::new(&turn.directory);
        let budget = self.listing_budget(location);
        let listing = skill_listing(&self.skills(location), &self.rules(location), budget);
        listing
            .map(|text| BTreeMap::from([("core/skills".to_string(), text)]))
            .unwrap_or_default()
    }

    fn context_observations(
        &self,
        turn: &TurnContext,
    ) -> BTreeMap<String, cyber_server::runtime::ContextObservation> {
        use cyber_server::runtime::ContextObservation;
        let mut sources: BTreeMap<_, _> = self
            .context_sources(turn)
            .into_iter()
            .map(|(key, value)| (key, ContextObservation::Value(value)))
            .collect();
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

    fn execute(&self, inv: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async move {
            let Some(tool) = self.tools.iter().find(|t| t.def().spec.name == inv.name) else {
                return ToolOutcome::Failed(format!("Unknown tool: {}", inv.name));
            };
            if let Err(error) = self.check_agent_tool(&inv) {
                return ToolOutcome::Failed(error);
            }
            if let Err(message) = crate::schema::validate(&tool.def().spec.input_schema, &inv.input)
            {
                return ToolOutcome::Failed(message);
            }
            let policy = match self.policy(&inv).await {
                Ok(policy) => policy,
                Err(error) => return ToolOutcome::Failed(error),
            };
            let ctx = Ctx {
                host: self,
                policy,
                location: PathBuf::from(&inv.directory),
                inv: &inv,
                cancel,
            };
            let keep_tail = matches!(inv.name.as_str(), "bash" | "powershell");
            match tool.run(&ctx).await {
                Ok(text) => match self.budget(&ctx.location).apply(text, keep_tail) {
                    Ok(text) => ToolOutcome::Ok(text),
                    Err(e) => ToolOutcome::Crashed(e),
                },
                Err(ToolError::Failed(message)) => ToolOutcome::Failed(message),
                Err(ToolError::Aborted) => ToolOutcome::Aborted,
            }
        })
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
        let inv = Invocation {
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
        };
        let command = command.to_string();
        Box::pin(async move {
            let ctx = Ctx {
                host: self,
                policy: self.policy_for(&inv, None).await?,
                location: PathBuf::from(&inv.directory),
                inv: &inv,
                cancel,
            };
            let output = tools::bash::run_user(&ctx, &command)
                .await
                .map_err(|e| match e {
                    ToolError::Failed(message) => message,
                    ToolError::Aborted => "aborted".into(),
                })?;
            self.budget(&ctx.location).apply(output, true)
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
        (Mode::Plan, "write" | "plan_exit" | "todo") => true,
        (Mode::Plan, "plan_enter") => false,
        (Mode::Plan, _) => safety == cyber_server::runtime::RetrySafety::ReadOnly,
        (_, "plan_exit") => false,
        _ => true,
    };
    by_model && by_mode
}

fn profile_allows_tool(profile: &cyber_core::config::AgentProfile, name: &str) -> bool {
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

/// Hidden when the last rule matching the action with resource `*` denies it.
fn fully_denied(rules: &[permissions::Rule], action: &str) -> bool {
    matches!(permissions::evaluate(rules, action, "*"), (Effect::Deny, Some(rule)) if rule.resource == "*")
}

/// Everything a tool needs for one call.
pub(crate) struct Ctx<'a> {
    pub host: &'a BuiltinHost,
    pub inv: &'a Invocation,
    pub policy: Policy,
    pub location: PathBuf,
    pub cancel: CancellationToken,
}

impl Ctx<'_> {
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
    pub async fn authorize(
        &self,
        req: Request,
        always: Vec<String>,
        metadata: Value,
    ) -> Result<(), ToolError> {
        match self.policy.decide(&req) {
            Decision::Allow => Ok(()),
            Decision::Deny(reason) => Err(ToolError::Failed(deny_message(&reason))),
            Decision::Ask => self.ask(req, always, metadata).await,
        }
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
            PermissionReply::Reject { message: Some(m) } => {
                Err(ToolError::Failed(format!("Rejected by user: {m}")))
            }
            PermissionReply::Reject { message: None } => {
                Err(ToolError::Failed("Rejected by user".into()))
            }
            PermissionReply::Unattended => Err(ToolError::Failed(unattended(self.policy.mode))),
        }
    }
}

fn deny_message(reason: &str) -> String {
    if reason.starts_with("Plan mode")
        || reason.starts_with("Not pre-approved")
        || reason.starts_with("Refused:")
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
            "Blocked by auto mode: no approver is available and the classifier arrives in P1".into()
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
