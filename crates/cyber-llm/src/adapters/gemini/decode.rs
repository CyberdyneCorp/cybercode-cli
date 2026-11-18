//! Stream decoding (`provider-catalog` → Gemini API adapter, Gemini thought signatures,
//! Gemini finish reasons and safety blocks, Gemini usage and implicit caching).
//!
//! Each event is a whole `GenerateContentResponse` chunk. A signature can arrive in the
//! last chunk on a part with empty text, so the decoder finishes only at the end of the
//! stream, after `finishReason`.

use serde_json::Value;

use super::errors;
use super::request::LOCAL_ID_PREFIX;
use crate::adapters::{Decoder, ToolCalls, parse_json};
use crate::error::{ErrorKind, LlmError};
use crate::json::u64_at;
use crate::sse::SseEvent;
use crate::types::{FinishReason, LlmEvent, Opaque, OpaqueKind, Usage};

/// Finish reasons of a safety or policy block.
const BLOCKED: &[&str] = &[
    "SAFETY",
    "PROHIBITED_CONTENT",
    "BLOCKLIST",
    "SPII",
    "IMAGE_SAFETY",
    "RECITATION",
];

type Out = Vec<Result<LlmEvent, LlmError>>;

#[derive(Default)]
pub(super) struct GeminiDecoder {
    tools: ToolCalls,
    calls: usize,
    /// Thought summary text since the last reasoning item.
    summary: String,
    /// Text, reasoning or a call was streamed.
    output: bool,
    usage: Option<Usage>,
    finish: Option<String>,
    block: Option<String>,
    /// A stream error was emitted; nothing follows it.
    failed: bool,
}

impl Decoder for GeminiDecoder {
    fn on_event(&mut self, event: SseEvent, out: &mut Out) {
        if self.failed {
            return;
        }
        let chunk = match parse_json(&event) {
            Ok(chunk) => chunk,
            Err(e) => return self.fail(e, out),
        };
        if chunk.get("error").is_some() {
            return self.fail(errors::stream_error(&chunk), out);
        }
        if let Some(reason) = chunk
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
        {
            self.block = Some(reason.into());
        }
        if let Some(usage) = chunk.get("usageMetadata") {
            self.usage = Some(usage_of(usage));
        }
        let Some(candidate) = chunk.pointer("/candidates/0") else {
            return;
        };
        let parts = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array);
        for part in parts.into_iter().flatten() {
            self.part(part, out);
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.finish = Some(reason.into());
        }
    }

    fn on_end(&mut self, out: &mut Out) {
        if self.failed {
            return;
        }
        if let Some(usage) = self.usage {
            out.push(Ok(LlmEvent::Usage(usage)));
        }
        out.push(self.outcome());
    }
}

impl GeminiDecoder {
    fn fail(&mut self, error: LlmError, out: &mut Out) {
        self.failed = true;
        out.push(Err(error));
    }

    fn part(&mut self, part: &Value, out: &mut Out) {
        let signature = part.get("thoughtSignature").and_then(Value::as_str);
        if let Some(call) = part.get("functionCall") {
            return self.call(call, signature, out);
        }
        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
        let thought = part.get("thought").and_then(Value::as_bool) == Some(true);
        if !text.is_empty() {
            self.output = true;
            out.push(Ok(if thought {
                self.summary.push_str(text);
                LlmEvent::ReasoningDelta { text: text.into() }
            } else {
                LlmEvent::TextDelta { text: text.into() }
            }));
        }
        // A signature on any part but a call, empty text included, closes a reasoning
        // item; it goes back as the last part of this model content.
        if let Some(signature) = signature {
            out.push(Ok(LlmEvent::ReasoningItem {
                text: std::mem::take(&mut self.summary),
                opaque: Some(Opaque::new(OpaqueKind::Signature, signature)),
            }));
        }
    }

    /// A whole call: Gemini streams a call's arguments in one part.
    fn call(&mut self, call: &Value, signature: Option<&str>, out: &mut Out) {
        self.calls += 1;
        self.output = true;
        let key = self.calls.to_string();
        let id = match call.get("id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => id.to_string(),
            // Models before Gemini 3 issue no ID; a local one is never sent back.
            _ => cyber_core::ids::new_id(LOCAL_ID_PREFIX.trim_end_matches('_')),
        };
        let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
        self.tools.start(&key, &id, name);
        if let Some(signature) = signature {
            self.tools
                .set_opaque(&key, Opaque::new(OpaqueKind::Signature, signature));
        }
        let args = call
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        let arguments = args.to_string();
        if let Some(done) = self.tools.finish(&key, Some(&arguments)) {
            out.push(Ok(LlmEvent::ToolCallDone(done)));
        }
    }

    /// The finish event, or the error a finish reason stands for (design D8).
    fn outcome(&self) -> Result<LlmEvent, LlmError> {
        let finish = |reason| Ok(LlmEvent::Finish { reason });
        let Some(reason) = self.finish.as_deref() else {
            return Err(match &self.block {
                Some(block) => blocked("prompt", block),
                None => LlmError::transport("stream ended before finishReason"),
            });
        };
        match reason {
            "STOP" if self.tools.any() => finish(FinishReason::ToolCalls),
            "STOP" => finish(FinishReason::Stop),
            "MAX_TOKENS" => finish(FinishReason::Length),
            r if BLOCKED.contains(&r) && self.output => finish(FinishReason::ContentFilter),
            r if BLOCKED.contains(&r) => Err(blocked("response", r)),
            "MALFORMED_FUNCTION_CALL" => Err(LlmError::new(
                ErrorKind::ProviderInternal,
                "Gemini produced a malformed function call",
            )),
            "UNEXPECTED_TOOL_CALL" => Err(LlmError::new(
                ErrorKind::InvalidRequest,
                "Gemini called a tool that was not offered",
            )),
            "MISSING_THOUGHT_SIGNATURE" => Err(errors::missing_signature()),
            other => finish(FinishReason::Other(other.to_ascii_lowercase())),
        }
    }
}

/// A safety or policy block: never retried, never a reason to fall back.
fn blocked(what: &str, reason: &str) -> LlmError {
    LlmError::new(
        ErrorKind::ContentPolicy,
        format!("Gemini blocked the {what}: {reason}"),
    )
}

/// `promptTokenCount` includes cached tokens and `candidatesTokenCount` excludes
/// thoughts; implicit caching has no write charge.
fn usage_of(usage: &Value) -> Usage {
    let cached = u64_at(usage, "/cachedContentTokenCount");
    let prompt = u64_at(usage, "/promptTokenCount") + u64_at(usage, "/toolUsePromptTokenCount");
    Usage {
        input: prompt.saturating_sub(cached),
        output: u64_at(usage, "/candidatesTokenCount"),
        reasoning: u64_at(usage, "/thoughtsTokenCount"),
        cache_read: cached,
        cache_write: 0,
    }
}
