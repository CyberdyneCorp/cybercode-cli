//! Sandbox configuration (`sandbox` → Sandbox policies, Writable roots, Network isolation).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAccess => "full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// Only through the allowlist proxy.
    Proxy,
    Off,
    On,
}

impl NetworkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Off => "off",
            Self::On => "on",
        }
    }
}

/// Package registries reachable without asking.
pub const DEFAULT_DOMAINS: &[&str] = &[
    "registry.npmjs.org",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "proxy.golang.org",
    "github.com",
];

/// Credential files hidden from sandboxed processes, relative to home.
const CREDENTIAL_PATHS: &[&str] = &[
    ".aws",
    ".config/gh",
    ".netrc",
    ".docker/config.json",
    ".ssh",
];

/// `sandbox.env` (`sandbox` → Environment policy).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvPolicy {
    /// `none`: only the base variables and `set` reach sandboxed processes.
    pub clean: bool,
    /// Variables added to every sandboxed process.
    pub set: BTreeMap<String, String>,
    /// Name patterns removed after masking; they win over `allow`.
    pub deny: Vec<String>,
    /// Run `bash -l` so profile scripts run.
    pub login_shell: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    pub policy: Policy,
    pub network: NetworkMode,
    pub allowed_domains: Vec<String>,
    /// Domains refused without asking, even when allowed by another list.
    pub denied_domains: Vec<String>,
    pub extra_writable: Vec<PathBuf>,
    pub readable_paths: Vec<PathBuf>,
    /// Unanchored `deny_read` globs; see [`crate::DenyRead`].
    pub deny_read: Vec<String>,
    /// `sandbox.env.allow`: exact names or name patterns kept despite masking.
    pub env_allow: Vec<String>,
    pub env: EnvPolicy,
    /// `sandbox.excluded_commands` from user-controlled layers only.
    pub excluded_commands: Vec<String>,
    /// The selected profile (a built-in name when none is configured).
    pub profile: String,
    /// Shown when organization policy raised the selection.
    pub notice: Option<String>,
    /// A selection that could not be resolved. Sandboxed launches fail with it.
    pub error: Option<String>,
}

/// The managed-policy notice (`sandbox` → Org enforcement).
pub const MANAGED_NOTICE: &str = "Sandbox policy is managed by your organization";

impl SandboxConfig {
    /// Resolve from the merged config. `cli_policy` is the `--sandbox` flag. `full-access`
    /// from a project layer is ignored: only the user can opt out of enforcement.
    pub fn resolve(
        config: &Value,
        sources: &BTreeMap<String, String>,
        cli_policy: Option<&str>,
        home: &Path,
    ) -> Self {
        Self::resolve_for(config, sources, cli_policy, None, home)
    }

    /// As [`Self::resolve`], also honouring the agent's `sandbox` profile.
    pub fn resolve_for(
        config: &Value,
        sources: &BTreeMap<String, String>,
        cli_policy: Option<&str>,
        agent: Option<&str>,
        home: &Path,
    ) -> Self {
        let selection = super::profiles::select(config, sources, cli_policy, agent);
        let out = Self::selected(config, sources, &selection, home);
        if !selection.untrusted {
            return out;
        }
        // A repository's selection may tighten the sandbox, never loosen it: the baseline
        // is what applies without it (the user's `sandbox.profile` for an agent's choice).
        let mut baseline = match agent {
            Some(_) => Self::resolve_for(config, sources, cli_policy, None, home),
            None => Self::selected(config, sources, &super::profiles::unselected(config), home),
        };
        if !super::profiles::loosens(&out, &baseline) {
            return out;
        }
        baseline.notice.get_or_insert_with(|| {
            format!(
                "Sandbox profile {} chosen by the repository was ignored: it would loosen the sandbox",
                selection.name
            )
        });
        baseline
    }

    /// The top-level keys, the selected profile chain, then the organization minimum.
    fn selected(
        config: &Value,
        sources: &BTreeMap<String, String>,
        selection: &super::profiles::Selection,
        home: &Path,
    ) -> Self {
        let sandbox = &config["sandbox"];
        let mut out = Self::base(sandbox, sources, home);
        out.profile = selection.name.clone();
        if let Err(error) = super::profiles::apply(&mut out, sandbox, sources, selection, home) {
            out.policy = Policy::WorkspaceWrite;
            out.error = Some(error);
        }
        super::profiles::apply_minimum(&mut out, config, sources, home);
        out
    }

    fn base(sandbox: &Value, sources: &BTreeMap<String, String>, home: &Path) -> Self {
        let untrusted = |pointer: &str| sources.get(pointer).is_some_and(|s| untrusted(s));
        let configured = sandbox["policy"]
            .as_str()
            .filter(|p| !(untrusted("/sandbox/policy") && *p == "full-access"));
        let policy = match configured {
            Some("read-only") => Policy::ReadOnly,
            Some("full-access") => Policy::FullAccess,
            _ => Policy::WorkspaceWrite,
        };
        let network = &sandbox["network"];
        let allowed_domains = match sandbox.get("allowed_domains") {
            Some(list) => strings(list),
            None => DEFAULT_DOMAINS.iter().map(|d| d.to_string()).collect(),
        };
        let excluded_commands = sandbox["excluded_commands"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(i, _)| {
                !untrusted("/sandbox/excluded_commands")
                    && !untrusted(&format!("/sandbox/excluded_commands/{i}"))
            })
            .filter_map(|(_, v)| v.as_str().map(str::to_string))
            .collect();
        let mut out = Self {
            policy,
            network: network_mode(network.as_str().or(network["mode"].as_str()))
                .unwrap_or(NetworkMode::Proxy),
            allowed_domains,
            denied_domains: strings(&network["denied_domains"]),
            extra_writable: paths(&sandbox["writable_roots"], home),
            readable_paths: paths(&sandbox["readable_paths"], home),
            deny_read: strings(&sandbox["deny_read"]),
            env_allow: Vec::new(),
            env: EnvPolicy::default(),
            excluded_commands,
            profile: String::new(),
            notice: None,
            error: None,
        };
        union(
            &mut out.allowed_domains,
            strings(&network["allowed_domains"]),
        );
        out.merge_env(&sandbox["env"]);
        out
    }

    /// Merge one `env` object: scalars replace, lists union, `set` maps merge.
    pub(crate) fn merge_env(&mut self, env: &Value) {
        union(&mut self.env_allow, strings(&env["allow"]));
        union(&mut self.env.deny, strings(&env["deny"]));
        match env["mode"].as_str() {
            Some("none") => self.env.clean = true,
            Some("inherit") => self.env.clean = false,
            _ => {}
        }
        if let Some(login) = env["login_shell"].as_bool() {
            self.env.login_shell = login;
        }
        for (name, value) in env["set"].as_object().into_iter().flatten() {
            if let Some(value) = value.as_str() {
                self.env.set.insert(name.clone(), value.to_string());
            }
        }
    }

    /// Credential files that stay unreadable.
    pub fn unreadable(&self, home: &Path) -> Vec<PathBuf> {
        CREDENTIAL_PATHS
            .iter()
            .map(|p| home.join(p))
            .filter(|p| {
                !self
                    .readable_paths
                    .iter()
                    .any(|r| p.starts_with(r) || r.starts_with(p))
            })
            .collect()
    }

    /// The compiled `deny_read` globs, anchored at the canonical Location and home.
    pub fn deny_read_for(&self, location: &Path, home: &Path) -> crate::DenyRead {
        let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        crate::DenyRead::new(&self.deny_read, &canonical(location), &canonical(home))
    }

    /// Whether `host` may be reached without asking.
    pub fn domain_allowed(&self, host: &str) -> bool {
        !matches_domain(host, &self.denied_domains) && matches_domain(host, &self.allowed_domains)
    }

    /// Whether `host` is refused without asking.
    pub fn domain_denied(&self, host: &str) -> bool {
        matches_domain(host, &self.denied_domains)
    }
}

/// Labels of layers a repository can write: they cannot weaken enforcement.
pub(crate) fn untrusted(source: &str) -> bool {
    source.starts_with("project") || source.starts_with("profile:")
}

pub(crate) fn network_mode(mode: Option<&str>) -> Option<NetworkMode> {
    match mode? {
        "off" => Some(NetworkMode::Off),
        "on" => Some(NetworkMode::On),
        "proxy" => Some(NetworkMode::Proxy),
        _ => None,
    }
}

pub(crate) fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

pub(crate) fn paths(v: &Value, home: &Path) -> Vec<PathBuf> {
    strings(v).iter().map(|p| expand(p, home)).collect()
}

pub(crate) fn union<T: PartialEq>(into: &mut Vec<T>, more: Vec<T>) {
    for item in more {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

pub(crate) fn expand(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// Whether `host` is allowed by a list of domains (`*.example.com` matches subdomains).
pub fn matches_domain(host: &str, allowed: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed.iter().any(|d| {
        let d = d.to_ascii_lowercase();
        match d.strip_prefix("*.") {
            Some(base) => host.ends_with(&format!(".{base}")),
            None => host == d,
        }
    })
}

/// Paths inside a writable root that stay read-only: repository metadata and the protected
/// configuration documents (`permissions-modes` → Protected paths).
pub fn protected_in(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        root.join(".git"),
        root.join("cyber.json"),
        root.join("cyber.jsonc"),
        root.join(".cyber/plugins"),
    ];
    for name in [
        "cyber.jsonc",
        "cyber.local.jsonc",
        "hooks.jsonc",
        "mcp.json",
        "plugins.json",
        "plugins.local.json",
        "plugins.lock",
    ] {
        out.push(root.join(".cyber").join(name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_config_cannot_select_full_access() {
        let home = Path::new("/home/u");
        let config = json!({"sandbox": {"policy": "full-access"}});
        let project = BTreeMap::from([(
            "/sandbox/policy".to_string(),
            "project:/repo/cyber.jsonc".to_string(),
        )]);
        assert_eq!(
            SandboxConfig::resolve(&config, &project, None, home).policy,
            Policy::WorkspaceWrite
        );
        let user = BTreeMap::from([(
            "/sandbox/policy".to_string(),
            "global:/home/u/.config/cyber/cyber.jsonc".to_string(),
        )]);
        assert_eq!(
            SandboxConfig::resolve(&config, &user, None, home).policy,
            Policy::FullAccess
        );
        assert_eq!(
            SandboxConfig::resolve(&json!({}), &project, Some("full-access"), home).policy,
            Policy::FullAccess
        );
    }

    #[test]
    fn defaults_and_credential_paths() {
        let home = Path::new("/home/u");
        let c = SandboxConfig::resolve(
            &json!({"sandbox": {"readable_paths": ["~/.config/gh"]}}),
            &BTreeMap::new(),
            None,
            home,
        );
        assert_eq!(
            (c.policy, c.network),
            (Policy::WorkspaceWrite, NetworkMode::Proxy)
        );
        assert!(c.allowed_domains.contains(&"crates.io".to_string()));
        let hidden = c.unreadable(home);
        assert!(hidden.contains(&home.join(".ssh")) && !hidden.contains(&home.join(".config/gh")));
    }

    #[test]
    fn domain_wildcards_match_subdomains_only() {
        let allowed = vec!["github.com".to_string(), "*.example.org".to_string()];
        assert!(matches_domain("GitHub.com", &allowed));
        assert!(!matches_domain("api.github.com", &allowed));
        assert!(matches_domain("a.example.org", &allowed));
        assert!(!matches_domain("example.org", &allowed));
    }
}
