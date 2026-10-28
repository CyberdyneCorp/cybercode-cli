//! The model's view of a Session (`session-runtime` → Turn assembly,
//! `compaction` → Model view after compaction).

use cyber_llm::{Content, Message, Opaque, Role};

use super::model::{AssistantEntry, CallState, CallStatus, Entry, SessionState, TaskState};

pub const INTERRUPTED: &str = "[Tool execution was interrupted]";
pub const UNKNOWN: &str = "[Tool outcome unknown; reconcile before retrying]";

/// Project history for a Turn on `provider`/`model`.
pub fn messages(state: &SessionState, provider: &str, model: &str) -> Vec<Message> {
    let mut out = Vec::new();
    let start = state.compacted.as_ref().map_or(0, |c| c.tail_start);
    if let Some(c) = &state.compacted {
        out.push(Message::user_text(summary_text(&c.summary, &state.task)));
    }
    let strip_media = state.compacted.as_ref().is_some_and(|c| c.strip_media);
    let epoch = state.epoch.as_ref().map(|e| e.number);
    for entry in &state.entries[start.min(state.entries.len())..] {
        match entry {
            Entry::User { parts, .. } => {
                let parts: Vec<Content> = parts
                    .iter()
                    .filter(|part| !is_model_part(part))
                    .cloned()
                    .collect();
                let parts = if strip_media { strip(&parts) } else { parts };
                out.push(Message {
                    role: Role::User,
                    content: parts,
                });
            }
            Entry::System { text, epoch: e, .. } => {
                // Context updates from replaced epochs are no longer projected.
                if e.is_none() || *e == epoch {
                    out.push(Message::user_text(format!(
                        "<system-reminder>\n{text}\n</system-reminder>"
                    )));
                }
            }
            Entry::Assistant(a) => push_assistant(state, a, provider, model, strip_media, &mut out),
        }
    }
    out
}

fn push_assistant(
    state: &SessionState,
    a: &AssistantEntry,
    provider: &str,
    model: &str,
    strip_media: bool,
    out: &mut Vec<Message>,
) {
    let calls: Vec<_> = a
        .calls
        .iter()
        .filter_map(|id| state.calls.get(id))
        .collect();
    let mut content = Vec::new();
    push_reasoning(a, 0, calls.len(), (provider, model), &mut content);
    if !a.text.is_empty() {
        content.push(Content::Text {
            text: a.text.clone(),
        });
    }
    for (index, call) in calls.iter().enumerate() {
        content.push(tool_call(call, provider, model));
        push_reasoning(a, index + 1, calls.len(), (provider, model), &mut content);
    }
    if content.is_empty() {
        return;
    }
    out.push(Message {
        role: Role::Assistant,
        content,
    });
    if calls.is_empty() {
        return;
    }
    let results = calls
        .iter()
        .map(|c| {
            let (output, is_error) = match c.status {
                CallStatus::Ok => (c.output.clone().unwrap_or_default(), false),
                CallStatus::Error => (c.output.clone().unwrap_or_default(), true),
                CallStatus::OutcomeUnknown => (UNKNOWN.to_string(), true),
                CallStatus::Interrupted | CallStatus::Called | CallStatus::Dispatched => {
                    (INTERRUPTED.to_string(), true)
                }
            };
            Content::ToolResult {
                call_id: c.call_id.clone(),
                output,
                is_error,
            }
        })
        .collect();
    out.push(Message {
        role: Role::User,
        content: results,
    });
    if !strip_media {
        let images = calls
            .iter()
            .filter_map(|call| {
                if call.name != "computer" || call.status != CallStatus::Ok {
                    return None;
                }
                let frame: super::computer::FrameInfo =
                    serde_json::from_value(call.structured_output.as_ref()?.get("frame")?.clone())
                        .ok()?;
                Some(Content::Attachment(cyber_llm::AttachmentRef {
                    sha256: frame.sha256,
                    media_type: "image/png".into(),
                    name: Some(format!("computer:{}", frame.id)),
                    size: frame.bytes,
                    width: Some(frame.target.image_width),
                    height: Some(frame.target.image_height),
                    pages: None,
                    original: None,
                }))
            })
            .collect::<Vec<_>>();
        if !images.is_empty() {
            out.push(Message {
                role: Role::User,
                content: images,
            });
        }
    }
}

/// The reasoning items the provider produced after `position` of the answer's calls, in
/// stream order. Items of the Turn's own model are sent natively with their opaque state;
/// any other model's are lowered to text, and an item without text is dropped.
fn push_reasoning(
    a: &AssistantEntry,
    position: usize,
    calls: usize,
    (provider, model): (&str, &str),
    out: &mut Vec<Content>,
) {
    let native = a.provider == provider && a.model == model;
    let items = a
        .reasoning_items
        .iter()
        .filter(|item| item.after_calls.min(calls) == position);
    for item in items {
        let opaque = item
            .opaque
            .clone()
            .filter(|opaque| opaque.is_from(provider, model));
        if native && (opaque.is_some() || !item.text.is_empty()) {
            out.push(Content::Reasoning {
                text: item.text.clone(),
                opaque,
            });
        } else if !native && !item.text.is_empty() {
            out.push(Content::Text {
                text: item.text.clone(),
            });
        }
    }
}

fn tool_call(call: &CallState, provider: &str, model: &str) -> Content {
    let input = call
        .input
        .clone()
        .unwrap_or_else(|| serde_json::Value::String(call.arguments.clone()));
    Content::ToolCall {
        id: call.call_id.clone(),
        name: call.name.clone(),
        input,
        opaque: call
            .opaque
            .clone()
            .filter(|opaque| opaque.is_from(provider, model)),
    }
}

/// Reasoning and tool calls are the model's own output, with provider state only the
/// model's answers carry; a user entry never replays them.
pub(super) fn is_model_part(part: &Content) -> bool {
    matches!(part, Content::Reasoning { .. } | Content::ToolCall { .. })
}

fn strip(parts: &[Content]) -> Vec<Content> {
    parts
        .iter()
        .map(|p| match super::attachments::stripped(p) {
            Some(text) => Content::Text { text },
            None => p.clone(),
        })
        .collect()
}

/// The summary message carries the durable task state separately from the prose.
fn summary_text(summary: &str, task: &TaskState) -> String {
    let mut text = format!("Summary of the earlier part of this session:\n\n{summary}");
    if !task.instructions.is_empty() {
        text.push_str("\n\n<task-state>\nUser instructions, verbatim with source IDs. They stay in force until the user supersedes them:\n");
        for i in &task.instructions {
            text.push_str(&format!("- [{}] {}\n", i.message_id, i.text));
        }
        text.push_str("</task-state>");
    }
    text
}

/// Rough token estimate: 4 characters per token, and 3 tokens per 4 bytes of opaque
/// provider state, which does not compress like text.
pub fn estimate_tokens(
    system: &[String],
    messages: &[Message],
    tools: &[cyber_llm::ToolSpec],
) -> u64 {
    let opaque: usize = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(Content::opaque)
        .map(|opaque| opaque.data.len())
        .sum();
    let chars = system.iter().map(String::len).sum::<usize>()
        + serde_json::to_string(messages).map_or(0, |s| s.len())
        + serde_json::to_string(tools).map_or(0, |s| s.len());
    (chars.saturating_sub(opaque) / 4) as u64 + Opaque::tokens(opaque)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_llm::OpaqueKind;

    fn assistant(content: Vec<Content>) -> Message {
        Message {
            role: Role::Assistant,
            content,
        }
    }

    #[test]
    fn opaque_bytes_count_three_tokens_per_four() {
        let text_only = assistant(vec![Content::Reasoning {
            text: "plan".into(),
            opaque: None,
        }]);
        let base = estimate_tokens(&[], std::slice::from_ref(&text_only), &[]);
        let blob = "A".repeat(4000);
        let with_blob = assistant(vec![Content::Reasoning {
            text: "plan".into(),
            opaque: Some(Opaque::new(OpaqueKind::EncryptedReasoning, blob)),
        }]);
        let estimate = estimate_tokens(&[], &[with_blob], &[]);
        // The blob adds 3000 tokens, plus a few for the `opaque` object's other fields.
        let added = estimate - base;
        assert!((3000..3020).contains(&added), "added {added} tokens");
    }
}
