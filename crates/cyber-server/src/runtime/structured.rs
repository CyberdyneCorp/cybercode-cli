//! Compiled child result schemas and terminal settlement derived from durable calls.

use std::sync::Arc;

use cyber_llm::ToolSpec;
use serde_json::Value;

use super::{CallState, CallStatus, RetrySafety, ToolDef, ToolOutcome};

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
            Err(format!(
                "Invalid return_result input: {}",
                errors.join("; ")
            ))
        }
    }

    pub(crate) fn definition(&self) -> ToolDef {
        ToolDef {
            registration: None,
            spec: ToolSpec {
                name: "return_result".into(),
                description: "Return the final validated structured result and end this subagent. No tools run after a valid result.".into(),
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
}

impl ResultState {
    pub fn settle(&mut self, call: &CallState) {
        let Some(schema) = &self.schema else {
            return;
        };
        if call.name != "return_result" {
            return;
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
