//! Reasoning carriers of Chat Completions servers (`session-runtime` → Reasoning items):
//! `reasoning_content` (DeepSeek, Kimi, GLM, Qwen, vLLM), `reasoning` (Ollama, LM Studio),
//! `reasoning_details` (OpenRouter), `reasoning_opaque` (Copilot), and the Gemini thought
//! signature on a tool call. The profile names the carrier; the decoder reads them all.

use serde_json::{Map, Value, json};

use crate::catalog::profile::ReasoningCarrier;
use crate::types::{LlmEvent, Opaque, OpaqueKind};

/// The reasoning of one assistant message, gathered from its items.
#[derive(Default)]
pub(super) struct Gathered<'a> {
    pub present: bool,
    pub text: String,
    pub opaque: Vec<&'a Opaque>,
}

impl<'a> Gathered<'a> {
    pub fn add(&mut self, text: &str, opaque: Option<&'a Opaque>) {
        self.present = true;
        self.text.push_str(text);
        self.opaque.extend(opaque);
    }

    fn of_kind(&self, kind: OpaqueKind) -> impl Iterator<Item = &'a Opaque> + '_ {
        self.opaque.iter().copied().filter(move |o| o.kind == kind)
    }
}

/// Set the carrier's field on an assistant message.
pub(super) fn carry(message: &mut Value, carrier: ReasoningCarrier, reasoning: &Gathered<'_>) {
    if !reasoning.present {
        return;
    }
    let text = reasoning.text.as_str();
    match carrier {
        // Some servers reject an empty string, so empty reasoning is one space.
        ReasoningCarrier::ReasoningContent => {
            message["reasoning_content"] = json!(if text.is_empty() { " " } else { text });
        }
        ReasoningCarrier::Reasoning if !text.is_empty() => message["reasoning"] = json!(text),
        ReasoningCarrier::ReasoningDetails => {
            let details: Vec<Value> = reasoning
                .of_kind(OpaqueKind::Details)
                .filter_map(|o| serde_json::from_str::<Vec<Value>>(&o.data).ok())
                .flatten()
                .collect();
            if !details.is_empty() {
                message["reasoning_details"] = Value::Array(details);
            }
        }
        ReasoningCarrier::CopilotOpaque => {
            if !text.is_empty() {
                message["reasoning_text"] = json!(text);
            }
            if let Some(opaque) = reasoning.of_kind(OpaqueKind::EncryptedReasoning).next() {
                message["reasoning_opaque"] = json!(opaque.data);
            }
        }
        _ => {}
    }
}

/// A `reasoning_content` server in thinking mode rejects a tool-calling message of the
/// current tool loop without the field, as when its reasoning was another model's and
/// went as text, or there was none: such a message gets the one-space placeholder.
pub(super) fn hold_tool_loop(message: &mut Value, carrier: ReasoningCarrier) {
    let calls = message.get("tool_calls").is_some();
    if carrier == ReasoningCarrier::ReasoningContent
        && calls
        && message.get("reasoning_content").is_none()
    {
        message["reasoning_content"] = json!(" ");
    }
}

/// A tool call's own state on the wire: the Gemini thought signature.
pub(super) fn call_extra(opaque: Option<&Opaque>) -> Option<Value> {
    let opaque = opaque.filter(|o| o.kind == OpaqueKind::Signature)?;
    Some(json!({ "google": { "thought_signature": opaque.data } }))
}

/// The thought signature a streamed tool call carries, if any.
pub(super) fn thought_signature(call: &Value) -> Option<Opaque> {
    let signature = call
        .pointer("/extra_content/google/thought_signature")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some(Opaque::new(OpaqueKind::Signature, signature))
}

/// Opaque reasoning state streamed in deltas, closed into items before the first call.
#[derive(Default)]
pub(super) struct Streamed {
    /// `reasoning_details` entries, merged by their `index`.
    details: Vec<Value>,
    /// Copilot's `reasoning_opaque`.
    opaque: String,
}

/// The string fields of a `reasoning_details` entry that stream in pieces.
const STREAMED_FIELDS: &[&str] = &["text", "summary", "data"];

impl Streamed {
    pub fn absorb(&mut self, delta: &Value) {
        for detail in delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.merge_detail(detail);
        }
        if let Some(opaque) = delta.get("reasoning_opaque").and_then(Value::as_str) {
            self.opaque.push_str(opaque);
        }
    }

    /// Concatenate a detail's streamed text onto the entry of the same `index`; its other
    /// fields (type, format, signature) take the latest non-null value.
    fn merge_detail(&mut self, detail: &Value) {
        let Some(fields) = detail.as_object() else {
            return;
        };
        let index = detail
            .get("index")
            .cloned()
            .unwrap_or(json!(self.details.len()));
        let Some(entry) = self
            .details
            .iter_mut()
            .find(|d| d.get("index") == Some(&index))
            .and_then(Value::as_object_mut)
        else {
            let mut entry = fields.clone();
            entry.insert("index".into(), index);
            self.details.push(Value::Object(entry));
            return;
        };
        for (key, value) in fields {
            merge_field(entry, key, value);
        }
    }

    /// The items to close the reasoning streamed so far with.
    pub fn take_items(&mut self) -> Vec<LlmEvent> {
        let mut states = Vec::new();
        if !self.details.is_empty() {
            let details = Value::Array(std::mem::take(&mut self.details));
            states.push(Opaque::new(OpaqueKind::Details, details.to_string()));
        }
        if !self.opaque.is_empty() {
            let data = std::mem::take(&mut self.opaque);
            states.push(Opaque::new(OpaqueKind::EncryptedReasoning, data));
        }
        states
            .into_iter()
            .map(|opaque| LlmEvent::ReasoningItem {
                text: String::new(),
                opaque: Some(opaque),
            })
            .collect()
    }
}

fn merge_field(entry: &mut Map<String, Value>, key: &str, value: &Value) {
    match (entry.get_mut(key), value) {
        (Some(Value::String(old)), Value::String(more)) if STREAMED_FIELDS.contains(&key) => {
            old.push_str(more);
        }
        (_, Value::Null) => {}
        _ => {
            entry.insert(key.into(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_concatenate_by_index_and_keep_their_latest_fields() {
        let mut streamed = Streamed::default();
        for delta in [
            json!({"reasoning_details": [{"type": "reasoning.text", "index": 0, "text": "Look", "signature": null}]}),
            json!({"reasoning_details": [{"type": "reasoning.text", "index": 0, "text": " here", "signature": "s1"}]}),
            json!({"reasoning_details": [{"type": "reasoning.encrypted", "index": 1, "data": "e"}]}),
        ] {
            streamed.absorb(&delta);
        }
        let items = streamed.take_items();
        let [
            LlmEvent::ReasoningItem {
                opaque: Some(opaque),
                ..
            },
        ] = items.as_slice()
        else {
            panic!("one details item");
        };
        let details: Value = serde_json::from_str(&opaque.data).unwrap();
        assert_eq!(
            details,
            json!([
                {"type": "reasoning.text", "index": 0, "text": "Look here", "signature": "s1"},
                {"type": "reasoning.encrypted", "index": 1, "data": "e"},
            ])
        );
        assert!(streamed.take_items().is_empty(), "taken once");
    }
}
