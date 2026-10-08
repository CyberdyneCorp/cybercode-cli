//! Validated definitions for hook consumers; parsing never executes a handler.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

const EVENTS: &[&str] = &[
    "Setup",
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "PermissionDenied",
    "Stop",
    "StopFailure",
    "Interrupt",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "InstructionsLoaded",
    "PreModelSwitch",
    "PostModelSwitch",
    "Elicitation",
    "ElicitationResult",
    "DirectoryAdded",
    "Notification",
    "FileChanged",
    "CwdChanged",
    "ConfigChange",
    "TaskCreated",
    "TaskCompleted",
    "WorktreeCreate",
    "WorktreeRemove",
    "JobEnded",
    "GoalEvaluated",
    "GoalCompleted",
    "WorkflowRunStart",
    "WorkflowRunEnd",
    "LoopIteration",
    "ScheduleRun",
    "TeammateIdle",
    "MessageReceived",
];

pub(crate) fn is_event_name(name: &str) -> bool {
    EVENTS.contains(&name)
}

#[derive(Debug, Clone)]
pub struct HookSettings {
    pub events: BTreeMap<String, Vec<HookGroup>>,
    pub concurrency: usize,
    pub max_stop_continuations: usize,
    pub sandbox_all: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookGroup {
    pub matcher: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    pub hooks: Vec<HookHandler>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    Command,
    Http,
    Prompt,
    McpTool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookCondition {
    pub field: String,
    pub matches: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookHandler {
    #[serde(rename = "type")]
    pub kind: HookKind,
    pub command: Option<String>,
    pub url: Option<String>,
    pub prompt: Option<String>,
    pub server: Option<String>,
    pub tool: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub arguments: Option<Value>,
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    #[serde(default, rename = "async")]
    pub asynchronous: bool,
    pub id: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub fail_closed: bool,
    #[serde(rename = "if")]
    pub condition: Option<HookCondition>,
    #[serde(default)]
    pub once: bool,
    pub status_message: Option<String>,
    pub system_message: Option<String>,
}

impl HookHandler {
    /// Hash the effective definition, including defaults and all execution options.
    pub fn digest(&self) -> Result<String, serde_json::Error> {
        let value = serde_json::to_value(self)?;
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(super::canonical_json(&value))
        ))
    }
}

fn default_timeout() -> u32 {
    60
}

impl HookSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let empty = serde_json::Map::new();
        let map = match config.get("hooks") {
            None => &empty,
            Some(value) => value.as_object().ok_or("hooks: expected an object")?,
        };
        let concurrency: usize = control(map, "concurrency", 8)?;
        if concurrency == 0 {
            return Err("hooks.concurrency: expected a positive integer".into());
        }
        let mut events = BTreeMap::new();
        for (event, value) in map {
            if matches!(
                event.as_str(),
                "concurrency" | "max_stop_continuations" | "sandbox_all"
            ) {
                continue;
            }
            if !is_event_name(event) {
                return Err(format!("hooks.{event}: unknown hook event"));
            }
            let groups = value
                .as_array()
                .ok_or_else(|| format!("hooks.{event}: expected an array of groups"))?;
            let mut parsed = Vec::with_capacity(groups.len());
            for (index, group) in groups.iter().enumerate() {
                let path = format!("hooks.{event}[{index}]");
                let group: HookGroup = serde_json::from_value(group.clone())
                    .map_err(|error| format!("{path}: {error}"))?;
                group.validate(&path)?;
                parsed.push(group);
            }
            events.insert(event.clone(), parsed);
        }
        Ok(Self {
            events,
            concurrency,
            max_stop_continuations: control(map, "max_stop_continuations", 5)?,
            sandbox_all: control(map, "sandbox_all", false)?,
        })
    }
}

fn control<T: DeserializeOwned>(
    map: &serde_json::Map<String, Value>,
    key: &str,
    default: T,
) -> Result<T, String> {
    match map.get(key) {
        None => Ok(default),
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|error| format!("hooks.{key}: {error}"))
        }
    }
}

impl HookGroup {
    fn validate(&self, path: &str) -> Result<(), String> {
        if let Some(matcher) = &self.matcher {
            selector(matcher, &format!("{path}.matcher"))?;
        }
        for (index, pattern) in self.paths.iter().enumerate() {
            glob(pattern, &format!("{path}.paths[{index}]"))?;
        }
        for (index, handler) in self.hooks.iter().enumerate() {
            handler.validate(&format!("{path}.hooks[{index}]"))?;
        }
        Ok(())
    }
}

impl HookHandler {
    fn validate(&self, path: &str) -> Result<(), String> {
        if !(1..=600).contains(&self.timeout) {
            return Err(format!("{path}.timeout: expected 1 through 600 seconds"));
        }
        if self.id.as_ref().is_some_and(|id| id.trim().is_empty()) {
            return Err(format!("{path}.id: expected a nonempty string"));
        }
        self.validate_payload(path)?;
        if let Some(condition) = &self.condition {
            if condition.field.split('.').any(str::is_empty) {
                return Err(format!("{path}.if.field: expected a dotted field name"));
            }
            regex(&condition.matches, &format!("{path}.if.matches"))?;
        }
        Ok(())
    }

    fn validate_payload(&self, path: &str) -> Result<(), String> {
        let fields = [
            ("command", self.command.is_some(), HookKind::Command),
            ("url", self.url.is_some(), HookKind::Http),
            ("prompt", self.prompt.is_some(), HookKind::Prompt),
            ("server", self.server.is_some(), HookKind::McpTool),
            ("tool", self.tool.is_some(), HookKind::McpTool),
            ("headers", !self.headers.is_empty(), HookKind::Http),
            ("arguments", self.arguments.is_some(), HookKind::McpTool),
        ];
        for (field, present, owner) in fields {
            if present && self.kind != owner {
                return Err(format!(
                    "{path}.{field}: not supported by this handler type"
                ));
            }
        }
        let required = match self.kind {
            HookKind::Command => vec![("command", &self.command)],
            HookKind::Http => vec![("url", &self.url)],
            HookKind::Prompt => vec![("prompt", &self.prompt)],
            HookKind::McpTool => vec![("server", &self.server), ("tool", &self.tool)],
        };
        for (field, value) in required {
            if value.as_ref().is_none_or(|value| value.trim().is_empty()) {
                return Err(format!("{path}.{field}: required nonempty string"));
            }
        }
        if let Some(url) = &self.url {
            let url = url::Url::parse(url).map_err(|error| format!("{path}.url: {error}"))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(format!("{path}.url: expected an absolute HTTP(S) URL"));
            }
        }
        Ok(())
    }
}

fn selector(pattern: &str, path: &str) -> Result<(), String> {
    if let Some(pattern) = pattern
        .strip_prefix('/')
        .and_then(|value| value.strip_suffix('/'))
    {
        regex(pattern, path)
    } else {
        glob(pattern, path)
    }
}

fn regex(pattern: &str, path: &str) -> Result<(), String> {
    bounded(pattern, path)?;
    regex::Regex::new(pattern)
        .map(drop)
        .map_err(|error| format!("{path}: invalid regex: {error}"))
}

fn glob(pattern: &str, path: &str) -> Result<(), String> {
    bounded(pattern, path)?;
    globset::Glob::new(pattern)
        .map(drop)
        .map_err(|error| format!("{path}: invalid glob: {error}"))
}

fn bounded(pattern: &str, path: &str) -> Result<(), String> {
    if pattern.len() > 65_536 {
        return Err(format!("{path}: selector exceeds 65536 bytes"));
    }
    Ok(())
}
