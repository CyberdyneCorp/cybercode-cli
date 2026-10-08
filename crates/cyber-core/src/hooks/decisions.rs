//! Validate hook decisions and merge them in declared order.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
/// Increasing precedence; Retry and Block are limited to their respective events.
pub enum HookAction {
    Allow,
    Retry,
    Ask,
    Deny,
    Block,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HookDecision {
    pub decision: Option<HookAction>,
    pub reason: Option<String>,
    pub updated_input: Option<Value>,
    pub additional_context: Option<String>,
    #[serde(rename = "continue")]
    pub continuation: Option<bool>,
    pub stop_reason: Option<String>,
    pub suppress_output: Option<bool>,
}

#[derive(Debug)]
pub struct ParsedDecision {
    pub decision: HookDecision,
    /// The dispatcher reports these as debug diagnostics, without applying them.
    pub ignored_fields: Vec<String>,
}

impl HookDecision {
    pub fn parse(event: &str, value: &Value) -> Result<ParsedDecision, String> {
        if !crate::config::is_event_name(event) {
            return Err(format!("unknown hook event: {event}"));
        }
        let mut object = value
            .as_object()
            .cloned()
            .ok_or("hook decision must be a JSON object")?;
        let mut ignored_fields = Vec::new();
        object.retain(|key, _| {
            let valid = matches!(
                key.as_str(),
                "decision"
                    | "reason"
                    | "additional_context"
                    | "continue"
                    | "stop_reason"
                    | "suppress_output"
            ) || (key == "updated_input"
                && matches!(event, "PreToolUse" | "PermissionDenied"));
            if !valid {
                ignored_fields.push(key.clone());
            }
            valid
        });
        let mut decision: Self = serde_json::from_value(Value::Object(object))
            .map_err(|error| format!("invalid hook decision: {error}"))?;
        if matches!(decision.decision, Some(HookAction::Block)) && event != "Stop"
            || matches!(decision.decision, Some(HookAction::Retry)) && event != "PermissionDenied"
        {
            decision.decision = None;
            ignored_fields.push("decision".into());
        }
        Ok(ParsedDecision {
            decision,
            ignored_fields,
        })
    }

    pub fn merge(&mut self, next: Self) {
        if let Some(action) = next.decision
            && self.decision.is_none_or(|previous| action >= previous)
        {
            self.decision = Some(action);
            self.reason = next.reason;
        }
        if let Some(input) = next.updated_input {
            self.updated_input = Some(input);
        }
        if let Some(context) = next.additional_context {
            match &mut self.additional_context {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(&context);
                }
                None => self.additional_context = Some(context),
            }
        }
        if next.continuation == Some(false) {
            self.continuation = Some(false);
            self.stop_reason = next.stop_reason;
        } else if self.continuation.is_none() {
            self.continuation = next.continuation;
        }
        if next.suppress_output == Some(true) || self.suppress_output.is_none() {
            self.suppress_output = next.suppress_output;
        }
    }

    /// Supply this input to the next handler; the tool host still must revalidate it.
    pub fn input<'a>(&'a self, original: &'a Value) -> &'a Value {
        self.updated_input.as_ref().unwrap_or(original)
    }
}
