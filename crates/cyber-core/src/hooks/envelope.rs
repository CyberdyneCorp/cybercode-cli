//! Immutable hook envelopes constructed from an owner's captured execution identity.

use std::path::PathBuf;

use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use super::HookDefinition;

const RESERVED: &[&str] = &[
    "event",
    "session_id",
    "location",
    "project_id",
    "agent",
    "mode",
    "timestamp",
];

#[derive(Debug, Clone, Serialize)]
pub struct HookLocation {
    pub directory: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookIdentity {
    pub session_id: String,
    pub location: HookLocation,
    pub project_id: String,
    pub agent: String,
    pub mode: String,
}

impl HookIdentity {
    fn validate(&self) -> Result<(), String> {
        if !crate::ids::has_prefix(&self.session_id, "ses") {
            return Err("hook envelope.session_id: expected a Session ID".into());
        }
        if self.project_id != "global" && !crate::ids::has_prefix(&self.project_id, "prj") {
            return Err("hook envelope.project_id: expected a project ID or global".into());
        }
        if !self.location.directory.is_absolute() {
            return Err("hook envelope.location.directory: expected an absolute Location".into());
        }
        if self.agent.trim().is_empty() || !crate::config::is_mode(&self.mode) {
            return Err("hook envelope: expected an agent and a valid permission Mode".into());
        }
        if self
            .location
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.trim().is_empty())
        {
            return Err("hook envelope.location.workspace: expected a nonempty workspace".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HookEvent {
    identity: HookIdentity,
    event: String,
    payload: Value,
}

impl HookEvent {
    /// Timestamp is Unix milliseconds. This validates shape, not execution authority.
    /// The runtime must supply its captured identity and own every subsequent effect.
    pub fn new(
        event: impl Into<String>,
        identity: HookIdentity,
        timestamp_ms: i64,
        fields: Map<String, Value>,
    ) -> Result<Self, String> {
        let event = event.into();
        if !crate::config::is_event_name(&event) {
            return Err(format!("unknown hook event: {event}"));
        }
        identity.validate()?;
        if timestamp_ms < 0 {
            return Err("hook envelope.timestamp: expected nonnegative Unix milliseconds".into());
        }
        if let Some(key) = fields.keys().find(|key| RESERVED.contains(&key.as_str())) {
            return Err(format!("hook payload cannot replace envelope field: {key}"));
        }
        let mut payload = serde_json::to_value(&identity).map_err(|error| error.to_string())?;
        let object = payload
            .as_object_mut()
            .ok_or("invalid hook identity serialization")?;
        object.insert("event".into(), Value::String(event.clone()));
        object.insert("timestamp".into(), timestamp_ms.into());
        object.extend(fields);
        Ok(Self {
            identity,
            event,
            payload,
        })
    }

    pub fn event(&self) -> &str {
        &self.event
    }
    pub fn identity(&self) -> &HookIdentity {
        &self.identity
    }
    pub fn as_json(&self) -> &Value {
        &self.payload
    }

    /// Retain the captured envelope while feeding a rewrite to subsequent handlers.
    /// Tool-schema validation and permission evaluation belong to the owning tool host.
    pub fn with_tool_input(&self, input: Value) -> Result<Self, String> {
        if !matches!(self.event.as_str(), "PreToolUse" | "PermissionDenied") {
            return Err(format!(
                "{} does not accept rewritten tool input",
                self.event
            ));
        }
        let mut event = self.clone();
        event.payload["tool_input"] = input;
        Ok(event)
    }
}

impl Serialize for HookEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.payload.serialize(serializer)
    }
}

impl HookDefinition {
    pub fn matches_event(
        &self,
        event: &HookEvent,
        subject: &str,
        relative_paths: &[String],
    ) -> bool {
        self.matches(event.event(), subject, relative_paths, event.as_json())
    }
}
