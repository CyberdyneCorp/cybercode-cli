//! Collect a stream into the result of one Turn.

use futures::{Stream, StreamExt};

use crate::error::LlmError;
use crate::types::{FinishReason, LlmEvent, ToolCall, Usage};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnOutput {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub finish: Option<FinishReason>,
}

/// Drain a stream, calling `on_event` for each event (for live rendering). A discarded
/// attempt's output is dropped; its usage was spent and stays counted.
pub async fn collect(
    mut stream: impl Stream<Item = Result<LlmEvent, LlmError>> + Unpin,
    mut on_event: impl FnMut(&LlmEvent),
) -> Result<TurnOutput, LlmError> {
    let mut out = TurnOutput::default();
    while let Some(event) = stream.next().await {
        let event = event?;
        on_event(&event);
        match event {
            LlmEvent::TextDelta { text } => out.text.push_str(&text),
            LlmEvent::ReasoningDelta { text } => out.reasoning.push_str(&text),
            // The streamed deltas already hold an item's text unless the adapter sent none.
            LlmEvent::ReasoningItem { text, .. } if out.reasoning.is_empty() => {
                out.reasoning = text;
            }
            LlmEvent::ReasoningItem { .. } => {}
            LlmEvent::ToolCallDone(call) => out.tool_calls.push(call),
            LlmEvent::Usage(usage) => out.usage.add(&usage),
            LlmEvent::Finish { reason } => out.finish = Some(reason),
            LlmEvent::ToolCallDelta { .. } => {}
            LlmEvent::Discarded => {
                out = TurnOutput {
                    usage: out.usage,
                    ..TurnOutput::default()
                };
            }
        }
    }
    Ok(out)
}
