//! Typed, trust-gated MCP server settings. Parsing never starts a server.
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum McpServer {
    Local {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
        #[serde(default = "enabled")]
        enabled: bool,
        #[serde(
            default,
            deserialize_with = "explicit_timeout",
            serialize_with = "normalized_timeout"
        )]
        timeout: Option<u32>,
        #[serde(default)]
        tools: McpToolFilter,
        #[serde(default, skip_serializing_if = "not_required")]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_token_limit: Option<usize>,
    },
    Remote {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        oauth: Option<Value>,
        #[serde(default = "enabled")]
        enabled: bool,
        #[serde(
            default,
            deserialize_with = "explicit_timeout",
            serialize_with = "normalized_timeout"
        )]
        timeout: Option<u32>,
        #[serde(default)]
        tools: McpToolFilter,
        #[serde(default, skip_serializing_if = "not_required")]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_token_limit: Option<usize>,
    },
}
fn not_required(value: &bool) -> bool {
    !value
}
fn enabled() -> bool {
    true
}
fn explicit_timeout<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    u32::deserialize(deserializer).map(Some)
}
fn normalized_timeout<S: serde::Serializer>(
    timeout: &Option<u32>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    timeout.unwrap_or(30).serialize(serializer)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpToolFilter {
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub deny: Vec<String>,
}
impl McpToolFilter {
    pub fn permits(&self, tool: &str) -> Result<bool, String> {
        let matches = |patterns: &[String]| -> Result<bool, String> {
            let mut matched = false;
            for pattern in patterns {
                let glob =
                    globset::Glob::new(pattern).map_err(|_| "invalid MCP tool glob".to_string())?;
                matched |= glob.compile_matcher().is_match(tool);
            }
            Ok(matched)
        };
        Ok(!matches(&self.deny)?
            && match &self.allow {
                Some(allow) => matches(allow)?,
                None => true,
            })
    }
    fn validate(&self) -> Result<(), String> {
        for pattern in self.allow.iter().flatten().chain(&self.deny) {
            globset::Glob::new(pattern).map_err(|_| "invalid MCP tool glob".to_string())?;
        }
        Ok(())
    }
}
impl McpServer {
    /// Bind approval to the server name and every effective field, including defaults.
    pub fn digest(&self, name: &str) -> Result<String, serde_json::Error> {
        use sha2::{Digest, Sha256};
        let mut definition = serde_json::json!({"kind":"mcp_server","name":name,"server":self});
        if let Some(timeout) = self.call_timeout() {
            definition["call_timeout_override"] = serde_json::json!(timeout);
        }
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(super::canonical_json(&definition))
        ))
    }
    pub fn enabled(&self) -> bool {
        match self {
            Self::Local { enabled, .. } | Self::Remote { enabled, .. } => *enabled,
        }
    }
    pub fn required(&self) -> bool {
        match self {
            Self::Local { required, .. } | Self::Remote { required, .. } => *required,
        }
    }
    pub fn output_token_limit(&self) -> Option<usize> {
        match self {
            Self::Local {
                output_token_limit, ..
            }
            | Self::Remote {
                output_token_limit, ..
            } => *output_token_limit,
        }
    }
    pub fn timeout(&self) -> u32 {
        self.call_timeout().unwrap_or(30)
    }
    pub fn call_timeout(&self) -> Option<u32> {
        match self {
            Self::Local { timeout, .. } | Self::Remote { timeout, .. } => *timeout,
        }
    }
    pub fn tools(&self) -> &McpToolFilter {
        match self {
            Self::Local { tools, .. } | Self::Remote { tools, .. } => tools,
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.timeout() == 0 {
            return Err("timeout must be positive".into());
        }
        if self.output_token_limit() == Some(0) {
            return Err("output_token_limit must be positive".into());
        }
        self.tools().validate()?;
        match self {
            Self::Local {
                command,
                args,
                env,
                cwd,
                ..
            } => {
                if command.trim().is_empty()
                    || command.contains('\0')
                    || args.iter().any(|arg| arg.contains('\0'))
                {
                    return Err("invalid command or arguments".into());
                }
                if env.iter().any(|(key, value)| {
                    key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
                }) {
                    return Err("invalid environment".into());
                }
                if cwd.as_ref().is_some_and(|cwd| cwd.as_os_str().is_empty()) {
                    return Err("cwd must be nonempty".into());
                }
            }
            Self::Remote {
                url,
                headers,
                oauth,
                ..
            } => {
                let parsed = url::Url::parse(url).map_err(|_| "invalid remote URL".to_string())?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err("remote URL must be http(s) without userinfo or fragment".into());
                }
                if headers.iter().any(|(key, value)| {
                    key.trim().is_empty()
                        || key.contains(['\r', '\n', '\0'])
                        || value.contains(['\r', '\n', '\0'])
                }) {
                    return Err("invalid headers".into());
                }
                if oauth
                    .as_ref()
                    .is_some_and(|oauth| !oauth.is_boolean() && !oauth.is_object())
                {
                    return Err("oauth must be boolean or object".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct McpSettings {
    pub servers: BTreeMap<String, McpServer>,
    pub tool_timeout: u32,
}
impl McpSettings {
    pub fn call_timeout(&self, name: &str) -> u32 {
        self.servers
            .get(name)
            .and_then(McpServer::call_timeout)
            .unwrap_or(self.tool_timeout)
    }
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self {
            servers: BTreeMap::new(),
            tool_timeout: 300,
        };
        let Some(value) = config.get("mcp") else {
            return Ok(settings);
        };
        let map = value.as_object().ok_or("mcp: expected server map")?;
        for (name, value) in map {
            if name == "tool_timeout" && !value.is_object() {
                settings.tool_timeout = value
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or("mcp.tool_timeout: expected positive seconds")?;
                continue;
            }
            if name.is_empty()
                || name.len() > 48
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(format!("mcp.{name}: invalid server name"));
            }
            let server: McpServer = serde_json::from_value(value.clone())
                .map_err(|_| format!("mcp.{name}: invalid server definition"))?;
            server
                .validate()
                .map_err(|error| format!("mcp.{name}: {error}"))?;
            settings.servers.insert(name.clone(), server);
        }
        Ok(settings)
    }
}
