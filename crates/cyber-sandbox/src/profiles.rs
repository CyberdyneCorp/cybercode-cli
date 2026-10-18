//! Named sandbox profiles (`sandbox` → Named sandbox profiles).
//!
//! `sandbox.profiles.<name>` holds `{ extends?, policy?, writable_roots?, readable_paths?,
//! deny_read?, network?, env? }`. A chain is applied from its root to the selected
//! profile: scalars replace, lists union. The built-ins `read-only`, `workspace` and
//! `full` set only the policy. A repository-controlled layer can never reach
//! `full-access`, through a selection, an `extends` link or a definition.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::policy::{MANAGED_NOTICE, network_mode, paths, strings, union, untrusted};
use crate::{EnvPolicy, NetworkMode, Policy, SandboxConfig};

/// The built-in profiles and the policies they stand for.
pub const BUILTIN_PROFILES: &[(&str, Policy)] = &[
    ("read-only", Policy::ReadOnly),
    ("workspace", Policy::WorkspaceWrite),
    ("full", Policy::FullAccess),
];

const MAX_CHAIN: usize = 16;

/// Which profile applies and whether a repository chose it.
#[derive(Debug, Clone)]
pub(crate) struct Selection {
    pub name: String,
    /// False when nothing selected a profile: the top-level keys apply alone.
    pub explicit: bool,
    pub untrusted: bool,
}

/// `--sandbox <policy|profile>`, then the agent's `sandbox`, then `sandbox.profile`.
pub(crate) fn select(
    config: &Value,
    sources: &BTreeMap<String, String>,
    cli: Option<&str>,
    agent: Option<&str>,
) -> Selection {
    let pointed = |pointer: &str| {
        let name = config.pointer(pointer)?.as_str()?.to_string();
        let untrusted = sources.get(pointer).is_some_and(|s| untrusted(s));
        Some(Selection {
            name,
            explicit: true,
            untrusted,
        })
    };
    if let Some(cli) = cli {
        return Selection {
            name: cli.to_string(),
            explicit: true,
            untrusted: false,
        };
    }
    agent
        .and_then(|agent| pointed(&format!("/agents/{}/sandbox", escape(agent))))
        .or_else(|| pointed("/sandbox/profile"))
        .unwrap_or_else(|| unselected(config))
}

/// No profile: the top-level keys apply alone.
pub(crate) fn unselected(config: &Value) -> Selection {
    Selection {
        name: builtin_name(policy_of(&config["sandbox"]["policy"])).to_string(),
        explicit: false,
        untrusted: false,
    }
}

/// Whether `selected` enforces less than `baseline`: a weaker policy, more network,
/// more writable or readable paths, more allowed domains, or a less masked environment.
pub(crate) fn loosens(selected: &SandboxConfig, baseline: &SandboxConfig) -> bool {
    rank(selected.policy) < rank(baseline.policy)
        || network_rank(selected.network) < network_rank(baseline.network)
        || added(&selected.extra_writable, &baseline.extra_writable)
        || added(&selected.readable_paths, &baseline.readable_paths)
        || added(&selected.allowed_domains, &baseline.allowed_domains)
        || added(&selected.env_allow, &baseline.env_allow)
        || env_loosens(&selected.env, &baseline.env)
}

fn added<T: PartialEq>(selected: &[T], baseline: &[T]) -> bool {
    selected.iter().any(|item| !baseline.contains(item))
}

fn env_loosens(selected: &EnvPolicy, baseline: &EnvPolicy) -> bool {
    (baseline.clean && !selected.clean)
        || (selected.login_shell && !baseline.login_shell)
        || selected.set != baseline.set
}

/// Less network ranks lower.
fn network_rank(network: NetworkMode) -> u8 {
    match network {
        NetworkMode::On => 0,
        NetworkMode::Proxy => 1,
        NetworkMode::Off => 2,
    }
}

/// Apply the selected chain on top of the top-level keys.
pub(crate) fn apply(
    out: &mut SandboxConfig,
    sandbox: &Value,
    sources: &BTreeMap<String, String>,
    selection: &Selection,
    home: &Path,
) -> Result<(), String> {
    if !selection.explicit {
        return Ok(());
    }
    let chain = chain(sandbox, &selection.name)?;
    let tainted = selection.untrusted
        || chain.iter().any(|link| {
            sources
                .get(&format!("/sandbox/profiles/{}/extends", escape(&link.name)))
                .is_some_and(|s| untrusted(s))
        });
    for link in &chain {
        let policy_untrusted = sources
            .get(&format!("/sandbox/profiles/{}/policy", escape(&link.name)))
            .is_some_and(|s| untrusted(s));
        apply_link(out, link, home, !(tainted || policy_untrusted))?;
    }
    Ok(())
}

/// `policy.sandbox.min_profile` (organization policy only): a weaker selection is raised
/// to it, and its `deny_read` always applies.
pub(crate) fn apply_minimum(
    out: &mut SandboxConfig,
    config: &Value,
    sources: &BTreeMap<String, String>,
    home: &Path,
) {
    let managed = sources
        .get("/policy/sandbox/min_profile")
        .is_some_and(|source| source.starts_with("managed:"));
    let Some(name) = config["policy"]["sandbox"]["min_profile"]
        .as_str()
        .filter(|_| managed)
    else {
        return;
    };
    let mut minimum = SandboxConfig::resolve(&Value::Null, &BTreeMap::new(), None, home);
    let resolved = chain(&config["sandbox"], name).and_then(|chain| {
        // A repository could otherwise redefine the organization's floor away.
        if let Some(link) = chain.iter().find(|link| repository_defined(sources, link)) {
            return Err(format!(
                "sandbox profile {} is set by a repository",
                link.name
            ));
        }
        chain
            .iter()
            .try_for_each(|link| apply_link(&mut minimum, link, home, false))
    });
    if resolved.is_err() {
        // An unusable organization minimum fails closed to the strictest policy.
        minimum.policy = Policy::ReadOnly;
    }
    if rank(out.policy) < rank(minimum.policy) {
        out.policy = minimum.policy;
        out.notice = Some(MANAGED_NOTICE.into());
    }
    union(&mut out.deny_read, minimum.deny_read);
}

/// Whether a project or config-profile layer writes any part of `link`'s definition.
fn repository_defined(sources: &BTreeMap<String, String>, link: &Link<'_>) -> bool {
    let prefix = format!("/sandbox/profiles/{}", escape(&link.name));
    link.definition.is_some()
        && sources.iter().any(|(pointer, source)| {
            pointer
                .strip_prefix(&prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
                && untrusted(source)
        })
}

/// One profile of a chain: a built-in policy or a definition.
pub(crate) struct Link<'a> {
    pub name: String,
    pub builtin: Option<Policy>,
    pub definition: Option<&'a Value>,
}

/// The chain of `name`, root first. Unknown names, cycles and overlong chains fail.
pub(crate) fn chain<'a>(sandbox: &'a Value, name: &str) -> Result<Vec<Link<'a>>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut next = Some(name.to_string());
    while let Some(name) = next.take() {
        if !seen.insert(name.clone()) || out.len() >= MAX_CHAIN {
            return Err(format!("sandbox profile {name} extends itself"));
        }
        if let Some(policy) = builtin(&name) {
            out.push(Link {
                name,
                builtin: Some(policy),
                definition: None,
            });
            break;
        }
        let definition = sandbox["profiles"]
            .get(&name)
            .filter(|d| d.is_object())
            .ok_or_else(|| format!("unknown sandbox profile {name}"))?;
        next = definition["extends"].as_str().map(str::to_string);
        out.push(Link {
            name,
            builtin: None,
            definition: Some(definition),
        });
    }
    out.reverse();
    Ok(out)
}

fn apply_link(
    out: &mut SandboxConfig,
    link: &Link<'_>,
    home: &Path,
    full_allowed: bool,
) -> Result<(), String> {
    let set_policy = |out: &mut SandboxConfig, policy: Policy| {
        if policy != Policy::FullAccess || full_allowed {
            out.policy = policy;
        }
    };
    if let Some(policy) = link.builtin {
        set_policy(out, policy);
        return Ok(());
    }
    let Some(def) = link.definition else {
        return Ok(());
    };
    if let Some(raw) = def["policy"].as_str() {
        let policy = parse_policy(raw)
            .ok_or_else(|| format!("sandbox profile {}: unknown policy {raw}", link.name))?;
        set_policy(out, policy);
    }
    union(&mut out.extra_writable, paths(&def["writable_roots"], home));
    union(&mut out.readable_paths, paths(&def["readable_paths"], home));
    union(&mut out.deny_read, strings(&def["deny_read"]));
    let network = &def["network"];
    if let Some(mode) = network_mode(network.as_str().or(network["mode"].as_str())) {
        out.network = mode;
    }
    union(
        &mut out.allowed_domains,
        strings(&network["allowed_domains"]),
    );
    union(&mut out.denied_domains, strings(&network["denied_domains"]));
    out.merge_env(&def["env"]);
    Ok(())
}

/// A built-in profile, also by its policy name.
fn builtin(name: &str) -> Option<Policy> {
    BUILTIN_PROFILES
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, policy)| *policy)
        .or_else(|| parse_policy(name))
}

pub(crate) fn parse_policy(name: &str) -> Option<Policy> {
    match name {
        "read-only" => Some(Policy::ReadOnly),
        "workspace-write" => Some(Policy::WorkspaceWrite),
        "full-access" => Some(Policy::FullAccess),
        _ => None,
    }
}

fn policy_of(value: &Value) -> Policy {
    value
        .as_str()
        .and_then(parse_policy)
        .unwrap_or(Policy::WorkspaceWrite)
}

fn builtin_name(policy: Policy) -> &'static str {
    BUILTIN_PROFILES
        .iter()
        .find(|(_, p)| *p == policy)
        .map_or("workspace", |(name, _)| name)
}

/// Stricter policies rank higher.
fn rank(policy: Policy) -> u8 {
    match policy {
        Policy::FullAccess => 0,
        Policy::WorkspaceWrite => 1,
        Policy::ReadOnly => 2,
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetworkMode;
    use serde_json::json;

    fn resolve(config: Value, sources: &[(&str, &str)], cli: Option<&str>) -> SandboxConfig {
        let sources = sources
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        SandboxConfig::resolve(&config, &sources, cli, Path::new("/home/u"))
    }

    #[test]
    fn extends_chain_merges_lists_and_replaces_scalars() {
        let config = json!({"sandbox": {
            "deny_read": ["*.key"],
            "profile": "ci",
            "profiles": {
                "base": {"extends": "workspace", "deny_read": ["~/.aws/**"],
                    "network": {"mode": "off", "denied_domains": ["evil.test"]},
                    "env": {"mode": "none", "set": {"A": "1"}}},
                "ci": {"extends": "base", "deny_read": ["**/*.pem"],
                    "env": {"set": {"CI": "1"}, "deny": ["NPM_*"]}}
            }
        }});
        let c = resolve(config, &[], None);
        assert_eq!(c.profile, "ci");
        assert_eq!(c.policy, Policy::WorkspaceWrite);
        assert_eq!(c.deny_read, ["*.key", "~/.aws/**", "**/*.pem"]);
        assert_eq!(c.network, NetworkMode::Off);
        assert!(c.env.clean);
        assert_eq!(
            c.env.set,
            BTreeMap::from([("A".into(), "1".into()), ("CI".into(), "1".into())])
        );
        assert_eq!(c.env.deny, ["NPM_*"]);
        assert!(c.domain_denied("evil.test") && !c.domain_allowed("evil.test"));
        assert!(c.error.is_none());
    }

    #[test]
    fn cli_selects_a_policy_or_profile_and_unknown_or_cyclic_profiles_fail_closed() {
        let config = json!({"sandbox": {"profiles": {
            "loop": {"extends": "loop2"}, "loop2": {"extends": "loop"},
            "ro": {"extends": "read-only"}
        }}});
        assert_eq!(
            resolve(config.clone(), &[], Some("ro")).policy,
            Policy::ReadOnly
        );
        assert_eq!(
            resolve(config.clone(), &[], Some("full-access")).policy,
            Policy::FullAccess
        );
        let cyclic = resolve(config.clone(), &[], Some("loop"));
        assert!(cyclic.error.unwrap().contains("extends itself"));
        let unknown = resolve(config, &[], Some("nope"));
        assert_eq!(unknown.policy, Policy::WorkspaceWrite);
        assert_eq!(unknown.error.unwrap(), "unknown sandbox profile nope");
    }

    #[test]
    fn repository_layers_cannot_reach_full_access_through_profiles() {
        let project = "project:/repo/cyber.jsonc";
        let selected = json!({"sandbox": {"profile": "full"}});
        assert_eq!(
            resolve(selected.clone(), &[("/sandbox/profile", project)], None).policy,
            Policy::WorkspaceWrite
        );
        assert_eq!(
            resolve(
                selected,
                &[("/sandbox/profile", "global:/u/cyber.jsonc")],
                None
            )
            .policy,
            Policy::FullAccess
        );
        let defined =
            json!({"sandbox": {"profile": "x", "profiles": {"x": {"policy": "full-access"}}}});
        assert_eq!(
            resolve(defined, &[("/sandbox/profiles/x/policy", project)], None).policy,
            Policy::WorkspaceWrite
        );
        let linked = json!({"sandbox": {"profile": "x", "profiles": {"x": {"extends": "full"}}}});
        assert_eq!(
            resolve(linked, &[("/sandbox/profiles/x/extends", project)], None).policy,
            Policy::WorkspaceWrite
        );
        let excluded = json!({"sandbox": {"excluded_commands": ["docker *"]}});
        assert!(
            resolve(
                excluded.clone(),
                &[("/sandbox/excluded_commands", project)],
                None
            )
            .excluded_commands
            .is_empty()
        );
        assert_eq!(resolve(excluded, &[], None).excluded_commands, ["docker *"]);
    }

    /// Regression: a repository's agent `sandbox` selection may tighten the user's sandbox
    /// but never loosen it: read-only stays read-only, the network stays behind the proxy
    /// and no writable root is added; the user's own `sandbox.profile` is the baseline.
    #[test]
    fn repository_agent_selection_cannot_loosen_the_sandbox() {
        let home = Path::new("/home/u");
        let project = "project:/repo/cyber.jsonc";
        let config = |selected: &str| {
            json!({
                "agents": {"build": {"sandbox": selected}},
                "sandbox": {"policy": "read-only", "profiles": {
                    "open": {"extends": "read-only", "network": "on"},
                    "roots": {"extends": "read-only", "writable_roots": ["~"]},
                    "tight": {"extends": "read-only", "network": "off", "deny_read": ["**/*.pem"]}
                }}
            })
        };
        let sources = BTreeMap::from([("/agents/build/sandbox".to_string(), project.to_string())]);
        let resolve = |selected: &str, sources: &BTreeMap<String, String>| {
            SandboxConfig::resolve_for(&config(selected), sources, None, Some("build"), home)
        };
        for selected in ["workspace", "open", "roots"] {
            let c = resolve(selected, &sources);
            assert_eq!(c.policy, Policy::ReadOnly, "{selected}");
            assert_eq!(c.network, NetworkMode::Proxy, "{selected}");
            assert!(c.extra_writable.is_empty(), "{selected}");
            assert!(c.notice.unwrap().contains("ignored"), "{selected}");
        }
        let tight = resolve("tight", &sources);
        assert_eq!((tight.network, tight.notice), (NetworkMode::Off, None));
        assert_eq!(tight.deny_read, ["**/*.pem"]);
        // The user's own agent selection applies as configured.
        let user = BTreeMap::from([(
            "/agents/build/sandbox".to_string(),
            "global:/u/cyber.jsonc".to_string(),
        )]);
        assert_eq!(resolve("workspace", &user).policy, Policy::WorkspaceWrite);
        // The baseline of an agent's selection is the user's `sandbox.profile`.
        let mut profiled = config("workspace");
        profiled["sandbox"]["profile"] = json!("tight");
        let c = SandboxConfig::resolve_for(&profiled, &sources, None, Some("build"), home);
        assert_eq!((c.policy, c.network), (Policy::ReadOnly, NetworkMode::Off));
        assert_eq!(c.deny_read, ["**/*.pem"]);
    }

    #[test]
    fn agent_profile_and_organization_minimum() {
        let config = json!({
            "agents": {"ci": {"sandbox": "read-only"}},
            "sandbox": {"policy": "full-access"},
            "policy": {"sandbox": {"min_profile": "workspace"}}
        });
        let home = Path::new("/home/u");
        let agent = SandboxConfig::resolve_for(&config, &BTreeMap::new(), None, Some("ci"), home);
        assert_eq!((agent.policy, agent.notice), (Policy::ReadOnly, None));
        let managed = BTreeMap::from([(
            "/policy/sandbox/min_profile".to_string(),
            "managed:/etc/cyber/cyber-policy.json".to_string(),
        )]);
        let unmanaged = SandboxConfig::resolve(&config, &BTreeMap::new(), None, home);
        assert_eq!(
            unmanaged.policy,
            Policy::FullAccess,
            "only managed layers set a minimum"
        );
        let raised = SandboxConfig::resolve(&config, &managed, None, home);
        assert_eq!(raised.policy, Policy::WorkspaceWrite);
        assert_eq!(raised.notice.as_deref(), Some(MANAGED_NOTICE));
        let broken = json!({"policy": {"sandbox": {"min_profile": "missing"}}});
        assert_eq!(
            SandboxConfig::resolve(&broken, &managed, None, home).policy,
            Policy::ReadOnly
        );
    }

    /// Regression: a repository layer cannot redefine the profile the organization names
    /// as its minimum (to `full-access` or an empty `deny_read`); the floor then fails
    /// closed to read-only. A user-defined minimum still adds its `deny_read`.
    #[test]
    fn repository_cannot_redefine_the_organization_minimum() {
        let home = Path::new("/home/u");
        let config = json!({
            "sandbox": {"policy": "full-access", "profiles": {"corp": {"policy": "full-access", "deny_read": []}}},
            "policy": {"sandbox": {"min_profile": "corp"}}
        });
        let sources = |profile_source: &str| {
            BTreeMap::from([
                (
                    "/policy/sandbox/min_profile".to_string(),
                    "managed:/etc/cyber/cyber-policy.json".to_string(),
                ),
                (
                    "/sandbox/profiles/corp/policy".to_string(),
                    profile_source.to_string(),
                ),
            ])
        };
        let tampered =
            SandboxConfig::resolve(&config, &sources("project:/repo/cyber.jsonc"), None, home);
        assert_eq!(tampered.policy, Policy::ReadOnly);
        let user = json!({
            "sandbox": {"policy": "full-access", "profiles": {"corp": {"extends": "workspace", "deny_read": ["**/*.pem"]}}},
            "policy": {"sandbox": {"min_profile": "corp"}}
        });
        let trusted = SandboxConfig::resolve(&user, &sources("global:/u/cyber.jsonc"), None, home);
        assert_eq!(trusted.policy, Policy::WorkspaceWrite);
        assert_eq!(trusted.deny_read, ["**/*.pem"]);
    }
}
