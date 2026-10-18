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
        /// A shell command whose stdout is a JSON object of string headers that override
        /// `headers`. It runs outside the sandbox, only once the definition is authorized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers_command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers_refresh_seconds: Option<u32>,
        /// `false` disables OAuth; `true`, an object or an omitted field enables it.
        oauth: Option<McpOAuthSetting>,
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
/// A remote server's `oauth` field. `true` and an omitted field both mean enabled with
/// defaults; the flag keeps its own serialized form so existing approval digests hold.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum McpOAuthSetting {
    Flag(bool),
    Settings(McpOAuth),
}

/// Lowest `oauth.redirect_port`: privileged ports are refused.
pub const OAUTH_REDIRECT_PORT_MIN: u16 = 1024;

/// OAuth client settings. `client_secret` supports `{env:NAME}` and never prints in
/// review or diagnostics.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// Fixed loopback callback port; an ephemeral port is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_port: Option<u16>,
}

impl std::fmt::Debug for McpOAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuth")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "***"))
            .field("scopes", &self.scopes)
            .field("redirect_port", &self.redirect_port)
            .finish()
    }
}

impl McpOAuth {
    fn validate(&self) -> Result<(), String> {
        let printable = |value: &str| !value.is_empty() && !value.chars().any(char::is_control);
        if self.client_id.as_deref().is_some_and(|id| !printable(id)) {
            return Err("oauth.client_id must be nonempty".into());
        }
        if self.client_secret.is_some() && self.client_id.is_none() {
            return Err("oauth.client_secret requires oauth.client_id".into());
        }
        if self
            .client_secret
            .as_deref()
            .is_some_and(|secret| !printable(secret))
        {
            return Err("invalid oauth.client_secret".into());
        }
        if let Some(scopes) = &self.scopes
            && (scopes.is_empty()
                || scopes.iter().any(|scope| {
                    !printable(scope) || scope.contains(char::is_whitespace) || scope.contains('"')
                }))
        {
            return Err("oauth.scopes must be nonempty scope tokens".into());
        }
        if self
            .redirect_port
            .is_some_and(|port| port < OAUTH_REDIRECT_PORT_MIN)
        {
            return Err(format!(
                "oauth.redirect_port must be between {OAUTH_REDIRECT_PORT_MIN} and 65535"
            ));
        }
        Ok(())
    }
}

/// Default and upper bound for `headers_refresh_seconds`.
pub const HEADERS_REFRESH_DEFAULT: u32 = 3600;
pub const HEADERS_REFRESH_MAX: u32 = 86_400;

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
    /// `stdio` for a local command, `remote` for an HTTP (or later WebSocket) server.
    pub fn transport(&self) -> &'static str {
        match self {
            Self::Local { .. } => "stdio",
            Self::Remote { .. } => "remote",
        }
    }

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
    /// The configured remote header command, if any.
    pub fn headers_command(&self) -> Option<&str> {
        match self {
            Self::Remote {
                headers_command, ..
            } => headers_command.as_deref(),
            Self::Local { .. } => None,
        }
    }
    /// Effective OAuth settings for a remote server; `None` when local or `oauth: false`.
    pub fn oauth(&self) -> Option<McpOAuth> {
        match self {
            Self::Remote { oauth, .. } => match oauth {
                None | Some(McpOAuthSetting::Flag(true)) => Some(McpOAuth::default()),
                Some(McpOAuthSetting::Flag(false)) => None,
                Some(McpOAuthSetting::Settings(settings)) => Some(settings.clone()),
            },
            Self::Local { .. } => None,
        }
    }
    /// Seconds between header command runs on a live connection.
    pub fn headers_refresh_seconds(&self) -> u32 {
        match self {
            Self::Remote {
                headers_refresh_seconds: Some(seconds),
                ..
            } => *seconds,
            _ => HEADERS_REFRESH_DEFAULT,
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
                headers_command,
                headers_refresh_seconds,
                oauth,
                ..
            } => {
                validate_headers_command(headers_command.as_deref(), *headers_refresh_seconds)?;
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
                if headers.keys().any(|key| transport_owned_header(key)) {
                    return Err("headers cannot set transport-owned names".into());
                }
                if let Some(McpOAuthSetting::Settings(oauth)) = oauth {
                    oauth.validate()?;
                }
            }
        }
        Ok(())
    }
}

fn validate_headers_command(command: Option<&str>, refresh: Option<u32>) -> Result<(), String> {
    match (command, refresh) {
        (Some(command), _) if command.trim().is_empty() || command.contains('\0') => {
            Err("invalid headers_command".into())
        }
        (None, Some(_)) => Err("headers_refresh_seconds requires headers_command".into()),
        (_, Some(seconds)) if seconds == 0 || seconds > HEADERS_REFRESH_MAX => Err(format!(
            "headers_refresh_seconds must be between 1 and {HEADERS_REFRESH_MAX}"
        )),
        _ => Ok(()),
    }
}

/// Names the remote MCP transport owns: framing, routing, session and content negotiation.
pub fn transport_owned_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name.starts_with("proxy-")
        || matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "mcp-session-id"
                | "mcp-protocol-version"
                | "accept"
                | "content-type"
        )
}

fn normalize_enabled(value: &Value) -> Result<Value, &'static str> {
    let mut definition = value.clone();
    let Some(map) = definition.as_object_mut() else {
        return Ok(definition);
    };
    if let Some(disabled) = map.remove("disabled") {
        let disabled = disabled.as_bool().ok_or("disabled must be boolean")?;
        if let Some(enabled) = map.get("enabled") {
            if enabled.as_bool() != Some(!disabled) {
                return Err("conflicting enabled/disabled flags");
            }
        } else {
            map.insert("enabled".into(), Value::Bool(!disabled));
        }
    }
    Ok(definition)
}

#[derive(Debug, Clone)]
pub struct McpSettings {
    pub servers: BTreeMap<String, McpServer>,
    pub tool_timeout: u32,
    pub sampling_enabled: bool,
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
            sampling_enabled: false,
        };
        let Some(value) = config.get("mcp") else {
            return Ok(settings);
        };
        let map = value.as_object().ok_or("mcp: expected server map")?;
        for (name, value) in map {
            if name == "sampling" && value.get("type").is_none() {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Sampling {
                    #[serde(default)]
                    enabled: bool,
                }
                settings.sampling_enabled = serde_json::from_value::<Sampling>(value.clone())
                    .map_err(|_| "mcp.sampling: expected an enabled boolean".to_string())?
                    .enabled;
                continue;
            }
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
            let definition = normalize_enabled(value)
                .map_err(|_| format!("mcp.{name}: invalid enablement flags"))?;
            let server: McpServer = serde_json::from_value(definition)
                .map_err(|_| format!("mcp.{name}: invalid server definition"))?;
            server
                .validate()
                .map_err(|error| format!("mcp.{name}: {error}"))?;
            settings.servers.insert(name.clone(), server);
        }
        Ok(settings)
    }
}
