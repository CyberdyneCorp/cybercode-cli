//! The goal evaluator (`goals` → Evaluation after each turn, Evaluator cost accounting): a
//! tool-free call on `model_roles.evaluator` (else `small`) that answers one JSON verdict
//! about the active Goal from the latest assistant text, the Turn's tool results and the
//! earlier verdicts, within `goals.evaluator_max_input_tokens`. A malformed reply is
//! retried once, then counts as `not_met`.

use std::time::Duration;

use cyber_core::goals::{MAX_REASON, Verdict};
use cyber_llm::catalog::{ModelRole, compute_cost};
use cyber_llm::{Adapter, FinishReason, LlmEvent, LlmRequest, Message, Usage};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::records::Goal;
use crate::runtime::{Entry, SessionState};
use crate::runtime::{Handle, Inner, RuntimeError};

const TIMEOUT: Duration = Duration::from_secs(60);
/// Characters per token when bounding the input.
const CHARS_PER_TOKEN: usize = 4;
/// Characters of each tool result kept in the evidence.
const TOOL_HEAD: usize = 400;
/// Earlier verdicts shown to the evaluator.
const EARLIER: usize = 5;

const SYSTEM: &str = "You judge whether a coding agent has met a goal. You see the goal's \
condition, the agent's latest answer, a summary of the tools it just ran and your earlier \
verdicts. Judge only from this evidence. Answer with one JSON object and nothing else: \
{\"verdict\": \"met\" | \"not_met\" | \"impossible\", \"reason\": \"<at most 300 characters>\"}. \
Use met only when the evidence shows the condition holds. Use impossible only when the \
condition cannot be reached at all (for example it contradicts itself or needs access the \
agent was refused), not when work remains. For not_met, the reason says concretely what is \
still missing, since the agent reads it to continue.";

/// What one evaluation produced; usage is billed whether or not a verdict was parsed.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Judgement {
    pub reservation_requests: Vec<String>,
    pub verdict: Verdict,
    pub reason: String,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    verdict: Verdict,
    reason: String,
}

/// The evidence for `goal` from the Session's latest Turn, bounded by `max_tokens`.
pub(super) fn evidence(state: &SessionState, goal: &Goal, max_tokens: u64) -> String {
    let budget = usize::try_from(max_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(CHARS_PER_TOKEN);
    let turn = latest_turn(state);
    let answer = turn
        .iter()
        .rev()
        .find_map(|e| match e {
            Entry::Assistant(a) if !a.text.trim().is_empty() => Some(a.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut tools: Vec<serde_json::Value> = turn
        .iter()
        .filter_map(|e| match e {
            Entry::Assistant(a) => Some(&a.calls),
            _ => None,
        })
        .flatten()
        .filter_map(|id| state.calls.get(id))
        .map(|call| {
            let output = call.output.as_deref().unwrap_or_default();
            json!({"tool": call.name, "status": call.status,
                   "output_head": output.chars().take(TOOL_HEAD).collect::<String>()})
        })
        .collect();
    let mut earlier: Vec<String> = goal
        .evaluations
        .iter()
        .rev()
        .take(EARLIER)
        .map(|e| format!("{:?}: {}", e.verdict, e.reason))
        .collect();
    let mut answer = answer;
    loop {
        let text = json!({"condition": goal.condition, "check": goal.check,
            "latest_answer": answer, "tools_run": tools, "earlier_verdicts": earlier})
        .to_string();
        if text.len() <= budget || (earlier.is_empty() && tools.is_empty() && answer.len() < 64) {
            return text;
        }
        // Older material goes first, then tool summaries, then the answer's head.
        if !earlier.is_empty() {
            earlier.pop();
        } else if !tools.is_empty() {
            tools.remove(0);
        } else {
            let keep = answer.len() / 2;
            answer = answer
                .chars()
                .rev()
                .take(keep)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
        }
    }
}

/// The entries since the newest user input that is not a goal prompt or continuation.
fn latest_turn(state: &SessionState) -> Vec<&Entry> {
    let start = state
        .entries
        .iter()
        .rposition(|e| matches!(e, Entry::User { .. }))
        .map_or(0, |i| i + 1);
    state.entries[start..].iter().collect()
}

/// Judge `evidence` on the evaluator model: `model_roles.evaluator`, else the small
/// model, else the Session's own model (`session_model`).
pub(super) async fn judge(
    inner: &Inner,
    handle: &Handle,
    session_model: &str,
    evidence: String,
    cancel: &CancellationToken,
) -> Result<Judgement, RuntimeError> {
    let resolver = inner.resolver.as_ref();
    let reference = resolver
        .role(ModelRole::Evaluator)
        .or_else(|| resolver.role(ModelRole::Small))
        .unwrap_or_else(|| session_model.to_owned());
    if reference.is_empty() {
        return Ok(unjudged("no evaluator model is configured"));
    }
    let Ok(resolved) = resolver.resolve(&reference) else {
        return Ok(unjudged("the configured evaluator is unavailable"));
    };
    let mut usage = Usage::default();
    let mut outcome = Err("not run".to_string());
    let gated =
        crate::runtime::reservations::GatedAdapter::new(inner, handle, &resolved, "goal_evaluator");
    for _ in 0..2 {
        let request = LlmRequest {
            system: vec![SYSTEM.into()],
            messages: vec![Message::user_text(evidence.clone())],
            tools: Vec::new(),
            max_output_tokens: Some(512),
            ..resolved.template.clone()
        };
        outcome = tokio::select! {
            () = cancel.cancelled() => Err("evaluation cancelled".into()),
            result = tokio::time::timeout(TIMEOUT, infer(&gated, request, &mut usage)) => {
                result.unwrap_or_else(|_| Err("the evaluator timed out".into()))
            }
        };
        gated.flush().await?;
        if let Some(error) = gated.take_error() {
            return Err(error);
        }
        if outcome.is_ok() || cancel.is_cancelled() {
            break;
        }
    }
    let receipts = gated.request_ids();
    let tokens = usage.input
        + usage.output
        + usage.reasoning
        + if receipts.is_empty() {
            0
        } else {
            usage.cache_read + usage.cache_write
        };
    let cost = compute_cost(resolved.cost.as_ref(), &usage).unwrap_or(0.0);
    Ok(match outcome {
        Ok(reply) => Judgement {
            reservation_requests: receipts,
            verdict: reply.verdict,
            reason: clip(&reply.reason),
            tokens,
            cost,
        },
        Err(_) => Judgement {
            reservation_requests: receipts,
            tokens,
            cost,
            ..unjudged("evaluator returned an invalid verdict")
        },
    })
}

fn unjudged(reason: &str) -> Judgement {
    Judgement {
        reservation_requests: Vec::new(),
        verdict: Verdict::NotMet,
        reason: reason.into(),
        tokens: 0,
        cost: 0.0,
    }
}

/// Cut a reason to [`MAX_REASON`] characters on one line.
pub(super) fn clip(reason: &str) -> String {
    let flat = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_REASON {
        return flat;
    }
    let mut cut: String = flat.chars().take(MAX_REASON - 1).collect();
    cut.push('…');
    cut
}

async fn infer(
    adapter: &dyn Adapter,
    mut request: LlmRequest,
    usage: &mut Usage,
) -> Result<Reply, String> {
    // Overlay keys must not re-enable tools or widen the bounded output allowance.
    if let Some(body) = request.body.as_object_mut() {
        for key in [
            "tools",
            "tool_choice",
            "max_tokens",
            "max_output_tokens",
            "max_completion_tokens",
        ] {
            body.remove(key);
        }
    }
    let mut stream = adapter
        .stream(request)
        .await
        .map_err(|_| "evaluator request failed".to_string())?;
    let mut text = String::new();
    let mut finish = None;
    while let Some(event) = stream.next().await {
        match event.map_err(|_| "evaluator stream failed".to_string())? {
            LlmEvent::TextDelta { text: delta } => text.push_str(&delta),
            LlmEvent::Usage(u) => usage.add(&u),
            LlmEvent::Finish { reason } => finish = Some(reason),
            LlmEvent::ToolCallDone(_) | LlmEvent::ToolCallDelta { .. } => {
                return Err("the evaluator called a tool".into());
            }
            _ => {}
        }
        if text.len() > 65_536 {
            return Err("evaluator output is too long".into());
        }
    }
    if finish != Some(FinishReason::Stop) {
        return Err("the evaluator did not finish".into());
    }
    parse(&text)
}

/// The JSON verdict, tolerating a fenced code block around it.
fn parse(text: &str) -> Result<Reply, String> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|t| t.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let reply: Reply =
        serde_json::from_str(body).map_err(|_| "malformed verdict JSON".to_string())?;
    if reply.reason.trim().is_empty() {
        return Err("empty reason".into());
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_parse_strictly() {
        let r = parse(r#"{"verdict": "not_met", "reason": "2 tests fail"}"#).unwrap();
        assert_eq!(
            (r.verdict, r.reason.as_str()),
            (Verdict::NotMet, "2 tests fail")
        );
        assert!(parse("```json\n{\"verdict\": \"met\", \"reason\": \"ok\"}\n```").is_ok());
        assert!(parse(r#"{"verdict": "maybe", "reason": "x"}"#).is_err());
        assert!(parse(r#"{"verdict": "met", "reason": "x", "extra": 1}"#).is_err());
        assert!(parse(r#"{"verdict": "met", "reason": " "}"#).is_err());
        assert!(parse("met").is_err());
    }

    #[test]
    fn reasons_are_clipped_to_one_line() {
        assert_eq!(clip("a\n  b"), "a b");
        let long = clip(&"x".repeat(400));
        assert_eq!(long.chars().count(), 300);
        assert!(long.ends_with('…'));
    }
}
