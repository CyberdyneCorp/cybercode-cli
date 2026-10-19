//! The plugin host fields of `cyber-plugin.json` (`plugins-marketplace` → Plugin host
//! process, Capability declaration and enforcement): `main`, `runtime` and
//! `capabilities`. Parsing is strict; an unknown capability fails the manifest.

use serde::Serialize;
use serde_json::{Map, Value};

use super::manifest::{ManifestError, inside};

/// How the host process of `main` is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Node,
    Bun,
    Deno,
    Python,
    /// `main` is itself an executable.
    Native,
}

impl Runtime {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "node" => Self::Node,
            "bun" => Self::Bun,
            "deno" => Self::Deno,
            "python" => Self::Python,
            "native" => Self::Native,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Bun => "bun",
            Self::Deno => "deno",
            Self::Python => "python",
            Self::Native => "native",
        }
    }

    /// The runtime a `main` without `runtime` implies: JavaScript and TypeScript run on
    /// Node, Python files on Python, anything else is an executable.
    fn infer(main: &str) -> Self {
        let extension = main.rsplit_once('.').map(|(_, ext)| ext);
        match extension {
            Some("js" | "mjs" | "cjs") => Self::Node,
            Some("ts" | "mts") => Self::Deno,
            Some("py") => Self::Python,
            _ => Self::Native,
        }
    }
}

/// The plugin host entry: `main` relative to the plugin root and its runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostEntry {
    pub main: String,
    pub runtime: Runtime,
}

/// Network access a plugin declares.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "hosts")]
pub enum NetworkCapability {
    /// Not declared: every connection is a sandbox denial.
    #[default]
    None,
    /// `"network": true`.
    Any,
    /// `"network": { "hosts": [...] }`: only these hosts, through the allowlist proxy.
    Hosts(Vec<String>),
}

impl NetworkCapability {
    /// Whether `host` is within the declaration (`*.example.com` matches subdomains).
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::None => false,
            Self::Any => true,
            Self::Hosts(hosts) => hosts
                .iter()
                .any(|allowed| match allowed.strip_prefix("*.") {
                    Some(base) => host.ends_with(&format!(".{base}")),
                    None => host == *allowed,
                }),
        }
    }
}

/// The declared capabilities; anything not declared is denied.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, schemars::JsonSchema)]
pub struct Capabilities {
    #[serde(rename = "fs.read")]
    pub fs_read: bool,
    #[serde(rename = "fs.write")]
    pub fs_write: bool,
    pub network: NetworkCapability,
    pub shell: bool,
    pub secrets: bool,
    #[serde(rename = "session.prompt")]
    pub session_prompt: bool,
}

const FLAGS: [&str; 5] = ["fs.read", "fs.write", "shell", "secrets", "session.prompt"];

impl Capabilities {
    /// The declared capabilities as the review prompt and `plugins info` list them.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (on, name) in [
            (self.fs_read, "fs.read"),
            (self.fs_write, "fs.write"),
            (self.shell, "shell"),
            (self.secrets, "secrets"),
            (self.session_prompt, "session.prompt"),
        ] {
            if on {
                out.push(name.to_owned());
            }
        }
        match &self.network {
            NetworkCapability::None => {}
            NetworkCapability::Any => out.push("network".into()),
            NetworkCapability::Hosts(hosts) => out.push(format!("network ({})", hosts.join(", "))),
        }
        out
    }
}

fn error(field: &str, message: impl Into<String>) -> ManifestError {
    ManifestError {
        field: field.into(),
        message: message.into(),
    }
}

pub(super) fn capabilities(map: &Map<String, Value>) -> Result<Capabilities, ManifestError> {
    let Some(value) = map.get("capabilities") else {
        return Ok(Capabilities::default());
    };
    let declared = value
        .as_object()
        .ok_or_else(|| error("capabilities", "must be an object"))?;
    let mut out = Capabilities::default();
    for (key, value) in declared {
        let field = format!("capabilities.{key}");
        if key == "network" {
            out.network = network(&field, value)?;
            continue;
        }
        if !FLAGS.contains(&key.as_str()) {
            return Err(error(&field, "unknown capability"));
        }
        let on = value
            .as_bool()
            .ok_or_else(|| error(&field, "must be true or false"))?;
        match key.as_str() {
            "fs.read" => out.fs_read = on,
            "fs.write" => out.fs_write = on,
            "shell" => out.shell = on,
            "secrets" => out.secrets = on,
            _ => out.session_prompt = on,
        }
    }
    Ok(out)
}

fn network(field: &str, value: &Value) -> Result<NetworkCapability, ManifestError> {
    match value {
        Value::Bool(true) => Ok(NetworkCapability::Any),
        Value::Bool(false) => Ok(NetworkCapability::None),
        Value::Object(map) => {
            if let Some(key) = map.keys().find(|key| *key != "hosts") {
                return Err(error(&format!("{field}.{key}"), "unknown field"));
            }
            let hosts = map
                .get("hosts")
                .and_then(Value::as_array)
                .ok_or_else(|| error(&format!("{field}.hosts"), "must be a list of hosts"))?;
            let hosts = hosts
                .iter()
                .map(|host| host.as_str().filter(|host| valid_host(host)))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| error(&format!("{field}.hosts"), "must be a list of hosts"))?;
            Ok(NetworkCapability::Hosts(
                hosts.into_iter().map(str::to_ascii_lowercase).collect(),
            ))
        }
        _ => Err(error(field, "must be true, false or { \"hosts\": [...] }")),
    }
}

/// A host name or a `*.` wildcard of one; no scheme, port or path.
fn valid_host(host: &str) -> bool {
    let name = host.strip_prefix("*.").unwrap_or(host);
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

pub(super) fn host_entry(map: &Map<String, Value>) -> Result<Option<HostEntry>, ManifestError> {
    let main = match map.get("main") {
        None => None,
        Some(Value::String(main)) => Some(main.clone()),
        Some(_) => return Err(error("main", "must be a string")),
    };
    let runtime = match map.get("runtime") {
        None => None,
        Some(Value::String(text)) => Some(Runtime::parse(text).ok_or_else(|| {
            error(
                "runtime",
                format!("{text:?} is not one of node, bun, deno, python or native"),
            )
        })?),
        Some(_) => return Err(error("runtime", "must be a string")),
    };
    let Some(main) = main else {
        return match runtime {
            Some(_) => Err(error("runtime", "requires main")),
            None => Ok(None),
        };
    };
    if inside(&main).is_none() {
        return Err(error(
            "main",
            format!("{main:?} must be a relative path inside the plugin"),
        ));
    }
    let runtime = runtime.unwrap_or_else(|| Runtime::infer(&main));
    Ok(Some(HostEntry { main, runtime }))
}

#[cfg(test)]
mod tests {
    use super::super::manifest::Manifest;
    use super::*;

    fn parse(extra: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(&format!(
            r#"{{"name":"jira-kit","version":"1.0.0"{extra}}}"#
        ))
    }

    #[test]
    fn capabilities_are_typed_and_unknown_ones_fail_the_manifest() {
        let manifest = parse(
            r#","main":"dist/index.js","capabilities":{"fs.write":true,"network":{"hosts":["API.example.com","*.atlassian.net"]}}"#,
        )
        .unwrap();
        let host = manifest.host.unwrap();
        assert_eq!(host.runtime, Runtime::Node);
        assert!(manifest.capabilities.fs_write && !manifest.capabilities.fs_read);
        assert_eq!(
            manifest.capabilities.network,
            NetworkCapability::Hosts(vec!["api.example.com".into(), "*.atlassian.net".into()])
        );
        assert_eq!(
            manifest.capabilities.describe(),
            ["fs.write", "network (api.example.com, *.atlassian.net)"]
        );
        for (extra, field) in [
            (r#","capabilities":{"camera":true}"#, "capabilities.camera"),
            (r#","capabilities":{"shell":"yes"}"#, "capabilities.shell"),
            (
                r#","capabilities":{"network":{"hosts":["https://x"]}}"#,
                "capabilities.network.hosts",
            ),
            (r#","runtime":"node""#, "runtime"),
            (r#","main":"../x.js""#, "main"),
            (r#","main":"x","runtime":"ruby""#, "runtime"),
        ] {
            assert_eq!(parse(extra).unwrap_err().field, field, "{extra}");
        }
    }

    #[test]
    fn runtime_is_inferred_from_main() {
        for (main, runtime) in [
            ("index.mjs", Runtime::Node),
            ("main.py", Runtime::Python),
            ("mod.ts", Runtime::Deno),
            ("bin/host", Runtime::Native),
        ] {
            let manifest = parse(&format!(r#","main":"{main}""#)).unwrap();
            assert_eq!(manifest.host.unwrap().runtime, runtime, "{main}");
        }
        let explicit = parse(r#","main":"index.js","runtime":"bun""#).unwrap();
        assert_eq!(explicit.host.unwrap().runtime, Runtime::Bun);
    }
}
