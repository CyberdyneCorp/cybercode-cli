//! The model's view of a Session (`session-runtime` → Turn assembly,
//! `compaction` → Model view after compaction).

use cyber_llm::{Content, Message, Role};

use super::model::{AssistantEntry, CallStatus, Entry, SessionState, TaskState};

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
                let parts = if strip_media {
                    strip(parts)
                } else {
                    parts.clone()
                };
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
            Entry::Assistant(a) => push_assistant(state, a, provider, model, &mut out),
        }
    }
    out
}

fn push_assistant(
    state: &SessionState,
    a: &AssistantEntry,
    provider: &str,
    model: &str,
    out: &mut Vec<Message>,
) {
    let mut content = Vec::new();
    if !a.reasoning.is_empty() {
        let native = a.provider == provider && a.model == model;
        content.push(if native {
            Content::Reasoning {
                text: a.reasoning.clone(),
                signature: a.signature.clone(),
            }
        } else {
            Content::Text {
                text: a.reasoning.clone(),
            }
        });
    }
    if !a.text.is_empty() {
        content.push(Content::Text {
            text: a.text.clone(),
        });
    }
    let calls: Vec<_> = a
        .calls
        .iter()
        .filter_map(|id| state.calls.get(id))
        .collect();
    for call in &calls {
        let input = call
            .input
            .clone()
            .unwrap_or_else(|| serde_json::Value::String(call.arguments.clone()));
        content.push(Content::ToolCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            input,
        });
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
}

fn strip(parts: &[Content]) -> Vec<Content> {
    parts
        .iter()
        .map(|p| match p {
            Content::Image { media_type, .. } => Content::Text {
                text: format!("[Attached {media_type}]"),
            },
            other => other.clone(),
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

/// Rough token estimate: 4 characters per token.
pub fn estimate_tokens(
    system: &[String],
    messages: &[Message],
    tools: &[cyber_llm::ToolSpec],
) -> u64 {
    let chars = system.iter().map(String::len).sum::<usize>()
        + serde_json::to_string(messages).map_or(0, |s| s.len())
        + serde_json::to_string(tools).map_or(0, |s| s.len());
    (chars / 4) as u64
}
