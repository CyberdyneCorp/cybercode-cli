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
    pub answer_ready: bool,
    pub ready_seq: i64,
    pub unnotified_since: Option<std::time::Instant>,
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
            answer_ready: false,
            ready_seq: -1,
            unnotified_since: None,
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

    pub fn identities(&self) -> Vec<String> {
        let mut inputs: Vec<_> = self.pending.values().collect();
        inputs.sort_by_key(|input| input.ordinal);
        inputs
            .into_iter()
            .map(|input| input.message_id.clone())
            .collect()
    }

    pub fn has_ready(&self) -> bool {
        self.pending.values().any(|input| input.answer_ready)
    }

    pub fn owns_cohort(&self, ids: &[String]) -> bool {
        !ids.is_empty()
            && ids.len() <= MAX_PENDING
            && ids.iter().all(|id| self.contains(id))
            && ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len()
    }

    /// Only a complete server-recorded cohort can authorize a driver reply.
    /// Current Session activity and assistant history are not ownership proof.
    #[cfg(test)]
    pub fn owns_request(&self, session: &str, data: &Value) -> bool {
        self.request_cohort(session, data).is_some()
    }

    /// The owned cohort of a request: its own step cohort when `session` asked it, or,
    /// for a descendant (subagent) request routed here, the root cohort its lineage
    /// inherited from `session`. Every identity must be a pending input of this driver.
    pub fn request_cohort(&self, session: &str, data: &Value) -> Option<Vec<String>> {
        if data["message_id"].as_str().is_none_or(str::is_empty)
            || data["call_id"].as_str().is_none_or(str::is_empty)
        {
            return None;
        }
        let ids = if data["session_id"].as_str() == Some(session) {
            &data["input_message_ids"]
        } else if data["root_session_id"].as_str() == Some(session) {
            &data["root_input_message_ids"]
        } else {
            return None;
        };
        let ids: Vec<String> = ids
            .as_array()?
            .iter()
            .map(|id| id.as_str().map(str::to_owned))
            .collect::<Option<_>>()?;
        self.owns_cohort(&ids).then_some(ids)
    }

    pub fn get(&self, message_id: &str) -> Option<&Input> {
        self.pending.get(message_id)
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
    fn request_ownership_requires_every_live_input_and_exact_session_call_evidence() {
        let mut inputs = Inputs::default();
        inputs.admit(input(0)).unwrap();
        inputs.admit(input(1)).unwrap();
        let owned = serde_json::json!({"session_id":"session","message_id":"assistant","call_id":"call","input_message_ids":["msg_0","msg_1"]});
        assert!(inputs.owns_request("session", &owned));
        for (field, value) in [
            ("session_id", serde_json::json!("other")),
            ("message_id", Value::Null),
            ("call_id", Value::Null),
            ("input_message_ids", serde_json::json!([])),
            ("input_message_ids", serde_json::json!(["msg_0", "foreign"])),
            ("input_message_ids", serde_json::json!(["msg_0", 3])),
        ] {
            let mut unknown = owned.clone();
            unknown[field] = value;
            assert!(!inputs.owns_request("session", &unknown), "{unknown}");
        }
        inputs.complete("msg_0").unwrap();
        assert!(!inputs.owns_request("session", &owned));
    }

    #[test]
    fn a_descendant_request_is_owned_only_through_its_complete_root_cohort() {
        let mut inputs = Inputs::default();
        inputs.admit(input(0)).unwrap();
        inputs.admit(input(1)).unwrap();
        let child = serde_json::json!({"session_id":"child","message_id":"assistant","call_id":"call",
            "input_message_ids":["child_input"],"root_session_id":"session","root_input_message_ids":["msg_1"]});
        assert_eq!(
            inputs.request_cohort("session", &child),
            Some(vec!["msg_1".to_owned()])
        );
        // The child's own cohort never stands in for the root cohort.
        assert!(!inputs.owns_request("child", &child));
        for (field, value) in [
            ("root_session_id", serde_json::json!("other")),
            ("root_session_id", Value::Null),
            ("root_input_message_ids", serde_json::json!([])),
            (
                "root_input_message_ids",
                serde_json::json!(["msg_1", "foreign"]),
            ),
            (
                "root_input_message_ids",
                serde_json::json!(["msg_1", "msg_1"]),
            ),
            ("root_input_message_ids", serde_json::json!(["msg_1", 3])),
            ("call_id", Value::Null),
        ] {
            let mut unknown = child.clone();
            unknown[field] = value;
            assert!(!inputs.owns_request("session", &unknown), "{unknown}");
        }
        inputs.complete("msg_1").unwrap();
        assert!(!inputs.owns_request("session", &child));
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
