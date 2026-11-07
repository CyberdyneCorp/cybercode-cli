//! Compiled result schemas and terminal settlement derived from durable calls: a child
//! Session's `return_result`, and a root prompt's `structured_output` (`session-runtime`
//! → Structured output), which belongs to the input that asked for it.

use std::sync::Arc;

use cyber_llm::ToolSpec;
use serde_json::Value;

use super::{CallState, CallStatus, RetrySafety, ToolDef, ToolOutcome};

/// The finalizer tool of a root prompt.
pub(crate) const ROOT_TOOL: &str = "structured_output";
/// The finalizer tool of a child Session.
pub(crate) const CHILD_TOOL: &str = "return_result";
/// One submission plus two validation corrections.
pub(crate) const ROOT_ATTEMPTS: u32 = 3;
/// `session.structured_output.settled.1`: a root prompt's terminal structured outcome.
pub const SETTLED: &str = "session.structured_output.settled.1";
/// Sent with each request while a root prompt's result is open.
pub(crate) const ROOT_INSTRUCTION: &str = "The user asked for a structured result. Do the work, then end your answer by calling structured_output once with the final result as JSON matching its schema. Text outside that call is not part of the result.";
/// The `reason` of the system message that asks again for the finalizer.
pub(crate) const MISSING_REASON: &str = "structured_output_required";
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;

/// The terminal outcome of a root prompt's structured result, kept on its input.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredOutcome {
    /// The input the result answers.
    pub message_id: String,
    /// The validated object; absent when the prompt failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// `StructuredOutputError: <why>` when no valid result was submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Submissions counted, including Turns that ended without the finalizer.
    pub attempts: u32,
}

/// Check a root prompt's schema before admission: an object schema of at most 64 KiB and
/// 64 levels, local `$ref`s only, that compiles.
pub fn check_root_schema(schema: &Value) -> Result<StructuredSchema, String> {
    if schema.to_string().len() > MAX_SCHEMA_BYTES {
        return Err("output_schema: larger than 64 KiB".into());
    }
    if schema["type"] != "object" {
        return Err(
            "output_schema: the root must be an object schema (\"type\": \"object\")".into(),
        );
    }
    check_nodes(schema, 0)?;
    StructuredSchema::new(schema.clone())
}

fn check_nodes(value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err("output_schema: nested deeper than 64 levels".into());
    }
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str)
                && !reference.starts_with('#')
            {
                return Err(format!(
                    "output_schema: only local references are allowed, not \"{reference}\""
                ));
            }
            map.values().try_for_each(|v| check_nodes(v, depth + 1))
        }
        Value::Array(items) => items.iter().try_for_each(|v| check_nodes(v, depth + 1)),
        _ => Ok(()),
    }
}

#[derive(Clone)]
pub struct StructuredSchema {
    schema: Value,
    validator: Arc<jsonschema::Validator>,
}

impl std::fmt::Debug for StructuredSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("StructuredSchema")
            .field(&self.schema)
            .finish()
    }
}

impl PartialEq for StructuredSchema {
    fn eq(&self, other: &Self) -> bool {
        self.schema == other.schema
    }
}

impl StructuredSchema {
    pub fn new(schema: Value) -> Result<Self, String> {
        let validator = jsonschema::validator_for(&schema)
            .map_err(|e| format!("Invalid output_schema: {e}"))?;
        Ok(Self {
            schema,
            validator: Arc::new(validator),
        })
    }

    pub fn schema(&self) -> &Value {
        &self.schema
    }

    pub fn validate(&self, value: &Value) -> Result<(), String> {
        let errors: Vec<_> = self
            .validator
            .iter_errors(value)
            .take(16)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("Invalid result: {}", errors.join("; ")))
        }
    }

    pub(crate) fn definition(&self, root: bool) -> ToolDef {
        let (name, description) = if root {
            (
                ROOT_TOOL,
                "Submit your final answer as JSON matching this schema. Call it once, as your last action, after the work is done; no tools run after a valid result.",
            )
        } else {
            (
                CHILD_TOOL,
                "Return the final validated structured result and end this subagent. No tools run after a valid result.",
            )
        };
        ToolDef {
            scope: crate::runtime::ToolScope::Session,
            deferred: false,
            registration: None,
            spec: ToolSpec {
                name: name.into(),
                description: description.into(),
                input_schema: self.schema.clone(),
            },
            retry_safety: RetrySafety::ReadOnly,
            concurrency_safe: false,
        }
    }

    pub(crate) fn outcome(&self, value: Value) -> ToolOutcome {
        match self.validate(&value) {
            Ok(()) => ToolOutcome::Structured {
                output: value.to_string(),
                value,
            },
            Err(error) => ToolOutcome::Failed(error),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResultState {
    pub schema: Option<StructuredSchema>,
    pub returned: Option<(String, Value)>,
    pub error: Option<(String, String)>,
    pub rejected: bool,
    /// The root input this schema belongs to; `None` for a child Session's result.
    pub input: Option<String>,
    /// Root submissions so far, including Turns that ended without the finalizer.
    pub attempts: u32,
    /// The root input's outcome has been recorded.
    pub settled: bool,
}

impl ResultState {
    /// The result state of a promoted root input with `schema`.
    pub fn root(schema: StructuredSchema, input: String) -> Self {
        Self {
            schema: Some(schema),
            input: Some(input),
            ..Self::default()
        }
    }

    /// The finalizer tool's name.
    pub fn tool(&self) -> &'static str {
        if self.input.is_some() {
            ROOT_TOOL
        } else {
            CHILD_TOOL
        }
    }

    /// A root input still waiting for its result.
    pub fn root_open(&self) -> bool {
        self.input.is_some() && !self.settled
    }

    /// A child's returned result ends its Drain; a root input's does not stop the next.
    pub fn blocks_drain(&self) -> bool {
        self.input.is_none() && self.returned.is_some()
    }

    pub fn settle(&mut self, call: &CallState) {
        let Some(schema) = &self.schema else {
            return;
        };
        if call.name != self.tool() {
            return;
        }
        if self.input.is_some() && matches!(call.status, CallStatus::Ok | CallStatus::Error) {
            self.attempts += 1;
        }
        if call.status == CallStatus::Ok {
            if let Some(value) = &call.structured_output {
                match schema.validate(value) {
                    Ok(()) => self.returned = Some((call.call_id.clone(), value.clone())),
                    Err(error) => self.error = Some((call.call_id.clone(), error)),
                }
            }
        } else if call.status == CallStatus::Error {
            self.error = call
                .output
                .as_ref()
                .map(|error| (call.call_id.clone(), error.clone()));
        }
    }

    pub fn rewind(&mut self, calls: &std::collections::BTreeMap<String, CallState>) {
        self.rejected = false;
        if self
            .returned
            .as_ref()
            .is_some_and(|(id, _)| !calls.contains_key(id))
        {
            self.returned = None;
        }
        if self
            .error
            .as_ref()
            .is_some_and(|(id, _)| !calls.contains_key(id))
        {
            self.error = None;
        }
    }
}
