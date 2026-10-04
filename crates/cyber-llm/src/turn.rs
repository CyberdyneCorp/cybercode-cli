//! Collect a stream into the result of one Turn.

use futures::StreamExt;

use crate::adapters::EventStream;
use crate::error::LlmError;
use crate::types::{FinishReason, LlmEvent, ToolCall, Usage};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnOutput {
    pub text: String,
    pub reasoning: String,
    pub reasoning_signature: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub finish: Option<FinishReason>,
}

/// Drain a stream, calling `on_event` for each event (for live rendering).
pub async fn collect(
    mut stream: EventStream,
    mut on_event: impl FnMut(&LlmEvent),
) -> Result<TurnOutput, LlmError> {
    let mut out = TurnOutput::default();
    while let Some(event) = stream.next().await {
        let event = event?;
        on_event(&event);
        match event {
            LlmEvent::TextDelta { text } => out.text.push_str(&text),
            LlmEvent::ReasoningDelta { text } => out.reasoning.push_str(&text),
            LlmEvent::ReasoningSignature { signature } => out.reasoning_signature = Some(signature),
            LlmEvent::ToolCallDone(call) => out.tool_calls.push(call),
            LlmEvent::Usage(usage) => out.usage.add(&usage),
            LlmEvent::Finish { reason } => out.finish = Some(reason),
            LlmEvent::ToolCallDelta { .. } => {}
        }
    }
    Ok(out)
}
