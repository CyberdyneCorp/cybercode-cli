//! Rules, wildcard evaluation and Mode effects (`permissions-modes`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyber_core::wildcard::matches;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Ask,
    Deny,
}

impl Effect {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// One ordered rule. `source` is the config layer it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv_prefix: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub action: String,
    pub resource: String,
    pub effect: Effect,
    pub source: String,
}

impl Rule {
    pub fn new(action: &str, resource: &str, effect: Effect, source: &str) -> Self {
        Self {
            tool_pattern: None,
            argv_prefix: None,
            tool: None,
            action: action.into(),
            resource: resource.into(),
            effect,
            source: source.into(),
        }
    }
}

/// Parse the `permissions` config value, preserving written order:
/// `"ask"` ≡ `{"*": "ask"}`; a map `action → effect | { pattern → effect }`,
/// or an array of `{ action, resource, effect }` rules.
/// `sources` maps JSON pointers to config layer labels.
pub fn parse_rules(value: &Value, sources: &BTreeMap<String, String>) -> Vec<Rule> {
    let source_of = |pointer: &str| {
        sources
            .get(pointer)
            .cloned()
            .unwrap_or_else(|| "config".into())
    };
    match value {
        Value::String(s) => Effect::parse(s)
            .map(|e| vec![Rule::new("*", "*", e, &source_of("/permissions"))])
            .unwrap_or_default(),
        Value::Object(map) => map
            .iter()
            .filter(|(action, _)| action.as_str() != "auto_mode")
            .flat_map(|(action, spec)| {
                if action == "rules"
                    && let Some(items) = spec.as_array()
                {
                    items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| {
                            ordered_rule(&format!("/permissions/rules/{index}"), item, sources)
                        })
                        .collect()
                } else {
                    action_rules(action, spec, &source_of)
                }
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                ordered_rule(&format!("/permissions/{index}"), item, sources)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn ordered_rule(base: &str, item: &Value, sources: &BTreeMap<String, String>) -> Option<Rule> {
    let source = sources
        .get(&format!("{base}/effect"))
        .or_else(|| sources.get(base))
        .or_else(|| sources.get("/permissions"))
        .map_or("config", String::as_str);
    let mut rule = Rule::new(
        item.get("action")?.as_str()?,
        item.get("resource")?.as_str()?,
        Effect::parse(item.get("effect")?.as_str()?)?,
        source,
    );
    if let Some(scope) = item.get("tool") {
        match scope
            .as_str()
            .filter(|tool| !tool.is_empty() && !tool.chars().any(char::is_control))
        {
            Some(tool) => rule.tool = Some(tool.into()),
            None => rule.effect = Effect::Deny,
        }
    }
    if let Some(prefix) = item.get("argv_prefix") {
        match prefix
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 128)
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .filter(|token| {
                                !token.is_empty()
                                    && token.len() <= 4096
                                    && !token.chars().any(char::is_control)
                            })
                            .map(str::to_owned)
                    })
                    .collect::<Option<Vec<_>>>()
            }) {
            Some(prefix) => rule.argv_prefix = Some(prefix),
            None => rule.effect = Effect::Deny,
        }
    }
    Some(rule)
}

fn action_rules(action: &str, spec: &Value, source_of: &dyn Fn(&str) -> String) -> Vec<Rule> {
    let base = format!("/permissions/{}", escape(action));
    match spec {
        Value::String(s) => Effect::parse(s)
            .map(|e| vec![Rule::new(action, "*", e, &source_of(&base))])
            .unwrap_or_default(),
        Value::Object(patterns) => patterns
            .iter()
            .filter_map(|(pattern, effect)| {
                let effect = Effect::parse(effect.as_str()?)?;
                Some(Rule::new(
                    action,
                    pattern,
                    effect,
                    &source_of(&format!("{base}/{}", escape(pattern))),
                ))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The last rule whose action and resource match, else `ask`.
pub fn evaluate<'a>(rules: &'a [Rule], action: &str, resource: &str) -> (Effect, Option<&'a Rule>) {
    evaluate_scoped(rules, action, resource, None)
}

/// A scoped rule requires the exact bound invocation; unscoped rules keep their meaning.
pub fn evaluate_scoped<'a>(
    rules: &'a [Rule],
    action: &str,
    resource: &str,
    tool: Option<&str>,
) -> (Effect, Option<&'a Rule>) {
    // Decode a resource once even when an imported batch contains many prefixes.
    let argv = rules
        .iter()
        .any(|rule| rule.argv_prefix.is_some() && matches(&rule.action, action))
        .then(|| crate::bash_analysis::literal_argv(resource))
        .flatten();
    rules
        .iter()
        .rev()
        .find(|rule| rule_matches_argv(rule, action, resource, tool, argv.as_deref()))
        .map_or((Effect::Ask, None), |rule| (rule.effect, Some(rule)))
}

fn rule_matches(rule: &Rule, action: &str, resource: &str, tool: Option<&str>) -> bool {
    let argv = rule
        .argv_prefix
        .as_ref()
        .and_then(|_| crate::bash_analysis::literal_argv(resource));
    rule_matches_argv(rule, action, resource, tool, argv.as_deref())
}

fn rule_matches_argv(
    rule: &Rule,
    action: &str,
    resource: &str,
    tool: Option<&str>,
    argv: Option<&[String]>,
) -> bool {
    matches(&rule.action, action)
        && resource_matches(rule, resource, argv)
        && rule.tool.as_deref().is_none_or(|scope| Some(scope) == tool)
        && rule
            .tool_pattern
            .as_deref()
            .is_none_or(|pattern| tool.is_some_and(|tool| matches(pattern, tool)))
}

fn resource_matches(rule: &Rule, resource: &str, argv: Option<&[String]>) -> bool {
    let Some(prefix) = &rule.argv_prefix else {
        return matches(&rule.resource, resource);
    };
    argv.map_or(rule.effect != Effect::Allow, |argv| {
        argv.starts_with(prefix)
    })
}

fn request_effect(rules: &[Rule], req: &Request) -> Effect {
    req.resources
        .iter()
        .map(|resource| evaluate_scoped(rules, &req.action, resource, req.tool.as_deref()).0)
        .max_by_key(|effect| match effect {
            Effect::Allow => 0,
            Effect::Ask => 1,
            Effect::Deny => 2,
        })
        .unwrap_or(Effect::Ask)
}

/// Several resources: `deny` if any is denied, else `ask` if any asks, else `allow`.
pub fn evaluate_all(rules: &[Rule], action: &str, resources: &[String]) -> Effect {
    resources
        .iter()
        .map(|r| evaluate(rules, action, r).0)
        .max_by_key(|e| match e {
            Effect::Allow => 0,
            Effect::Ask => 1,
            Effect::Deny => 2,
        })
        .unwrap_or(Effect::Ask)
}

/// Starting a Workflow Run, with the workflow name as resource (`workflows` → Permissions
/// for runs).
pub const WORKFLOW_RUN: &str = "workflow.run";

/// Sending a cross-session message, with the target's canonical address and its name as
/// resources (`cross-session-messaging` → Send message tool).
pub const MESSAGE_SEND: &str = "message.send";

/// Spawning teammates and merging their work (`agent-teams` → Permissions in teams):
/// allowed for primary agents, denied for subagents; the runtime refuses teammates.
pub const TEAM_MANAGE: &str = "team.manage";

/// The model setting a Goal on its own Session through the `goal` tool (`goals` → Set
/// the active goal): asks by default; the user's own `/goal` routes never check it.
pub const GOAL_SET: &str = "goal.set";

/// The shared task board of a team (`agent-teams` → Shared task list): allowed, and
/// offered to team members only; the runtime enforces the board's own rules.
pub const TEAM_TASK: &str = "team.task";

/// The `workflow.run` resource of a module (`workflows` → Permissions for runs): the
/// name of a global or bundled saved Workflow, `project:<name>` for a project one and
/// `inline:<name>@<digest>` for an inline script. A script declares its own name, so a
/// rule or an "always" approval for one origin never covers a script of another origin
/// (or another inline script) that merely declares the same name. Deny rules for the
/// bare name hold for every origin.
pub fn workflow_resource(module: &cyber_workflow::WorkflowModule) -> String {
    use cyber_workflow::{Origin, Scope};
    let name = &module.meta.name;
    match &module.origin {
        Origin::Saved {
            scope: Scope::Global,
            ..
        } => name.clone(),
        Origin::Saved {
            scope: Scope::Project,
            ..
        } => format!("project:{name}"),
        Origin::Inline => format!("inline:{name}@{}", module.digest),
    }
}

/// Defaults without configuration (`permissions-modes` → Default rules).
pub fn defaults(allowed_dirs: &[PathBuf], primary_agent: bool) -> Vec<Rule> {
    let mut rules = vec![Rule::new("*", "*", Effect::Ask, "default")];
    for action in [
        "read",
        "glob",
        "grep",
        "list",
        "todo",
        "skill",
        "history_search",
        "wait_for_mcp",
        "tool_search",
        "notify",
        "list_sessions",
        "watch_session",
        TEAM_TASK,
    ] {
        rules.push(Rule::new(action, "*", Effect::Allow, "default"));
    }
    for action in ["read", "edit"] {
        rules.push(Rule::new(action, "*.env", Effect::Ask, "default"));
        rules.push(Rule::new(action, "*.env.*", Effect::Ask, "default"));
        rules.push(Rule::new(action, "*.env.example", Effect::Allow, "default"));
    }
    rules.push(Rule::new("external_directory", "*", Effect::Ask, "default"));
    // Explicit, so inspection names its source; `auto` and `bypass` allow it.
    rules.push(Rule::new(WORKFLOW_RUN, "*", Effect::Ask, "default"));
    // Same-server targets are allowed; a cross-machine address (`name@machine`) asks.
    rules.push(Rule::new(MESSAGE_SEND, "*", Effect::Allow, "default"));
    rules.push(Rule::new(MESSAGE_SEND, "*@*", Effect::Ask, "default"));
    for dir in allowed_dirs {
        rules.push(Rule::new(
            "external_directory",
            &format!("{}/*", slash(dir)),
            Effect::Allow,
            "default",
        ));
    }
    let interactive = if primary_agent {
        Effect::Allow
    } else {
        Effect::Deny
    };
    for action in ["question", "plan_enter", "plan_exit", TEAM_MANAGE] {
        rules.push(Rule::new(action, "*", interactive, "default"));
    }
    rules
}

/// A permission Mode (`permissions-modes` → Permission modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Default,
    AcceptEdits,
    Plan,
    Auto,
    DontAsk,
    Bypass,
}

impl Mode {
    pub fn parse(value: &str) -> Self {
        Self::checked_parse(value).unwrap_or(Self::Default)
    }

    pub fn checked_parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "accept-edits" => Some(Self::AcceptEdits),
            "plan" => Some(Self::Plan),
            "auto" => Some(Self::Auto),
            "dont-ask" => Some(Self::DontAsk),
            "bypass" => Some(Self::Bypass),
            _ => None,
        }
    }
}

/// What is being asked: the action, its resources, and facts the Mode needs.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// Bound by the host from the actual invocation before authorization.
    pub tool: Option<String>,
    pub action: String,
    pub resources: Vec<String>,
    /// The tool is read-only.
    pub read_only: bool,
    /// Absolute paths the action would mutate.
    pub mutates: Vec<PathBuf>,
    /// A file edit (edit, write, apply_patch) or a plain filesystem command.
    pub file_edit: bool,
    /// A critical or unresolved removal, which automatic modes cannot authorize.
    pub removal_risk: Option<super::RemovalRisk>,
    /// The tool declares itself destructive (`destructiveHint`): `auto` mode always
    /// sends it to the classifier, even when a rule or saved approval allows it.
    pub destructive: bool,
    /// Crossing an OS sandbox boundary (an unlisted domain, an escalation): `bypass`
    /// does not turn its `ask` into `allow` (`sandbox` boundaries are ceilings).
    pub sandbox_boundary: bool,
    /// At least `ask` in every Mode but `bypass`, whatever the rules and saved
    /// approvals allow (`sandbox` → Excluded commands).
    pub ask_floor: bool,
    /// `team_merge`: the teammate whose branch lands. The ownership guard lets its own
    /// `owns` paths through, and still denies every path another member owns.
    pub merged_teammate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny(String),
}

/// Rules for one evaluation, in layer order, plus the facts Modes need.
#[derive(Clone)]
pub struct Policy {
    /// Defaults, config, agent and Session rules, lowest priority first.
    pub rules: Vec<Rule>,
    /// Saved approvals for this checkout; they only turn `ask` into `allow`.
    pub saved: Vec<Rule>,
    pub mode: Mode,
    /// Ancestor Turn Modes are intersected with this Session's own decision.
    pub parent_modes: Vec<Mode>,
    pub location: PathBuf,
    pub home: PathBuf,
    /// The plan file this Session may write in `plan` mode.
    pub plan_file: PathBuf,
    /// Paths other members own in a shared-Location team (`agent-teams` → File conflict
    /// avoidance): mutating one is denied before any rule or ask.
    pub ownership: Option<cyber_server::runtime::teams::Ownership>,
}

const PLAN_DENY: &str = "Plan mode is read-only. Present the plan with plan_exit.";

impl Policy {
    pub(crate) fn skill_tool_denial(&self, tool: &str) -> Option<String> {
        crate::skill_permissions::tool_denial(&self.rules, tool)
    }

    pub fn decide(&self, req: &Request) -> Decision {
        let ruled = request_effect(&self.rules, req);
        if let Some(denied) = self.rule_denial(req, ruled) {
            return denied;
        }
        let effect = if req.sandbox_boundary {
            self.boundary_effect(req, ruled)
        } else if ruled == Effect::Ask && self.saved_allows(req) {
            Effect::Allow
        } else {
            ruled
        };
        let effect = if req.ask_floor && effect == Effect::Allow {
            Effect::Ask
        } else {
            effect
        };
        let mut decision = self.decide_in_mode(req, effect, self.mode);
        for mode in &self.parent_modes {
            decision = intersect(decision, self.decide_in_mode(req, effect, *mode));
        }
        decision
    }

    /// A sandbox boundary is a ceiling: only an explicit `allow` for its own action
    /// (`sandbox.escalate`, `network`) from a user, global, command-line or managed layer,
    /// or a person's saved approval (a domain approved `always`), crosses it without
    /// asking. Wildcard actions (`"permissions": "allow"`) and repository, agent and
    /// Session rules leave it at `ask`.
    fn boundary_effect(&self, req: &Request, ruled: Effect) -> Effect {
        if explicit_allow(&self.rules, req, ruled) || self.saved_allows(req) {
            Effect::Allow
        } else {
            Effect::Ask
        }
    }

    /// `tool-registry` → Annotations drive modes and hooks: a destructive tool is always
    /// evaluated by the classifier in `auto` mode, whatever the rules decide.
    pub(crate) fn forces_classifier(&self, req: &Request) -> bool {
        req.destructive && self.mode == Mode::Auto
    }

    /// Classification cannot replace a hard ceiling or another Session's manual approval.
    pub(crate) fn auto_review_allowed(&self, req: &Request) -> bool {
        (self.mode == Mode::Auto || self.parent_modes.contains(&Mode::Auto))
            && (self.mode == Mode::Auto
                || self.decide_in_mode(req, request_effect(&self.rules, req), self.mode)
                    != Decision::Ask)
            && req.removal_risk.is_none()
            && !self.touches_protected(req)
            && self.parent_modes.iter().all(|mode| {
                *mode == Mode::Auto
                    || self.decide_in_mode(req, request_effect(&self.rules, req), *mode)
                        != Decision::Ask
            })
    }

    /// Why a rule (or a user ceiling) denies `req`, ignoring Mode and `ask`.
    pub(crate) fn rule_denies(&self, req: &Request) -> Option<String> {
        match self.rule_denial(req, request_effect(&self.rules, req)) {
            Some(Decision::Deny(reason)) => Some(reason),
            _ => None,
        }
    }

    /// Explicit user delegation approves only spawn admission; child tools still use decide.
    pub(crate) fn user_delegation(&self, req: &Request) -> Decision {
        let ruled = request_effect(&self.rules, req);
        self.rule_denial(req, ruled).unwrap_or(Decision::Allow)
    }

    fn rule_denial(&self, req: &Request, effect: Effect) -> Option<Decision> {
        if let Some(owned) = self
            .ownership
            .as_ref()
            .and_then(|ownership| ownership.denial(&req.mutates, req.merged_teammate.as_deref()))
        {
            return Some(Decision::Deny(owned));
        }
        if effect == Effect::Deny {
            return Some(Decision::Deny(format!("denied by rule for {}", req.action)));
        }
        self.ceiling_denies(req)
            .then(|| Decision::Deny("denied by a user rule".into()))
    }

    fn decide_in_mode(&self, req: &Request, effect: Effect, mode: Mode) -> Decision {
        let decision = self
            .mode_ceiling(req, mode)
            .unwrap_or_else(|| self.apply_mode(req, effect, mode));
        if mode == Mode::DontAsk && decision == Decision::Ask {
            Decision::Deny("Not pre-approved (dont-ask mode)".into())
        } else {
            decision
        }
    }

    fn mode_ceiling(&self, req: &Request, mode: Mode) -> Option<Decision> {
        if mode == Mode::Plan && (!req.read_only || req.removal_risk.is_some()) {
            // The plan file is the one write plan mode allows outright.
            return Some(if self.only_plan_file(req) && req.removal_risk.is_none() {
                Decision::Allow
            } else {
                Decision::Deny(PLAN_DENY.into())
            });
        }
        if let Some(risk) = &req.removal_risk {
            return Some(match mode {
                Mode::Auto | Mode::DontAsk | Mode::Bypass => Decision::Deny(risk.refusal()),
                _ => Decision::Ask,
            });
        }
        if self.touches_protected(req) && !self.exact_allow(req) {
            return Some(Decision::Ask);
        }
        None
    }

    /// Explicit denies from user, global or command-line layers cannot be widened.
    fn ceiling_denies(&self, req: &Request) -> bool {
        ceiling_denies(&self.rules, req)
    }

    pub(super) fn saved_allows(&self, req: &Request) -> bool {
        req.resources.iter().all(|res| {
            self.saved
                .iter()
                .any(|r| rule_matches(r, &req.action, res, req.tool.as_deref()))
        })
    }

    fn apply_mode(&self, req: &Request, effect: Effect, mode: Mode) -> Decision {
        match (mode, effect) {
            (_, Effect::Deny) => Decision::Deny(format!("denied by rule for {}", req.action)),
            (_, Effect::Allow) => Decision::Allow,
            (Mode::Bypass, Effect::Ask) if req.sandbox_boundary => Decision::Ask,
            (Mode::Bypass, Effect::Ask) => Decision::Allow,
            (Mode::Auto, Effect::Ask) if self.default_workflow_run(req) => Decision::Allow,
            (Mode::AcceptEdits, Effect::Ask) if req.file_edit && self.inside_location(req) => {
                Decision::Allow
            }
            (Mode::DontAsk, Effect::Ask) => {
                Decision::Deny("Not pre-approved (dont-ask mode)".into())
            }
            (_, Effect::Ask) => Decision::Ask,
        }
    }

    /// `workflow.run` asks only through its default rule: `auto` starts the run without
    /// the classifier; a configured `ask` still reaches it.
    fn default_workflow_run(&self, req: &Request) -> bool {
        req.action == WORKFLOW_RUN
            && req.resources.iter().all(|resource| {
                evaluate(&self.rules, WORKFLOW_RUN, resource)
                    .1
                    .is_some_and(|rule| rule.source == "default")
            })
    }

    fn inside_location(&self, req: &Request) -> bool {
        !req.mutates.is_empty() && req.mutates.iter().all(|p| p.starts_with(&self.location))
    }

    fn only_plan_file(&self, req: &Request) -> bool {
        req.file_edit && !req.mutates.is_empty() && req.mutates.iter().all(|p| *p == self.plan_file)
    }

    pub(crate) fn touches_protected(&self, req: &Request) -> bool {
        req.mutates
            .iter()
            .any(|p| is_protected(p, &self.location, &self.home))
    }

    /// A config rule naming the exact protected path with `allow` lifts the protection.
    fn exact_allow(&self, req: &Request) -> bool {
        req.mutates.iter().all(|p| {
            let path = slash(p);
            self.rules.iter().any(|r| {
                r.effect == Effect::Allow
                    && r.source != "default"
                    && r.resource == path
                    && rule_matches(r, &req.action, &path, req.tool.as_deref())
            })
        })
    }
}

/// A deny from a user, global, command-line or managed layer, which no later layer lifts.
fn ceiling_denies(rules: &[Rule], req: &Request) -> bool {
    let ceilings: Vec<Rule> = rules
        .iter()
        .filter(|r| {
            r.effect == Effect::Deny
                && !r.source.starts_with("project:")
                && r.source != "default"
                && r.source != "session"
                && !r.source.starts_with("agent:")
        })
        .cloned()
        .collect();
    req.resources.iter().any(|res| {
        evaluate_scoped(&ceilings, &req.action, res, req.tool.as_deref()).0 == Effect::Deny
    })
}

/// Whether `ruled` (the rules' effect on `req`) is an `allow` that an explicit rule for
/// `req.action` itself from a trusted layer gives: not a wildcard action, not a
/// repository, agent or Session rule.
fn explicit_allow(rules: &[Rule], req: &Request, ruled: Effect) -> bool {
    let explicit: Vec<Rule> = rules
        .iter()
        .filter(|rule| rule.action == req.action && (rule.effect != Effect::Allow || trusted(rule)))
        .cloned()
        .collect();
    ruled == Effect::Allow && request_effect(&explicit, req) == Effect::Allow
}

/// `action` on `resource` as a boundary no Mode crosses, such as `remote.attach`
/// (`cross-session-messaging` → Peer attach): `deny` when any rule (or a user ceiling)
/// denies it, `allow` only for an explicit `allow` of that action from a user, global,
/// command-line or managed layer, else `ask`. A blanket `"permissions": "allow"`, an
/// `"*": "allow"` action and repository, agent or Session rules leave it at `ask`.
pub fn boundary_evaluate(rules: &[Rule], action: &str, resource: &str) -> Effect {
    let req = Request {
        action: action.into(),
        resources: vec![resource.into()],
        sandbox_boundary: true,
        ..Request::default()
    };
    let ruled = request_effect(rules, &req);
    if ruled == Effect::Deny || ceiling_denies(rules, &req) {
        Effect::Deny
    } else if explicit_allow(rules, &req, ruled) {
        Effect::Allow
    } else {
        Effect::Ask
    }
}

/// A rule the user controls: not a default, repository, config-profile, agent or Session
/// rule.
fn trusted(rule: &Rule) -> bool {
    !["project:", "profile:", "agent:"]
        .iter()
        .any(|prefix| rule.source.starts_with(prefix))
        && rule.source != "default"
        && rule.source != "session"
}

fn intersect(child: Decision, parent: Decision) -> Decision {
    match (child, parent) {
        (denied @ Decision::Deny(_), _) | (_, denied @ Decision::Deny(_)) => denied,
        (Decision::Ask, _) | (_, Decision::Ask) => Decision::Ask,
        _ => Decision::Allow,
    }
}

/// Configuration documents inside `.cyber/` are protected; its content directories are not.
const CYBER_CONFIG_FILES: &[&str] = &[
    "cyber.jsonc",
    "cyber.local.jsonc",
    "hooks.jsonc",
    "mcp.json",
    "plugins.json",
    "plugins.local.json",
    "plugins.lock",
];

/// `permissions-modes` → Protected paths.
pub fn is_protected(path: &Path, location: &Path, home: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let in_git_dir = path.components().any(|c| c.as_os_str() == ".git");
    let parent_is_cyber = path
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == ".cyber");
    let cyber_doc = parent_is_cyber && CYBER_CONFIG_FILES.contains(&name);
    let cyber_plugins = path
        .components()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0].as_os_str() == ".cyber" && w[1].as_os_str() == "plugins");
    let root_config = (name == "cyber.json" || name == "cyber.jsonc")
        && path.starts_with(location.ancestors().last().unwrap_or(location));
    let home_files = [
        ".bashrc",
        ".zshrc",
        ".profile",
        ".config/fish/config.fish",
        ".aws/credentials",
    ]
    .iter()
    .any(|f| path == home.join(f));
    let home_dirs = [".ssh", ".gnupg", ".config/cyber"]
        .iter()
        .any(|d| path.starts_with(home.join(d)));
    in_git_dir || cyber_doc || cyber_plugins || root_config || home_files || home_dirs
}

pub fn slash(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(rules: Vec<Rule>, mode: Mode) -> Policy {
        Policy {
            rules,
            saved: Vec::new(),
            mode,
            parent_modes: Vec::new(),
            location: "/repo".into(),
            home: "/home/u".into(),
            plan_file: "/repo/.cyber/plans/ses_1.md".into(),
            ownership: None,
        }
    }

    fn edit(path: &str) -> Request {
        Request {
            action: "edit".into(),
            resources: vec![path.trim_start_matches("/repo/").into()],
            mutates: vec![path.into()],
            file_edit: true,
            ..Request::default()
        }
    }

    /// `agent-teams` → File conflict avoidance: a path another member owns is denied
    /// before any rule or Mode, even under `allow` rules in `bypass`, for edits and for
    /// shell write targets alike; other paths keep their normal decision.
    #[test]
    fn paths_other_members_own_are_denied_before_rules_and_modes() {
        let mut policy = policy(
            parse_rules(&json!({"edit": "allow", "bash": "allow"}), &BTreeMap::new()),
            Mode::Bypass,
        );
        policy.ownership = Some(cyber_server::runtime::teams::Ownership {
            directory: "/repo".into(),
            owners: vec![("api".into(), vec!["server/**".into()])],
        });
        let owned = Decision::Deny("Path owned by teammate api".into());
        assert_eq!(policy.decide(&edit("/repo/server/api.rs")), owned);
        let shell = Request {
            action: "bash".into(),
            resources: vec!["echo x > server/api.rs".into()],
            mutates: vec!["/repo/web/a.ts".into(), "/repo/server/api.rs".into()],
            ..Request::default()
        };
        assert_eq!(policy.decide(&shell), owned);
        assert_eq!(
            policy.rule_denies(&shell).as_deref(),
            Some("Path owned by teammate api")
        );
        assert_eq!(policy.decide(&edit("/repo/web/app.ts")), Decision::Allow);
        // `team_merge` of `api`'s branch lands its own paths.
        let mut merged = edit("/repo/server/api.rs");
        merged.merged_teammate = Some("api".into());
        assert_eq!(policy.decide(&merged), Decision::Allow);
        merged.merged_teammate = Some("ui".into());
        assert_eq!(policy.decide(&merged), owned);
        policy.ownership = None;
        assert_eq!(policy.decide(&edit("/repo/server/api.rs")), Decision::Allow);
    }

    #[test]
    fn written_order_and_last_match_wins() {
        let rules = parse_rules(
            &json!({"bash": {"*": "ask", "git status": "allow"}}),
            &BTreeMap::new(),
        );
        assert_eq!(
            rules
                .iter()
                .map(|r| r.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["*", "git status"]
        );
        assert_eq!(evaluate(&rules, "bash", "git status").0, Effect::Allow);
        let rules = parse_rules(
            &json!({"edit": {"src/**": "allow", "*": "ask"}}),
            &BTreeMap::new(),
        );
        assert_eq!(
            evaluate(&rules, "edit", "src/a.rs").0,
            Effect::Ask,
            "a later broad rule overrides"
        );
        assert_eq!(
            parse_rules(&json!("ask"), &BTreeMap::new())[0],
            Rule::new("*", "*", Effect::Ask, "config")
        );
    }

    #[test]
    fn several_resources_take_the_strictest() {
        let rules = parse_rules(
            &json!({"bash": {"*": "allow", "git push *": "deny"}}),
            &BTreeMap::new(),
        );
        assert_eq!(
            evaluate_all(
                &rules,
                "bash",
                &["npm test".into(), "git push origin".into()]
            ),
            Effect::Deny
        );
    }

    #[test]
    fn env_files_ask_but_examples_are_allowed() {
        let rules = defaults(&[], true);
        assert_eq!(evaluate(&rules, "read", ".env.local").0, Effect::Ask);
        assert_eq!(evaluate(&rules, "read", "app/.env").0, Effect::Ask);
        assert_eq!(evaluate(&rules, "read", ".env.example").0, Effect::Allow);
        assert_eq!(evaluate(&rules, "read", "src/main.rs").0, Effect::Allow);
        assert_eq!(evaluate(&rules, "bash", "ls").0, Effect::Ask);
        assert_eq!(
            evaluate(&defaults(&[], false), "question", "*").0,
            Effect::Deny
        );
    }

    #[test]
    fn a_workflow_allow_rule_or_approval_covers_only_its_origin() {
        use cyber_workflow::{Origin, Scope, WorkflowModule};
        let source = "export const meta = { name: 'audit', description: 'Audit' };\n";
        let saved = |scope| {
            WorkflowModule::parse(
                source,
                Origin::Saved {
                    scope,
                    path: "/w/audit.js".into(),
                },
            )
            .unwrap()
        };
        let inline = WorkflowModule::inline(source).unwrap();
        let other = WorkflowModule::inline(format!("{source}log(1);\n")).unwrap();
        assert_eq!(workflow_resource(&saved(Scope::Global)), "audit");
        assert_eq!(workflow_resource(&saved(Scope::Project)), "project:audit");
        assert_eq!(
            workflow_resource(&inline),
            format!("inline:audit@{}", inline.digest)
        );
        let run = |module: &WorkflowModule| Request {
            action: WORKFLOW_RUN.into(),
            resources: vec![workflow_resource(module)],
            ..Default::default()
        };
        // `audit: allow` (the global saved Workflow) does not cover an inline script or a
        // project file that declares the same name.
        let mut rules = defaults(&[], true);
        rules.push(Rule::new(WORKFLOW_RUN, "audit", Effect::Allow, "global"));
        let allowed = policy(rules, Mode::Default);
        assert_eq!(allowed.decide(&run(&saved(Scope::Global))), Decision::Allow);
        assert_eq!(allowed.decide(&run(&saved(Scope::Project))), Decision::Ask);
        assert_eq!(allowed.decide(&run(&inline)), Decision::Ask);
        // An "always" approval of one inline script covers only that script.
        let mut approved = policy(defaults(&[], true), Mode::Default);
        approved.saved = vec![Rule::new(
            WORKFLOW_RUN,
            &workflow_resource(&inline),
            Effect::Allow,
            "session",
        )];
        assert_eq!(approved.decide(&run(&inline)), Decision::Allow);
        assert_eq!(approved.decide(&run(&other)), Decision::Ask);
        assert_eq!(approved.decide(&run(&saved(Scope::Global))), Decision::Ask);
        // A deny for the bare name holds for every origin through `rule_denies`.
        let mut rules = defaults(&[], true);
        rules.push(Rule::new(WORKFLOW_RUN, "audit", Effect::Deny, "global"));
        let denied = policy(rules, Mode::Bypass);
        let bare = Request {
            action: WORKFLOW_RUN.into(),
            resources: vec!["audit".into()],
            ..Default::default()
        };
        assert!(denied.rule_denies(&bare).is_some());
        assert_eq!(denied.decide(&run(&inline)), Decision::Allow);
    }

    #[test]
    fn workflow_run_asks_by_default_with_a_visible_source_and_is_allowed_in_auto_and_bypass() {
        let base = defaults(&[], true);
        let (effect, rule) = evaluate(&base, WORKFLOW_RUN, "audit");
        assert_eq!(effect, Effect::Ask);
        let rule = rule.unwrap();
        assert_eq!(
            (rule.action.as_str(), rule.source.as_str()),
            (WORKFLOW_RUN, "default")
        );
        let run = Request {
            action: WORKFLOW_RUN.into(),
            resources: vec!["audit".into()],
            ..Default::default()
        };
        let decide = |rules: Vec<Rule>, mode| policy(rules, mode).decide(&run);
        assert_eq!(decide(base.clone(), Mode::Default), Decision::Ask);
        assert_eq!(decide(base.clone(), Mode::Auto), Decision::Allow);
        assert_eq!(decide(base.clone(), Mode::Bypass), Decision::Allow);
        assert!(matches!(
            decide(base.clone(), Mode::Plan),
            Decision::Deny(_)
        ));
        assert!(matches!(
            decide(base.clone(), Mode::DontAsk),
            Decision::Deny(_)
        ));
        // A configured ask is not the default: auto still classifies it.
        let mut asked = base.clone();
        asked.push(Rule::new(WORKFLOW_RUN, "*", Effect::Ask, "user"));
        assert_eq!(decide(asked, Mode::Auto), Decision::Ask);
        let mut denied = base;
        denied.push(Rule::new(WORKFLOW_RUN, "audit", Effect::Deny, "user"));
        assert!(matches!(decide(denied, Mode::Bypass), Decision::Deny(_)));
    }

    /// `permissions-modes` → Default rules: same-server messages are allowed, an `@`
    /// address asks, listing and watching are allowed, and a deny rule on a name pattern
    /// refuses a target addressed by its ID and name.
    #[test]
    fn message_send_defaults_and_a_name_pattern_deny() {
        let base = defaults(&[], true);
        let send = |resources: &[&str]| Request {
            action: MESSAGE_SEND.into(),
            resources: resources.iter().map(|r| (*r).to_owned()).collect(),
            ..Default::default()
        };
        let decide = |rules: Vec<Rule>, req: &Request| policy(rules, Mode::Default).decide(req);
        assert_eq!(
            decide(base.clone(), &send(&["ses_01JB", "backend"])),
            Decision::Allow
        );
        let (effect, rule) = evaluate(&base, MESSAGE_SEND, "migration@desk");
        assert_eq!(
            (effect, rule.map(|r| r.source.as_str())),
            (Effect::Ask, Some("default"))
        );
        assert_eq!(
            decide(base.clone(), &send(&["migration@desk"])),
            Decision::Ask
        );
        for action in ["list_sessions", "watch_session"] {
            assert_eq!(evaluate(&base, action, "*").0, Effect::Allow, "{action}");
        }
        let mut denied = base.clone();
        denied.push(Rule::new(MESSAGE_SEND, "noisy-*", Effect::Deny, "user"));
        assert!(matches!(
            decide(denied.clone(), &send(&["ses_01JN", "noisy-bot"])),
            Decision::Deny(_)
        ));
        assert_eq!(
            decide(denied.clone(), &send(&["ses_01JB", "backend"])),
            Decision::Allow
        );
        assert!(matches!(
            policy(denied, Mode::Bypass).decide(&send(&["ses_01JN", "noisy-bot"])),
            Decision::Deny(_)
        ));
        let mut asked = base;
        asked.push(Rule::new(MESSAGE_SEND, "*", Effect::Ask, "user"));
        assert_eq!(
            decide(asked, &send(&["ses_01JB", "backend"])),
            Decision::Ask
        );
    }

    #[test]
    fn modes_shape_the_decision() {
        let base = defaults(&[], true);
        assert_eq!(
            policy(base.clone(), Mode::Default).decide(&edit("/repo/src/a.rs")),
            Decision::Ask
        );
        assert_eq!(
            policy(base.clone(), Mode::AcceptEdits).decide(&edit("/repo/src/a.rs")),
            Decision::Allow
        );
        assert_eq!(
            policy(base.clone(), Mode::Bypass).decide(&edit("/repo/src/a.rs")),
            Decision::Allow
        );
        assert!(
            matches!(policy(base.clone(), Mode::DontAsk).decide(&edit("/repo/src/a.rs")), Decision::Deny(m) if m.contains("dont-ask"))
        );
        assert_eq!(
            policy(base.clone(), Mode::Plan).decide(&edit("/repo/src/a.rs")),
            Decision::Deny(PLAN_DENY.into())
        );
        assert_eq!(
            policy(base.clone(), Mode::Plan).decide(&edit("/repo/.cyber/plans/ses_1.md")),
            Decision::Allow,
            "plan mode allows its plan file"
        );
        let denied = parse_rules(
            &serde_json::json!({"edit": {".cyber/plans/*": "deny"}}),
            &BTreeMap::new(),
        );
        let rules: Vec<Rule> = base.into_iter().chain(denied).collect();
        assert!(
            matches!(
                policy(rules, Mode::Plan).decide(&edit("/repo/.cyber/plans/ses_1.md")),
                Decision::Deny(_)
            ),
            "deny rules still win"
        );
    }

    #[test]
    fn critical_removal_cannot_be_approved_by_rules_or_saved_patterns() {
        let req = Request {
            action: "bash".into(),
            resources: vec!["rm -rf /repo".into()],
            mutates: vec!["/repo".into()],
            removal_risk: Some(super::super::RemovalRisk::Critical("/repo".into())),
            ..Request::default()
        };
        let rules = vec![Rule::new("bash", "*", Effect::Allow, "global:x")];
        for mode in [Mode::Auto, Mode::DontAsk, Mode::Bypass] {
            let mut p = policy(rules.clone(), mode);
            p.saved = vec![Rule::new("bash", "*", Effect::Allow, "saved")];
            assert!(
                matches!(p.decide(&req), Decision::Deny(reason) if reason.starts_with("Refused:"))
            );
        }
        for mode in [Mode::Default, Mode::AcceptEdits] {
            assert_eq!(policy(rules.clone(), mode).decide(&req), Decision::Ask);
        }
        assert_eq!(
            policy(rules.clone(), Mode::Plan).decide(&req),
            Decision::Deny(PLAN_DENY.into())
        );
        let denied = vec![Rule::new("bash", "*", Effect::Deny, "global:x")];
        assert!(
            matches!(
                policy(denied, Mode::Default).decide(&req),
                Decision::Deny(_)
            ),
            "manual approval cannot widen a deny"
        );
    }

    #[test]
    fn protected_paths_ask_even_in_bypass() {
        let base = defaults(&[], true);
        let p = policy(base.clone(), Mode::Bypass);
        for path in [
            "/home/u/.ssh/config",
            "/repo/.git/config",
            "/repo/.cyber/hooks.jsonc",
            "/repo/cyber.jsonc",
            "/home/u/.zshrc",
        ] {
            assert_eq!(p.decide(&edit(path)), Decision::Ask, "{path}");
        }
        assert_eq!(
            p.decide(&edit("/repo/.cyber/plans/x.md")),
            Decision::Allow,
            "content directories are not protected"
        );
        let mut rules = base;
        rules.push(Rule::new(
            "edit",
            "/home/u/.zshrc",
            Effect::Allow,
            "global:x",
        ));
        assert_eq!(
            policy(rules, Mode::Bypass).decide(&edit("/home/u/.zshrc")),
            Decision::Allow,
            "exact allow lifts protection"
        );
    }

    /// Regression (`cross-session-messaging` → Peer attach): `remote.attach` was allowed
    /// by any rule evaluation that came out `allow`, a blanket `"permissions": "allow"`
    /// and a repository rule included, so an attach grant took over a Session unasked.
    /// Only an explicit user-level `remote.attach` allow skips the prompt; a deny from any
    /// layer denies.
    #[test]
    fn remote_attach_needs_an_explicit_user_allow() {
        let with = |extra: Vec<Rule>| {
            let mut rules = defaults(&[], true);
            rules.extend(extra);
            rules
        };
        let attach = |rules: &[Rule]| boundary_evaluate(rules, "remote.attach", "peer:laptop");
        for rule in [
            Rule::new("*", "*", Effect::Allow, "global:x"),
            Rule::new(
                "remote.attach",
                "*",
                Effect::Allow,
                "project:/repo/cyber.jsonc",
            ),
            Rule::new("remote.attach", "*", Effect::Allow, "agent:build"),
            Rule::new("remote.attach", "peer:laptop", Effect::Allow, "session"),
        ] {
            let source = rule.source.clone();
            assert_eq!(attach(&with(vec![rule])), Effect::Ask, "{source}");
        }
        assert_eq!(attach(&with(vec![])), Effect::Ask);
        let user = Rule::new("remote.attach", "peer:laptop", Effect::Allow, "global:x");
        assert_eq!(attach(&with(vec![user.clone()])), Effect::Allow);
        assert_eq!(
            boundary_evaluate(&with(vec![user.clone()]), "remote.attach", "peer:tablet"),
            Effect::Ask
        );
        for deny in [
            Rule::new(
                "remote.attach",
                "*",
                Effect::Deny,
                "project:/repo/cyber.jsonc",
            ),
            Rule::new("*", "*", Effect::Deny, "global:x"),
        ] {
            let source = deny.source.clone();
            assert_eq!(
                attach(&with(vec![user.clone(), deny])),
                Effect::Deny,
                "{source}"
            );
        }
        // A user deny is a ceiling a later repository allow does not lift.
        let ceiling = Rule::new("remote.attach", "*", Effect::Deny, "global:x");
        let lifted = Rule::new(
            "remote.attach",
            "*",
            Effect::Allow,
            "project:/r/cyber.jsonc",
        );
        assert_eq!(attach(&with(vec![ceiling, lifted])), Effect::Deny);
    }

    /// Regression (`sandbox` → Escalation requests): a blanket `allow` (also an
    /// `"*": "allow"` action) or a repository `sandbox.escalate: allow` does not cross the
    /// sandbox boundary: the escalation asks in `default` and `bypass` and is denied in
    /// `dont-ask`. Only the user's explicit rule allows it; a deny still denies.
    #[test]
    fn sandbox_boundary_needs_an_explicit_user_allow() {
        let escalate = Request {
            action: "sandbox.escalate".into(),
            resources: vec!["rm -rf /tmp/x".into()],
            sandbox_boundary: true,
            ..Request::default()
        };
        let with = |rule: Rule| {
            let mut rules = defaults(&[], true);
            rules.push(rule);
            rules
        };
        for rule in [
            Rule::new("*", "*", Effect::Allow, "global:x"),
            Rule::new(
                "sandbox.escalate",
                "*",
                Effect::Allow,
                "project:/repo/cyber.jsonc",
            ),
            Rule::new("sandbox.escalate", "*", Effect::Allow, "agent:build"),
        ] {
            let source = rule.source.clone();
            let rules = with(rule);
            assert_eq!(
                policy(rules.clone(), Mode::Default).decide(&escalate),
                Decision::Ask,
                "{source}"
            );
            assert_eq!(
                policy(rules.clone(), Mode::Bypass).decide(&escalate),
                Decision::Ask,
                "{source}"
            );
            assert!(
                matches!(
                    policy(rules, Mode::DontAsk).decide(&escalate),
                    Decision::Deny(_)
                ),
                "{source}"
            );
        }
        // A domain a person approved `always` stays approved.
        let mut saved = policy(
            with(Rule::new("*", "*", Effect::Allow, "global:x")),
            Mode::Bypass,
        );
        saved.saved = vec![Rule::new("network", "example.org", Effect::Allow, "saved")];
        let domain = |host: &str| Request {
            action: "network".into(),
            resources: vec![host.into()],
            sandbox_boundary: true,
            ..Request::default()
        };
        assert_eq!(saved.decide(&domain("example.org")), Decision::Allow);
        assert_eq!(saved.decide(&domain("evil.test")), Decision::Ask);
        let explicit = with(Rule::new(
            "sandbox.escalate",
            "*",
            Effect::Allow,
            "global:x",
        ));
        assert_eq!(
            policy(explicit, Mode::DontAsk).decide(&escalate),
            Decision::Allow
        );
        let denied = with(Rule::new(
            "*",
            "*",
            Effect::Deny,
            "project:/repo/cyber.jsonc",
        ));
        assert!(matches!(
            policy(denied, Mode::Bypass).decide(&escalate),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn saved_approvals_cannot_widen_user_denies() {
        let mut rules = defaults(&[], true);
        rules.push(Rule::new(
            "bash",
            "curl *",
            Effect::Deny,
            "global:/u/cyber.jsonc",
        ));
        rules.push(Rule::new(
            "bash",
            "curl *",
            Effect::Allow,
            "project:/repo/cyber.jsonc",
        ));
        let mut p = policy(rules, Mode::Default);
        p.saved = vec![Rule::new("bash", "curl *", Effect::Allow, "saved")];
        let req = Request {
            action: "bash".into(),
            resources: vec!["curl https://x".into()],
            ..Request::default()
        };
        assert!(matches!(p.decide(&req), Decision::Deny(_)));
        let npm = Request {
            action: "bash".into(),
            resources: vec!["npm test".into()],
            ..Request::default()
        };
        p.saved
            .push(Rule::new("bash", "npm test *", Effect::Allow, "saved"));
        assert_eq!(
            p.decide(&npm),
            Decision::Allow,
            "saved approvals turn ask into allow"
        );
    }
}
