//! The feature registry (`configuration` → Feature flags registry): the named features
//! this binary knows, their maturity and default, and how one configuration load
//! resolves each to on or off.
//!
//! Resolution order, highest first: the organization's `policy.features.lock`, the
//! `--features` flag of this process, `CYBER_FEATURES`, the merged `features.<name>`,
//! the deprecated `experimental.<name>` (with a warning), the registry default. An
//! unknown name in any source fails the load with `unknown feature "<name>"`.
//!
//! [`resolve`] writes every value that does not come from the default back into the
//! merged document as `features.<name>`, with the winning source as its label, so a
//! consumer that only has `(value, sources)` reads the effective value through
//! [`Features::from_config`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::{Map, Value};

/// How far along a feature is. `experimental` features are off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Maturity {
    Experimental,
    Beta,
    Stable,
}

impl Maturity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Experimental => "experimental",
            Self::Beta => "beta",
            Self::Stable => "stable",
        }
    }
}

/// One registry entry.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub maturity: Maturity,
    pub default: bool,
    pub description: &'static str,
}

/// Agent teams (`agent-teams`).
pub const TEAMS: &str = "teams";
/// Shared project hub and explicitly published local agents.
pub const COLLABORATION: &str = "collaboration";
pub const COMPUTER_USE: &str = "computer_use";

/// Every feature this binary knows.
pub const REGISTRY: &[Entry] = &[
    Entry {
        name: COMPUTER_USE,
        maturity: Maturity::Experimental,
        default: false,
        description: "Native computer use: explicit owner-armed macOS screen and input control",
    },
    Entry {
        name: TEAMS,
        maturity: Maturity::Experimental,
        default: false,
        description: "Agent teams: a lead Session and named teammates sharing a task list",
    },
    Entry {
        name: COLLABORATION,
        maturity: Maturity::Experimental,
        default: false,
        description: "Project collaboration: people and explicitly published local agents through a self-hosted hub",
    },
];

/// The environment variable that toggles features: `a,b,-c`.
pub const ENV: &str = "CYBER_FEATURES";
/// Source label of a value set by the `--features` flag.
pub const FLAG_SOURCE: &str = "cli:--features";
/// Source label of a value set by [`ENV`].
pub const ENV_SOURCE: &str = "env:CYBER_FEATURES";
/// Source label of a registry default.
pub const DEFAULT_SOURCE: &str = "default";

static FLAG: OnceLock<String> = OnceLock::new();

/// `--features`: record the flag for every configuration load of this process. The
/// first call wins.
pub fn set_flag(raw: &str) {
    let _ = FLAG.set(raw.to_owned());
}

/// The `--features` value of this process, if any.
pub fn flag() -> Option<&'static str> {
    FLAG.get().map(String::as_str)
}

pub fn entry(name: &str) -> Option<&'static Entry> {
    REGISTRY.iter().find(|entry| entry.name == name)
}

pub fn unknown(name: &str) -> String {
    format!("unknown feature \"{name}\"")
}

/// One feature as `cyber features list` and `GET /api/v1/features` report it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct FeatureStatus {
    pub name: String,
    pub maturity: Maturity,
    pub default: bool,
    pub enabled: bool,
    /// The label of the source that decided `enabled`: `default`, `cli:--features`,
    /// `env:CYBER_FEATURES`, a config layer, or a `managed:` policy document.
    pub source: String,
    /// The organization policy fixes the value.
    pub locked: bool,
    pub description: String,
}

/// The effective features of one resolved configuration.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Features {
    pub features: Vec<FeatureStatus>,
}

impl Default for Features {
    /// Every feature at its registry default.
    fn default() -> Self {
        Self::from_config(&Value::Null, &BTreeMap::new())
    }
}

impl Features {
    /// Read a document [`resolve`] normalized. A missing value is the default; a value
    /// that is not a boolean (a document that was never resolved) is the default too.
    pub fn from_config(config: &Value, sources: &BTreeMap<String, String>) -> Self {
        let features = REGISTRY
            .iter()
            .map(|entry| {
                let pointer = format!("/features/{}", entry.name);
                let set = config.pointer(&pointer).and_then(Value::as_bool);
                let source = match set {
                    Some(_) => sources
                        .get(&pointer)
                        .cloned()
                        .unwrap_or_else(|| "config".into()),
                    None => DEFAULT_SOURCE.into(),
                };
                FeatureStatus {
                    name: entry.name.into(),
                    maturity: entry.maturity,
                    default: entry.default,
                    enabled: set.unwrap_or(entry.default),
                    source,
                    locked: config
                        .pointer(&format!("/policy/features/lock/{}", entry.name))
                        .is_some_and(Value::is_boolean),
                    description: entry.description.into(),
                }
            })
            .collect();
        Self { features }
    }

    pub fn enabled(&self, name: &str) -> bool {
        self.get(name).is_some_and(|status| status.enabled)
    }

    pub fn get(&self, name: &str) -> Option<&FeatureStatus> {
        self.features.iter().find(|status| status.name == name)
    }
}

/// `a,b,-c`: `-name` turns a feature off. Unknown names are issues.
pub fn parse_list(raw: &str, origin: &str) -> Result<Vec<(&'static str, bool)>, Vec<String>> {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    for item in raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (name, on) = match item.strip_prefix('-') {
            Some(name) => (name.trim(), false),
            None => (item.strip_prefix('+').unwrap_or(item).trim(), true),
        };
        match entry(name) {
            Some(entry) => out.push((entry.name, on)),
            None => issues.push(format!("{origin}: {}", unknown(name))),
        }
    }
    if issues.is_empty() {
        Ok(out)
    } else {
        Err(issues)
    }
}

/// Issues of a `features` or `experimental` object in the merged document: every key a
/// known feature, every value a boolean.
pub fn validate_config(config: &Value) -> Vec<String> {
    let mut issues = Vec::new();
    for key in ["features", "experimental"] {
        match config.get(key) {
            None => {}
            Some(Value::Object(map)) => check_members(key, map, &mut issues),
            Some(other) => issues.push(format!("{key}: expected an object, got {other}")),
        }
    }
    issues
}

fn check_members(key: &str, map: &Map<String, Value>, issues: &mut Vec<String>) {
    for (name, value) in map {
        if entry(name).is_none() {
            issues.push(unknown(name));
        } else if !value.is_boolean() {
            issues.push(format!("{key}.{name}: expected true or false, got {value}"));
        }
    }
}

/// The non-config sources of one load.
#[derive(Debug, Clone, Copy, Default)]
pub struct Overrides<'a> {
    /// `--features`.
    pub flag: Option<&'a str>,
    /// `CYBER_FEATURES`.
    pub env: Option<&'a str>,
}

/// Resolve every feature of the merged document (policy layers included) and write the
/// effective values back as `features.<name>`; see the module documentation.
pub fn resolve(
    config: &mut Value,
    sources: &mut BTreeMap<String, String>,
    overrides: Overrides<'_>,
    warnings: &mut Vec<String>,
) -> Result<(), Vec<String>> {
    let mut issues = validate_config(config);
    let flag = list(overrides.flag, "--features", &mut issues);
    let env = list(overrides.env, ENV, &mut issues);
    let lock = lock(config, &mut issues);
    if !issues.is_empty() {
        return Err(issues);
    }
    for entry in REGISTRY {
        if config
            .pointer(&format!("/experimental/{}", entry.name))
            .is_some()
        {
            warnings.push(format!(
                "experimental.{0} is deprecated; use features.{0}",
                entry.name
            ));
        }
        if let Some((value, source)) = decide(entry.name, config, sources, &lock, &flag, &env) {
            set(config, entry.name, value);
            sources.insert(format!("/features/{}", entry.name), source);
        }
    }
    Ok(())
}

type Decided = BTreeMap<&'static str, bool>;

fn list(raw: Option<&str>, origin: &str, issues: &mut Vec<String>) -> Decided {
    match raw.map(|raw| parse_list(raw, origin)) {
        None => Decided::new(),
        Some(Ok(items)) => items.into_iter().collect(),
        Some(Err(found)) => {
            issues.extend(found);
            Decided::new()
        }
    }
}

/// `policy.features.lock`: `{ "<name>": true|false }`.
fn lock(config: &Value, issues: &mut Vec<String>) -> Decided {
    let mut out = Decided::new();
    let Some(lock) = config.pointer("/policy/features/lock") else {
        return out;
    };
    let Some(map) = lock.as_object() else {
        issues.push("policy.features.lock: expected an object".into());
        return out;
    };
    for (name, value) in map {
        match (entry(name), value.as_bool()) {
            (Some(entry), Some(on)) => {
                out.insert(entry.name, on);
            }
            (None, _) => issues.push(format!("policy.features.lock: {}", unknown(name))),
            (Some(_), None) => issues.push(format!(
                "policy.features.lock.{name}: expected true or false"
            )),
        }
    }
    out
}

/// The value of one feature and its source label, or `None` for the default.
fn decide(
    name: &'static str,
    config: &Value,
    sources: &BTreeMap<String, String>,
    lock: &Decided,
    flag: &Decided,
    env: &Decided,
) -> Option<(bool, String)> {
    let label = |pointer: String, fallback: &str| {
        sources
            .get(&pointer)
            .cloned()
            .unwrap_or_else(|| fallback.into())
    };
    if let Some(on) = lock.get(name) {
        let source = label(format!("/policy/features/lock/{name}"), "policy");
        return Some((*on, source));
    }
    if let Some(on) = flag.get(name) {
        return Some((*on, FLAG_SOURCE.into()));
    }
    if let Some(on) = env.get(name) {
        return Some((*on, ENV_SOURCE.into()));
    }
    let configured = |key: &str| config.get(key)?.get(name)?.as_bool();
    if let Some(on) = configured("features") {
        return Some((on, label(format!("/features/{name}"), "config")));
    }
    let on = configured("experimental")?;
    Some((on, label(format!("/experimental/{name}"), "config")))
}

fn set(config: &mut Value, name: &str, on: bool) {
    let Some(root) = config.as_object_mut() else {
        return;
    };
    let features = root
        .entry("features")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(map) = features.as_object_mut() {
        map.insert(name.into(), Value::Bool(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(
        mut config: Value,
        labels: &[(&str, &str)],
        overrides: Overrides<'_>,
    ) -> Result<(Features, Vec<String>), Vec<String>> {
        let mut sources: BTreeMap<String, String> = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut warnings = Vec::new();
        resolve(&mut config, &mut sources, overrides, &mut warnings)?;
        Ok((Features::from_config(&config, &sources), warnings))
    }

    fn teams(result: &(Features, Vec<String>)) -> (bool, &str) {
        let status = result.0.get(TEAMS).unwrap();
        (status.enabled, status.source.as_str())
    }

    #[test]
    fn teams_is_experimental_and_off_by_default() {
        let resolved = run(json!({}), &[], Overrides::default()).unwrap();
        let status = resolved.0.get(TEAMS).unwrap();
        assert_eq!(status.maturity, Maturity::Experimental);
        assert!(!status.default && !status.enabled && !status.locked);
        assert_eq!(status.source, "default");
        assert_eq!(Features::default(), resolved.0);
    }

    #[test]
    fn collaboration_requires_opt_in_and_respects_policy_lock() {
        let resolved = run(json!({}), &[], Overrides::default()).unwrap();
        assert!(!resolved.0.enabled(COLLABORATION));
        let enabled = run(
            json!({"features":{"collaboration":true}}),
            &[],
            Overrides::default(),
        )
        .unwrap();
        assert!(enabled.0.enabled(COLLABORATION));
        let locked=run(json!({"features":{"collaboration":true},"policy":{"features":{"lock":{"collaboration":false}}}}),&[],Overrides::default()).unwrap();
        assert!(!locked.0.enabled(COLLABORATION));
    }

    #[test]
    fn config_value_carries_its_layer_label() {
        let resolved = run(
            json!({"features": {"teams": true}}),
            &[("/features/teams", "global:/u/cyber.jsonc")],
            Overrides::default(),
        )
        .unwrap();
        assert_eq!(teams(&resolved), (true, "global:/u/cyber.jsonc"));
        assert!(resolved.1.is_empty());
    }

    #[test]
    fn environment_outranks_config() {
        let env = Overrides {
            flag: None,
            env: Some("-teams"),
        };
        let resolved = run(json!({"features": {"teams": true}}), &[], env).unwrap();
        assert_eq!(teams(&resolved), (false, ENV_SOURCE));
    }

    #[test]
    fn flag_outranks_environment() {
        let both = Overrides {
            flag: Some("teams"),
            env: Some("-teams"),
        };
        let resolved = run(json!({}), &[], both).unwrap();
        assert_eq!(teams(&resolved), (true, FLAG_SOURCE));
        let off = Overrides {
            flag: Some(" -teams ,"),
            env: Some("teams"),
        };
        assert_eq!(
            teams(&run(json!({}), &[], off).unwrap()),
            (false, FLAG_SOURCE)
        );
    }

    #[test]
    fn policy_lock_outranks_every_other_source() {
        let resolved = run(
            json!({"features": {"teams": true}, "policy": {"features": {"lock": {"teams": false}}}}),
            &[("/policy/features/lock/teams", "managed:/etc/cyber/cyber-policy.json")],
            Overrides {
                flag: Some("teams"),
                env: Some("teams"),
            },
        )
        .unwrap();
        assert_eq!(
            teams(&resolved),
            (false, "managed:/etc/cyber/cyber-policy.json")
        );
        assert!(resolved.0.get(TEAMS).unwrap().locked);
    }

    #[test]
    fn experimental_alias_warns_and_yields_to_features() {
        let resolved = run(
            json!({"experimental": {"teams": true}}),
            &[("/experimental/teams", "project:/r/cyber.jsonc")],
            Overrides::default(),
        )
        .unwrap();
        assert_eq!(teams(&resolved), (true, "project:/r/cyber.jsonc"));
        assert_eq!(
            resolved.1,
            ["experimental.teams is deprecated; use features.teams"]
        );
        let both = run(
            json!({"experimental": {"teams": true}, "features": {"teams": false}}),
            &[],
            Overrides::default(),
        )
        .unwrap();
        assert_eq!(teams(&both), (false, "config"));
        assert_eq!(
            both.1.len(),
            1,
            "the alias warns even when it does not decide"
        );
    }

    #[test]
    fn unknown_names_fail_in_every_source() {
        let unknown = "unknown feature \"telepathy\"";
        for config in [
            json!({"features": {"telepathy": true}}),
            json!({"experimental": {"telepathy": true}}),
            json!({"policy": {"features": {"lock": {"telepathy": false}}}}),
        ] {
            let issues = run(config, &[], Overrides::default()).unwrap_err();
            assert!(issues.iter().any(|i| i.contains(unknown)), "{issues:?}");
        }
        for overrides in [
            Overrides {
                flag: Some("teams,-telepathy"),
                env: None,
            },
            Overrides {
                flag: None,
                env: Some("telepathy"),
            },
        ] {
            let issues = run(json!({}), &[], overrides).unwrap_err();
            assert!(issues.iter().any(|i| i.contains(unknown)), "{issues:?}");
        }
    }

    #[test]
    fn non_boolean_values_are_rejected() {
        let issues = run(
            json!({"features": {"teams": "yes"}}),
            &[],
            Overrides::default(),
        )
        .unwrap_err();
        assert_eq!(
            issues,
            ["features.teams: expected true or false, got \"yes\""]
        );
        assert!(
            run(json!({"features": true}), &[], Overrides::default())
                .unwrap_err()
                .iter()
                .any(|i| i.starts_with("features: expected an object"))
        );
    }
}
