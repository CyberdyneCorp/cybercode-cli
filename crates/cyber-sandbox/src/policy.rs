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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// Only through the allowlist proxy.
    Proxy,
    Off,
    On,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    pub policy: Policy,
    pub network: NetworkMode,
    pub allowed_domains: Vec<String>,
    pub extra_writable: Vec<PathBuf>,
    pub readable_paths: Vec<PathBuf>,
    pub env_allow: Vec<String>,
}

impl SandboxConfig {
    /// Resolve from the merged config. `cli_policy` is the `--sandbox` flag. `full-access`
    /// from a project layer is ignored: only the user can opt out of enforcement.
    pub fn resolve(
        config: &Value,
        sources: &BTreeMap<String, String>,
        cli_policy: Option<&str>,
        home: &Path,
    ) -> Self {
        let sandbox = &config["sandbox"];
        let from_project = sources
            .get("/sandbox/policy")
            .is_some_and(|s| s.starts_with("project"));
        let configured = sandbox["policy"]
            .as_str()
            .filter(|p| !(from_project && *p == "full-access"));
        let policy = match cli_policy.or(configured) {
            Some("read-only") => Policy::ReadOnly,
            Some("full-access") => Policy::FullAccess,
            _ => Policy::WorkspaceWrite,
        };
        let network = match sandbox["network"].as_str() {
            Some("off") => NetworkMode::Off,
            Some("on") => NetworkMode::On,
            _ => NetworkMode::Proxy,
        };
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        };
        let paths =
            |v: &Value| -> Vec<PathBuf> { strings(v).iter().map(|p| expand(p, home)).collect() };
        let allowed_domains = match sandbox.get("allowed_domains") {
            Some(list) => strings(list),
            None => DEFAULT_DOMAINS.iter().map(|d| d.to_string()).collect(),
        };
        Self {
            policy,
            network,
            allowed_domains,
            extra_writable: paths(&sandbox["writable_roots"]),
            readable_paths: paths(&sandbox["readable_paths"]),
            env_allow: strings(&sandbox["env"]["allow"]),
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
}

fn expand(path: &str, home: &Path) -> PathBuf {
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
