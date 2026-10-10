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
    pub tool: Option<String>,
    pub action: String,
    pub resource: String,
    pub effect: Effect,
    pub source: String,
}

impl Rule {
    pub fn new(action: &str, resource: &str, effect: Effect, source: &str) -> Self {
        Self {
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
            .flat_map(|(action, spec)| action_rules(action, spec, &source_of))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| ordered_rule(index, item, sources))
            .collect(),
        _ => Vec::new(),
    }
}

fn ordered_rule(index: usize, item: &Value, sources: &BTreeMap<String, String>) -> Option<Rule> {
    let base = format!("/permissions/{index}");
    let source = sources
        .get(&format!("{base}/effect"))
        .or_else(|| sources.get(&base))
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
    rules
        .iter()
        .rev()
        .find(|rule| rule_matches(rule, action, resource, tool))
        .map_or((Effect::Ask, None), |rule| (rule.effect, Some(rule)))
}

fn rule_matches(rule: &Rule, action: &str, resource: &str, tool: Option<&str>) -> bool {
    matches(&rule.action, action)
        && matches(&rule.resource, resource)
        && rule.tool.as_deref().is_none_or(|scope| Some(scope) == tool)
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
    ] {
        rules.push(Rule::new(action, "*", Effect::Allow, "default"));
    }
    for action in ["read", "edit"] {
        rules.push(Rule::new(action, "*.env", Effect::Ask, "default"));
        rules.push(Rule::new(action, "*.env.*", Effect::Ask, "default"));
        rules.push(Rule::new(action, "*.env.example", Effect::Allow, "default"));
    }
    rules.push(Rule::new("external_directory", "*", Effect::Ask, "default"));
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
    for action in ["question", "plan_enter", "plan_exit"] {
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
}

const PLAN_DENY: &str = "Plan mode is read-only. Present the plan with plan_exit.";

impl Policy {
    pub fn decide(&self, req: &Request) -> Decision {
        let ruled = request_effect(&self.rules, req);
        if let Some(denied) = self.rule_denial(req, ruled) {
            return denied;
        }
        let effect = if ruled == Effect::Ask && self.saved_allows(req) {
            Effect::Allow
        } else {
            ruled
        };
        let mut decision = self.decide_in_mode(req, effect, self.mode);
        for mode in &self.parent_modes {
            decision = intersect(decision, self.decide_in_mode(req, effect, *mode));
        }
        decision
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

    /// Explicit user delegation approves only spawn admission; child tools still use decide.
    pub(crate) fn user_delegation(&self, req: &Request) -> Decision {
        let ruled = request_effect(&self.rules, req);
        self.rule_denial(req, ruled).unwrap_or(Decision::Allow)
    }

    fn rule_denial(&self, req: &Request, effect: Effect) -> Option<Decision> {
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
        let ceilings: Vec<Rule> = self
            .rules
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

    fn saved_allows(&self, req: &Request) -> bool {
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
            (Mode::Bypass, Effect::Ask) => Decision::Allow,
            (Mode::AcceptEdits, Effect::Ask) if req.file_edit && self.inside_location(req) => {
                Decision::Allow
            }
            (Mode::DontAsk, Effect::Ask) => {
                Decision::Deny("Not pre-approved (dont-ask mode)".into())
            }
            (_, Effect::Ask) => Decision::Ask,
        }
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
