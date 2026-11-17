//! Byte budgets for privileged instruction assembly. `instructions` stays an array.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_MAX_FILE_BYTES: usize = 32 * 1024;
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstructionLimits {
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for InstructionLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl InstructionLimits {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.get("instruction_limits") else {
            return Ok(Self::default());
        };
        if !value.is_object() {
            return Err("instruction_limits: expected an object".into());
        }
        let limits: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("instruction_limits: {error}"))?;
        if limits.max_file_bytes == 0 || limits.max_total_bytes == 0 {
            return Err("instruction_limits: byte limits must be positive integers".into());
        }
        if limits.max_file_bytes > limits.max_total_bytes {
            return Err("instruction_limits.max_file_bytes must not exceed max_total_bytes".into());
        }
        Ok(limits)
    }
}
