//! Read-only OS-managed policy (`org-policy` → Policy sources, the OS directory only).
//!
//! The policy document is never part of the user/project layer chain: `load` strips
//! `policy` from every other layer, and this loader is the only place `managed:` hook
//! origins and `policy.hooks.managed_only` come from. A present but unusable document
//! fails closed: no managed hooks are added and every other hook is refused.
//!
//! `policy.mcp` (`mcp` → Organization MCP controls) allowlists or denylists MCP servers
//! by name, command or URL pattern, and a top-level `mcp` map provides managed servers to
//! every Location. A malformed document blocks every non-managed server as well.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use super::jsonc;
use crate::env::EnvSource;

#[cfg(windows)]
#[allow(unsafe_code)] // Bounded read-only owner/DACL and known-folder queries are isolated here.
mod windows;

/// Test and packaging override for the OS-managed directory, like `CYBER_HOME`. It is
/// ignored while the OS-managed directory itself holds a policy document, so it can
/// never replace or relax a real managed policy.
pub const MANAGED_POLICY_DIR_ENV: &str = "CYBER_MANAGED_POLICY_DIR";

const FILES: [&str; 2] = ["cyber-policy.json", "cyber-policy.jsonc"];

/// Every top-level key a managed document may hold. Anything else, including a typo of
/// `policy`, makes the document malformed so it fails closed instead of being ignored.
const DOCUMENT_KEYS: [&str; 4] = ["$schema", "hooks", "mcp", "policy"];

/// The status error of a server the organization policy refuses.
pub const MCP_BLOCKED: &str = "blocked by organization policy";

/// The hook-related state of the managed policy for one configuration load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ManagedPolicy {
    /// `managed:<file>` labels of the documents read, lowest priority first.
    pub sources: Vec<String>,
    /// `policy.hooks.managed_only`: only managed-scope hooks run.
    pub managed_only: bool,
    /// Why a present policy could not be used. Every non-managed hook is refused.
    pub error: Option<String>,
    /// `policy.mcp`: which configured (non-managed) MCP servers may start.
    #[serde(skip)]
    #[schemars(skip)]
    pub mcp: McpPolicy,
    /// `policy.plugins.disabled`: plugin names no scope may enable.
    pub plugins_disabled: Vec<String>,
}

/// `policy.mcp`: `allow` (when present, only matching servers may start) and `deny`
/// lists of server patterns. Managed servers are the organization's own and are exempt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpPolicy {
    pub allow: Option<Vec<McpPattern>>,
    pub deny: Vec<McpPattern>,
}

/// One server pattern: exactly one of a server `name`, a local `command` line (the
/// command and its arguments joined by spaces) or a remote `url`, each a wildcard
/// pattern (`*` matches any sequence). A `url` pattern is matched against the parsed,
/// normalized server URL ([`PolicyUrl`]), its origin and its path separately.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpPattern {
    pub name: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
}

impl McpPattern {
    fn matches(&self, name: &str, server: &super::McpServer) -> bool {
        let field = |pattern: &Option<String>, value: Option<String>| {
            pattern
                .as_deref()
                .zip(value)
                .is_some_and(|(pattern, value)| crate::wildcard::matches(pattern, &value))
        };
        field(&self.name, Some(name.to_owned()))
            || field(&self.command, server.command_line())
            || self
                .url
                .as_deref()
                .zip(server.url())
                .is_some_and(|(pattern, url)| {
                    PolicyUrl::parse(url).is_some_and(|url| url.matches(pattern))
                })
    }
}

/// A remote server URL as policy sees it: lowercase scheme and host without a trailing
/// dot, no default port, no credentials or fragment, and at least a `/` path, so
/// spelling variants of one endpoint match the same patterns.
struct PolicyUrl {
    /// `scheme://host`.
    host: String,
    /// `:port` for a non-default port, else empty.
    port: String,
    /// The path and any `?query`.
    rest: String,
}

impl PolicyUrl {
    fn parse(raw: &str) -> Option<Self> {
        let url = url::Url::parse(raw.trim()).ok()?;
        let host = format!(
            "{}://{}",
            url.scheme(),
            url.host_str()?.trim_end_matches('.').to_ascii_lowercase()
        );
        let port = url
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default();
        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        let rest = match url.query() {
            Some(query) => format!("{path}?{query}"),
            None => path.to_owned(),
        };
        Some(Self { host, port, rest })
    }

    /// `pattern` splits at the first `/` after `://` into an origin pattern (matched
    /// case-insensitively, against any port when it names none, and against the default
    /// port only when it names that) and a path pattern (`/` when absent, `*` after an
    /// authority ending in `*`). A pattern
    /// without `://` matches the whole normalized URL.
    fn matches(&self, pattern: &str) -> bool {
        let Some(scheme_end) = pattern.find("://").map(|index| index + 3) else {
            let whole = format!("{}{}{}", self.host, self.port, self.rest);
            return crate::wildcard::matches(pattern, &whole);
        };
        let (origin, path) = match pattern[scheme_end..].find('/') {
            Some(index) => pattern.split_at(scheme_end + index),
            // A trailing `*` in the authority still reaches into the path.
            None if pattern.ends_with('*') => (pattern, "*"),
            None => (pattern, "/"),
        };
        let origin = origin_pattern(&origin.to_ascii_lowercase(), scheme_end);
        let value = match origin.names_port {
            true => format!("{}{}", self.host, self.port),
            false => self.host.clone(),
        };
        crate::wildcard::matches(&origin.pattern, &value)
            && crate::wildcard::matches(path, &self.rest)
    }
}

struct OriginPattern {
    pattern: String,
    names_port: bool,
}

/// A lowercase `scheme://host[:port]` pattern without its scheme's default port, and
/// whether it names a port (an explicit default port names the default one).
fn origin_pattern(origin: &str, scheme_end: usize) -> OriginPattern {
    let default_port = match &origin[..scheme_end - 3] {
        "https" | "wss" => ":443",
        "http" | "ws" => ":80",
        _ => "",
    };
    if let Some(pattern) = origin
        .strip_suffix(default_port)
        .filter(|_| !default_port.is_empty())
    {
        return OriginPattern {
            pattern: pattern.to_owned(),
            names_port: true,
        };
    }
    let names_port = origin[scheme_end..]
        .rsplit(']')
        .next()
        .is_some_and(|host| host.contains(':'));
    OriginPattern {
        pattern: origin.to_owned(),
        names_port,
    }
}

impl McpPolicy {
    /// Whether a configured, non-managed server is refused. A remote server whose URL does not parse is refused while any URL pattern is set.
    pub fn blocks(&self, name: &str, server: &super::McpServer) -> bool {
        let url_policy = self
            .deny
            .iter()
            .chain(self.allow.iter().flatten())
            .any(|pattern| pattern.url.is_some());
        if url_policy
            && server
                .url()
                .is_some_and(|url| PolicyUrl::parse(url).is_none())
        {
            return true;
        }
        self.deny
            .iter()
            .any(|pattern| pattern.matches(name, server))
            || self
                .allow
                .as_ref()
                .is_some_and(|allow| !allow.iter().any(|pattern| pattern.matches(name, server)))
    }
}

impl ManagedPolicy {
    /// Whether non-managed hooks are refused, by policy or because it is malformed.
    pub fn restricts_hooks(&self) -> bool {
        self.managed_only || self.error.is_some()
    }

    /// `mcp` → Organization MCP controls: a non-managed server the policy refuses, or any
    /// non-managed server while the policy is malformed (fail closed), is blocked.
    pub fn blocks_mcp(&self, name: &str, server: &super::McpServer) -> bool {
        self.error.is_some() || self.mcp.blocks(name, server)
    }

    /// Why organization policy refuses plugin `name` (`plugins-marketplace` → Install
    /// scopes): a managed `disabled` entry overrides every scope, and a malformed policy
    /// refuses every plugin.
    pub fn blocks_plugin(&self, name: &str) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(format!("managed policy is malformed ({error})"));
        }
        self.plugins_disabled
            .iter()
            .any(|disabled| disabled == name)
            .then(|| MCP_BLOCKED.to_owned())
    }

    /// The diagnostic shown when non-managed hooks are refused.
    pub fn skip_reason(&self) -> String {
        match &self.error {
            Some(error) => format!("managed policy is malformed ({error})"),
            None => "managed-only policy".into(),
        }
    }
}

/// The managed policy and the `hooks` and `mcp` documents it contributes, as
/// `(label, value)`.
pub(super) struct ManagedRead {
    pub policy: ManagedPolicy,
    pub hooks: Vec<(String, Value)>,
    pub mcp: Vec<(String, Value)>,
    /// `policy.modes` and `policy.sandbox`, merged into the resolved config under
    /// `policy` where no other layer can write.
    pub settings: Vec<(String, Value)>,
}

/// The `policy.modes.disable` a malformed document implies: `bypass` stays off.
pub const MALFORMED_DISABLED_MODES: &[&str] = &["bypass"];

const MODES: &[&str] = &[
    "default",
    "accept-edits",
    "plan",
    "auto",
    "dont-ask",
    "bypass",
];

/// The OS-managed directory: `/etc/cyber` on Linux and other Unix systems,
/// `/Library/Application Support/cyber` on macOS, `%ProgramData%\cyber` on Windows, where
/// `%ProgramData%` comes from the known-folder store rather than the environment.
#[cfg(windows)]
pub fn os_managed_dir() -> PathBuf {
    windows::program_data().join("cyber")
}

#[cfg(not(windows))]
pub fn os_managed_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return PathBuf::from("/Library/Application Support/cyber");
    }
    PathBuf::from("/etc/cyber")
}

pub(super) fn read(env: &dyn EnvSource, warnings: &mut Vec<String>) -> ManagedRead {
    let os = os_managed_dir();
    let (dir, owner_checked) = match env.get(MANAGED_POLICY_DIR_ENV) {
        Some(_) if has_document(&os) => {
            warnings.push(format!(
                "{MANAGED_POLICY_DIR_ENV} ignored: {} holds the managed policy",
                os.display()
            ));
            (os, true)
        }
        Some(dir) => (PathBuf::from(dir), false),
        None => (os, true),
    };
    let mut read = ManagedRead {
        policy: ManagedPolicy::default(),
        hooks: Vec::new(),
        mcp: Vec::new(),
        settings: Vec::new(),
    };
    for path in FILES.iter().map(|name| dir.join(name)) {
        match document(&path, owner_checked, warnings) {
            Ok(None) => {}
            Ok(Some(value)) => {
                let label = format!("managed:{}", path.display());
                if let Err(error) = absorb(&mut read, &label, value) {
                    return malformed(read.policy.sources, format!("{}: {error}", path.display()));
                }
            }
            Err(error) => return malformed(read.policy.sources, error),
        }
    }
    read
}

fn malformed(sources: Vec<String>, error: String) -> ManagedRead {
    ManagedRead {
        policy: ManagedPolicy {
            sources,
            managed_only: false,
            error: Some(error),
            mcp: McpPolicy::default(),
            plugins_disabled: Vec::new(),
        },
        hooks: Vec::new(),
        mcp: Vec::new(),
        settings: vec![(
            "managed:malformed".into(),
            serde_json::json!({
                "modes": {"disable": MALFORMED_DISABLED_MODES},
                "messaging": {"enabled": false},
                "features": malformed_feature_lock(),
            }),
        )],
    }
}

fn has_document(dir: &Path) -> bool {
    FILES.iter().any(|name| dir.join(name).exists())
}

/// One policy document; `Ok(None)` when it is absent or ignored for its owner.
fn document(
    path: &Path,
    owner_checked: bool,
    warnings: &mut Vec<String>,
) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if owner_checked && let Some(reason) = not_admin_controlled(path) {
        warnings.push(format!("{reason}; ignored: {}", path.display()));
        return Ok(None);
    }
    let label = path.display().to_string();
    jsonc::parse(&label, &text)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Only an administrator-controlled file in the OS directory is managed
/// (`org-policy` → Policy sources). On Unix it must be owned by root and not writable by
/// group or others.
#[cfg(unix)]
fn not_admin_controlled(path: &Path) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.uid() != 0 => Some("managed policy file not root-owned"),
        Ok(metadata) if metadata.mode() & 0o022 != 0 => {
            Some("managed policy file writable by non-root users")
        }
        Ok(_) => None,
        Err(_) => Some("managed policy file not root-owned"),
    }
}

/// On Windows the file and its directory must be owned by SYSTEM, Administrators or
/// TrustedInstaller, and no other principal may change, remove or take them over.
#[cfg(windows)]
fn not_admin_controlled(path: &Path) -> Option<&'static str> {
    (!windows::admin_controlled(path)).then_some("managed policy file not admin-owned")
}

#[cfg(not(any(unix, windows)))]
fn not_admin_controlled(_path: &Path) -> Option<&'static str> {
    Some("managed policy file ownership cannot be verified")
}

/// Validate one document and add its hooks and `policy.hooks.managed_only`.
fn absorb(read: &mut ManagedRead, label: &str, value: Value) -> Result<(), String> {
    let Value::Object(document) = value else {
        return Err("the document must be a JSON object".into());
    };
    if let Some(key) = document
        .keys()
        .find(|key| !DOCUMENT_KEYS.contains(&key.as_str()))
    {
        return Err(format!("{key}: unsupported managed policy key"));
    }
    if let Some(policy) = document.get("policy") {
        absorb_policy(&mut read.policy, policy)?;
        let settings = policy_settings(policy)?;
        if !settings.is_empty() {
            read.settings
                .push((label.to_owned(), Value::Object(settings)));
        }
    }
    if let Some(hooks) = document.get("hooks") {
        read.hooks.push((label.to_owned(), managed_hooks(hooks)?));
    }
    if let Some(servers) = document.get("mcp") {
        read.mcp.push((label.to_owned(), managed_servers(servers)?));
    }
    read.policy.sources.push(label.to_owned());
    Ok(())
}

fn absorb_policy(read: &mut ManagedPolicy, policy: &Value) -> Result<(), String> {
    let policy = policy.as_object().ok_or("policy: expected an object")?;
    if let Some(key) = policy.keys().find(|key| {
        !matches!(
            key.as_str(),
            "features" | "hooks" | "messaging" | "mcp" | "modes" | "plugins" | "sandbox"
        )
    }) {
        return Err(format!("policy.{key}: unsupported managed policy key"));
    }
    read.managed_only |= managed_only(policy)?;
    if let Some(mcp) = policy.get("mcp") {
        mcp_policy(&mut read.mcp, mcp)?;
    }
    if let Some(plugins) = policy.get("plugins") {
        read.plugins_disabled.extend(disabled_plugins(plugins)?);
    }
    Ok(())
}

/// `policy.plugins.disabled`: a list of plugin names.
fn disabled_plugins(plugins: &Value) -> Result<Vec<String>, String> {
    let plugins = plugins
        .as_object()
        .ok_or("policy.plugins: expected an object")?;
    if let Some(key) = plugins.keys().find(|key| *key != "disabled") {
        return Err(format!(
            "policy.plugins.{key}: unsupported managed policy key"
        ));
    }
    let Some(disabled) = plugins.get("disabled") else {
        return Ok(Vec::new());
    };
    disabled
        .as_array()
        .ok_or("policy.plugins.disabled: expected an array")?
        .iter()
        .map(|name| {
            name.as_str()
                .filter(|name| crate::plugins::valid_name(name))
                .map(str::to_owned)
                .ok_or_else(|| format!("policy.plugins.disabled: invalid plugin name {name}"))
        })
        .collect()
}

/// `policy.modes.disable` (a list of Modes), `policy.sandbox.min_profile` (a profile
/// name) and `policy.messaging.enabled`, validated strictly so a typo fails closed.
fn policy_settings(policy: &Value) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    if let Some(messaging) = policy.get("messaging") {
        out.insert("messaging".into(), messaging_policy(messaging)?);
    }
    if let Some(features) = policy.get("features") {
        out.insert("features".into(), features_policy(features)?);
    }
    if let Some(modes) = policy.get("modes") {
        let modes = modes
            .as_object()
            .ok_or("policy.modes: expected an object")?;
        if let Some(key) = modes.keys().find(|key| *key != "disable") {
            return Err(format!(
                "policy.modes.{key}: unsupported managed policy key"
            ));
        }
        let disable = modes.get("disable").map_or(Ok(Vec::new()), |list| {
            list.as_array()
                .ok_or("policy.modes.disable: expected an array")?
                .iter()
                .map(|mode| {
                    mode.as_str()
                        .filter(|mode| MODES.contains(mode))
                        .map(|mode| Value::String(mode.into()))
                        .ok_or_else(|| format!("policy.modes.disable: unknown mode {mode}"))
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        out.insert("modes".into(), serde_json::json!({"disable": disable}));
    }
    if let Some(sandbox) = policy.get("sandbox") {
        let sandbox = sandbox
            .as_object()
            .ok_or("policy.sandbox: expected an object")?;
        if let Some(key) = sandbox.keys().find(|key| *key != "min_profile") {
            return Err(format!(
                "policy.sandbox.{key}: unsupported managed policy key"
            ));
        }
        if let Some(profile) = sandbox.get("min_profile") {
            let profile = profile
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or("policy.sandbox.min_profile: expected a profile name")?;
            out.insert(
                "sandbox".into(),
                serde_json::json!({"min_profile": profile}),
            );
        }
    }
    Ok(out)
}

/// `policy.messaging`: only `enabled`, a boolean (`cross-session-messaging` → Disable
/// switch).
fn messaging_policy(messaging: &Value) -> Result<Value, String> {
    let messaging = messaging
        .as_object()
        .ok_or("policy.messaging: expected an object")?;
    if let Some(key) = messaging.keys().find(|key| *key != "enabled") {
        return Err(format!(
            "policy.messaging.{key}: unsupported managed policy key"
        ));
    }
    match messaging.get("enabled") {
        None => Ok(serde_json::json!({})),
        Some(Value::Bool(enabled)) => Ok(serde_json::json!({ "enabled": enabled })),
        Some(_) => Err("policy.messaging.enabled: expected a boolean".into()),
    }
}

/// `policy.features.lock`: `{ "<feature>": true|false }` over known features only, so a
/// typo fails closed instead of leaving a feature unlocked.
fn features_policy(features: &Value) -> Result<Value, String> {
    let features = features
        .as_object()
        .ok_or("policy.features: expected an object")?;
    if let Some(key) = features.keys().find(|key| *key != "lock") {
        return Err(format!(
            "policy.features.{key}: unsupported managed policy key"
        ));
    }
    let Some(lock) = features.get("lock") else {
        return Ok(serde_json::json!({}));
    };
    let lock = lock
        .as_object()
        .ok_or("policy.features.lock: expected an object")?;
    for (name, value) in lock {
        if crate::features::entry(name).is_none() {
            return Err(format!(
                "policy.features.lock: {}",
                crate::features::unknown(name)
            ));
        }
        if !value.is_boolean() {
            return Err(format!(
                "policy.features.lock.{name}: expected true or false"
            ));
        }
    }
    Ok(serde_json::json!({ "lock": lock }))
}

/// Every feature locked to its default: what a malformed policy implies, so an unusable
/// document never leaves an experimental feature switchable.
fn malformed_feature_lock() -> Value {
    let lock: Map<String, Value> = crate::features::REGISTRY
        .iter()
        .map(|entry| (entry.name.to_owned(), Value::Bool(entry.default)))
        .collect();
    serde_json::json!({ "lock": lock })
}

/// `policy.mcp`: `allow` and `deny` arrays of patterns; several documents add up.
fn mcp_policy(read: &mut McpPolicy, mcp: &Value) -> Result<(), String> {
    let mcp = mcp.as_object().ok_or("policy.mcp: expected an object")?;
    if let Some(key) = mcp
        .keys()
        .find(|key| !matches!(key.as_str(), "allow" | "deny"))
    {
        return Err(format!("policy.mcp.{key}: unsupported managed policy key"));
    }
    if let Some(allow) = mcp.get("allow") {
        let patterns = patterns(allow, "policy.mcp.allow")?;
        read.allow.get_or_insert_with(Vec::new).extend(patterns);
    }
    if let Some(deny) = mcp.get("deny") {
        read.deny.extend(patterns(deny, "policy.mcp.deny")?);
    }
    Ok(())
}

fn patterns(value: &Value, path: &str) -> Result<Vec<McpPattern>, String> {
    let patterns: Vec<McpPattern> =
        serde_json::from_value(value.clone()).map_err(|error| format!("{path}: {error}"))?;
    for (index, pattern) in patterns.iter().enumerate() {
        let set = [&pattern.name, &pattern.command, &pattern.url]
            .into_iter()
            .filter(|field| {
                field
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
            })
            .count();
        if set != 1 {
            return Err(format!(
                "{path}[{index}]: expected exactly one nonempty name, command or url"
            ));
        }
    }
    Ok(patterns)
}

/// Managed servers: a map of complete server definitions, validated like any other.
fn managed_servers(servers: &Value) -> Result<Value, String> {
    let map = servers.as_object().ok_or("mcp: expected a server map")?;
    if let Some(name) = map.keys().find(|name| map[*name].get("type").is_none()) {
        return Err(format!(
            "mcp.{name}: managed entries must be server definitions"
        ));
    }
    super::McpSettings::from_config(&serde_json::json!({ "mcp": servers }))?;
    Ok(servers.clone())
}

fn managed_only(policy: &Map<String, Value>) -> Result<bool, String> {
    let hooks = match policy.get("hooks") {
        None => return Ok(false),
        Some(hooks) => hooks
            .as_object()
            .ok_or("policy.hooks: expected an object")?,
    };
    if let Some(key) = hooks.keys().find(|key| *key != "managed_only") {
        return Err(format!(
            "policy.hooks.{key}: unsupported managed policy key"
        ));
    }
    match hooks.get("managed_only") {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err("policy.hooks.managed_only: expected a boolean".into()),
    }
}

/// Managed hooks are event arrays only, validated like any other hook definition and
/// never substituted.
fn managed_hooks(hooks: &Value) -> Result<Value, String> {
    let events = hooks.as_object().ok_or("hooks: expected an object")?;
    if let Some(key) = events.keys().find(|key| !super::is_event_name(key)) {
        return Err(format!(
            "hooks.{key}: managed hooks support only event arrays"
        ));
    }
    let mut wrapped = Map::new();
    wrapped.insert("hooks".into(), hooks.clone());
    super::HookSettings::from_config(&Value::Object(wrapped))?;
    Ok(hooks.clone())
}
