//! Keep only pending driver inputs; emitted answers leave bounded accounting.
use std::collections::BTreeMap;

use serde_json::Value;

use super::EXIT_ERROR;

const MAX_PENDING: usize = 32;

pub(super) struct Input {
    ordinal: u64,
    pub id: Option<String>,
    pub message_id: String,
    pub text: String,
    pub error: Option<String>,
    pub structured: Option<Value>,
}

impl Input {
    pub fn new(id: Option<String>, message_id: String) -> Self {
        Self {
            ordinal: 0,
            id,
            message_id,
            text: String::new(),
            error: None,
            structured: None,
        }
    }

    pub fn exit_code(&self) -> u8 {
        if self.error.is_some() { EXIT_ERROR } else { 0 }
    }
}

#[derive(Default)]
pub(super) struct Inputs {
    pending: BTreeMap<String, Input>,
    total: u64,
    failure: Option<(u64, u8)>,
}

impl Inputs {
    /// Reserve correlation before the HTTP request; refusal retires the same slot.
    pub fn admit(&mut self, mut input: Input) -> Result<(), &'static str> {
        if self.pending.len() >= MAX_PENDING {
            return Err("more than 32 inputs waiting for an answer");
        }
        if self.pending.contains_key(&input.message_id) {
            return Err("duplicate driver message identity");
        }
        let total = self.total.checked_add(1).ok_or("input count exhausted")?;
        input.ordinal = self.total;
        self.total = total;
        self.pending.insert(input.message_id.clone(), input);
        Ok(())
    }

    pub fn contains(&self, message_id: &str) -> bool {
        self.pending.contains_key(message_id)
    }

    pub fn get_mut(&mut self, message_id: &str) -> Option<&mut Input> {
        self.pending.get_mut(message_id)
    }

    /// The caller emits this value and drops it; no completed payload is retained.
    pub fn complete(&mut self, message_id: &str) -> Option<Input> {
        let input = self.pending.remove(message_id)?;
        self.record_failure(&input);
        Some(input)
    }

    pub fn unfinished(&mut self) -> Vec<Input> {
        let mut pending: Vec<_> = std::mem::take(&mut self.pending).into_values().collect();
        pending.sort_by_key(|input| input.ordinal);
        for input in &mut pending {
            input
                .error
                .get_or_insert_with(|| "not answered before the run ended".into());
            self.record_failure(input);
        }
        pending
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn failure_code(&self) -> Option<u8> {
        self.failure.map(|(_, code)| code)
    }

    fn record_failure(&mut self, input: &Input) {
        let code = input.exit_code();
        if code != 0
            && self
                .failure
                .is_none_or(|(ordinal, _)| input.ordinal < ordinal)
        {
            self.failure = Some((input.ordinal, code));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: usize) -> Input {
        let mut input = Input::new(Some(id.to_string()), format!("msg_{id}"));
        input.text = "answer".repeat(1024);
        input.structured = Some(serde_json::json!({"answer":"structured"}));
        input
    }

    #[test]
    fn completed_payloads_release_capacity_and_failures_survive_long_runs() {
        let mut inputs = Inputs::default();
        for index in 0..4096 {
            inputs.admit(input(index)).unwrap();
            if index == 1 {
                inputs.get_mut("msg_1").unwrap().error = Some("refused".into());
            }
            let done = inputs.complete(&format!("msg_{index}")).unwrap();
            assert_eq!(done.id, Some(index.to_string()));
            assert!(!done.text.is_empty());
            assert!(done.structured.is_some());
            assert!(inputs.pending.is_empty());
        }
        assert_eq!(inputs.total(), 4096);
        assert_eq!(inputs.failure_code(), Some(EXIT_ERROR));
        assert!(inputs.unfinished().is_empty());
    }

    #[test]
    fn capacity_refuses_before_counting_and_retirement_preserves_other_identities() {
        let mut inputs = Inputs::default();
        for index in 0..MAX_PENDING {
            inputs.admit(input(index)).unwrap();
        }
        assert!(inputs.admit(input(100)).is_err());
        assert_eq!(inputs.total(), MAX_PENDING as u64);
        inputs.complete("msg_0").unwrap();
        assert!(inputs.contains("msg_31"));
        assert!(!inputs.contains("msg_0"));
        inputs.admit(input(100)).unwrap();
        inputs.get_mut("msg_31").unwrap().error = Some("existing error".into());
        let remaining = inputs.unfinished();
        assert_eq!(remaining.len(), MAX_PENDING);
        assert_eq!(remaining[0].message_id, "msg_1");
        assert_eq!(remaining.last().unwrap().message_id, "msg_100");
        assert_eq!(remaining[30].error.as_deref(), Some("existing error"));
        assert!(remaining.iter().all(|input| input.error.is_some()));
        assert_eq!(inputs.total(), MAX_PENDING as u64 + 1);
        assert!(inputs.is_empty());
        assert_eq!(inputs.failure_code(), Some(EXIT_ERROR));
    }
}
