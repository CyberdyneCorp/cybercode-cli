//! The model's view of a Session (`session-runtime` → Turn assembly,
//! `compaction` → Model view after compaction).

use cyber_llm::catalog::profile::{ReasoningCarrier, ReasoningReplay};
use cyber_llm::{Content, LlmRequest, Message, Opaque, Reasoning, Role};

use super::host::ResolvedModel;
use super::model::{
    AssistantEntry, CallState, CallStatus, Entry, ReasoningBlock, SessionState, TaskState,
};

pub const INTERRUPTED: &str = "[Tool execution was interrupted]";
pub const UNKNOWN: &str = "[Tool outcome unknown; reconcile before retrying]";

/// The model a view is projected for, and how much reasoning it gets back natively
/// (`session-runtime` → Reasoning replay policy).
#[derive(Debug, Clone, Copy)]
pub struct Replay<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub policy: ReasoningReplay,
}

impl<'a> Replay<'a> {
    pub fn of(resolved: &'a ResolvedModel) -> Self {
        Self {
            provider: &resolved.provider,
            model: &resolved.model,
            policy: resolved.profile.reasoning_replay,
        }
    }
}

/// What an answer's reasoning becomes in the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Treatment {
    /// Items of the Turn's model keep their opaque state; others are lowered.
    Native,
    /// Every item with text becomes assistant text.
    Lowered,
    /// The reasoning is left out.
    Dropped,
}

/// Project history for a Turn on `target`.
pub fn messages(state: &SessionState, target: &Replay<'_>) -> Vec<Message> {
    let mut out = Vec::new();
    let start = state.compacted.as_ref().map_or(0, |c| c.tail_start);
    // A compaction summary is plain text: it carries no opaque state.
    if let Some(c) = &state.compacted {
        out.push(Message::user_text(summary_text(&c.summary, &state.task)));
    }
    let strip_media = state.compacted.as_ref().is_some_and(|c| c.strip_media);
    let epoch = state.epoch.as_ref().map(|e| e.number);
    let entries = &state.entries[start.min(state.entries.len())..];
    let latest_user = entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::User { .. }));
    for (index, entry) in entries.iter().enumerate() {
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
            Entry::Assistant(a) => {
                let in_loop = latest_user.is_none_or(|user| index > user);
                let view = AnswerView {
                    target,
                    treatment: treatment(target.policy, in_loop),
                };
                push_assistant(state, a, view, strip_media, &mut out);
            }
        }
    }
    out
}

/// `tool_loop` replays the answers after the latest user message and drops older
/// reasoning, which those providers ignore or count twice.
fn treatment(policy: ReasoningReplay, in_loop: bool) -> Treatment {
    match policy {
        ReasoningReplay::Never => Treatment::Lowered,
        ReasoningReplay::ToolLoop if !in_loop => Treatment::Dropped,
        ReasoningReplay::ToolLoop | ReasoningReplay::Always => Treatment::Native,
    }
}

#[derive(Clone, Copy)]
struct AnswerView<'a> {
    target: &'a Replay<'a>,
    treatment: Treatment,
}

fn push_assistant(
    state: &SessionState,
    a: &AssistantEntry,
    view: AnswerView<'_>,
    strip_media: bool,
    out: &mut Vec<Message>,
) {
    let calls: Vec<_> = a
        .calls
        .iter()
        .filter_map(|id| state.calls.get(id))
        .collect();
    let mut content = Vec::new();
    push_reasoning(a, 0, calls.len(), view, &mut content);
    if !a.text.is_empty() {
        content.push(Content::Text {
            text: a.text.clone(),
        });
    }
    for (index, call) in calls.iter().enumerate() {
        content.push(tool_call(call, view));
        push_reasoning(a, index + 1, calls.len(), view, &mut content);
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
/// stream order. An item goes back natively only when the answer is replayed natively and
/// the Turn's own model produced it; otherwise its text is lowered, and an item without
/// text (a redacted block) is dropped.
fn push_reasoning(
    a: &AssistantEntry,
    position: usize,
    calls: usize,
    view: AnswerView<'_>,
    out: &mut Vec<Content>,
) {
    if view.treatment == Treatment::Dropped {
        return;
    }
    let items = a
        .reasoning_items
        .iter()
        .filter(|item| item.after_calls.min(calls) == position);
    for item in items {
        let native = view.treatment == Treatment::Native && produced_by(a, item, view.target);
        if native && (item.opaque.is_some() || !item.text.is_empty()) {
            out.push(Content::Reasoning {
                text: item.text.clone(),
                opaque: item.opaque.clone(),
            });
        } else if !native && !item.text.is_empty() {
            out.push(Content::Text {
                text: item.text.clone(),
            });
        }
    }
}

/// Whether the Turn's model produced `item`: its opaque state names its producer, and an
/// item without state belongs to the answer's model.
fn produced_by(a: &AssistantEntry, item: &ReasoningBlock, target: &Replay<'_>) -> bool {
    match &item.opaque {
        Some(opaque) => opaque.is_from(target.provider, target.model),
        None => a.provider == target.provider && a.model == target.model,
    }
}

/// `session-runtime` → Reasoning replay policy: an Anthropic request with thinking on
/// must continue a tool loop from a native thinking block. When the answer it continues
/// has none (another model made it, or it answered a request sent with thinking off),
/// thinking is off for this request, and so for the rest of the loop. Returns whether
/// it turned thinking off. `carrier` is the profile's, which names the protocol
/// even when nothing is replayed.
pub fn thinking_guard(request: &mut LlmRequest, carrier: ReasoningCarrier) -> bool {
    let thinking = matches!(
        request.reasoning,
        Some(Reasoning::Effort(_) | Reasoning::BudgetTokens(_))
    );
    if carrier != ReasoningCarrier::Anthropic || !thinking {
        return false;
    }
    // The answer being continued, and what followed it: its results, and any images
    // the results attached, all in user messages.
    let Some(at) = request
        .messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
    else {
        return false;
    };
    let continues_loop = request.messages[at + 1..]
        .iter()
        .flat_map(|message| &message.content)
        .any(|part| matches!(part, Content::ToolResult { .. }));
    let native = request.messages[at].content.iter().any(|part| {
        matches!(
            part,
            Content::Reasoning {
                opaque: Some(_),
                ..
            }
        )
    });
    if !continues_loop || native {
        return false;
    }
    request.reasoning = Some(Reasoning::Off);
    true
}

/// A call keeps its own opaque state (a thought signature) only when its answer is
/// replayed natively to the model that produced it.
fn tool_call(call: &CallState, view: AnswerView<'_>) -> Content {
    let input = call
        .input
        .clone()
        .unwrap_or_else(|| serde_json::Value::String(call.arguments.clone()));
    let target = view.target;
    Content::ToolCall {
        id: call.call_id.clone(),
        name: call.name.clone(),
        input,
        opaque: call.opaque.clone().filter(|opaque| {
            view.treatment == Treatment::Native && opaque.is_from(target.provider, target.model)
        }),
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

    fn continued_loop(answer: Vec<Content>) -> LlmRequest {
        LlmRequest {
            reasoning: Some(Reasoning::BudgetTokens(1024)),
            messages: vec![
                Message::user_text("go"),
                assistant(answer),
                Message {
                    role: Role::User,
                    content: vec![Content::ToolResult {
                        call_id: "c1".into(),
                        output: "ok".into(),
                        is_error: false,
                    }],
                },
                // A computer call's screenshot follows its result in a user message.
                Message {
                    role: Role::User,
                    content: vec![Content::Image {
                        media_type: "image/png".into(),
                        data: "AA==".into(),
                    }],
                },
            ],
            ..LlmRequest::default()
        }
    }

    fn call() -> Content {
        Content::ToolCall {
            id: "c1".into(),
            name: "computer".into(),
            input: serde_json::json!({}),
            opaque: None,
        }
    }

    #[test]
    fn thinking_is_off_only_where_the_continued_answer_has_no_native_block() {
        let lowered = vec![
            Content::Text {
                text: "other model's plan".into(),
            },
            call(),
        ];
        let mut request = continued_loop(lowered.clone());
        assert!(thinking_guard(&mut request, ReasoningCarrier::Anthropic));
        assert_eq!(request.reasoning, Some(Reasoning::Off));

        let signed = vec![
            Content::Reasoning {
                text: "plan".into(),
                opaque: Some(Opaque::new(OpaqueKind::Signature, "sig")),
            },
            call(),
        ];
        let mut request = continued_loop(signed);
        assert!(!thinking_guard(&mut request, ReasoningCarrier::Anthropic));
        let mut other_protocol = continued_loop(lowered.clone());
        assert!(!thinking_guard(
            &mut other_protocol,
            ReasoningCarrier::Responses
        ));
        let mut thinking_off = continued_loop(lowered.clone());
        thinking_off.reasoning = None;
        assert!(!thinking_guard(
            &mut thinking_off,
            ReasoningCarrier::Anthropic
        ));
        let mut new_prompt = continued_loop(lowered);
        new_prompt.messages.truncate(2);
        new_prompt.messages.push(Message::user_text("next"));
        assert!(!thinking_guard(
            &mut new_prompt,
            ReasoningCarrier::Anthropic
        ));
    }
}
