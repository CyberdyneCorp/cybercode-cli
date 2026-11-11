//! Read-only inspection of the Session state used by request assembly.

use cyber_llm::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context_sources::{SourceMetadata, SourceStatus};
use super::{InputStatus, Runtime, SessionState, view};

#[derive(Debug, Serialize, JsonSchema)]
pub struct EffectiveContextSource {
    pub id: String,
    #[serde(flatten)]
    pub metadata: SourceMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSourceEstimate {
    pub id: String,
    pub origin: String,
    pub estimated_tokens: u64,
}

/// The complete input estimate captured from a final assembled provider request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PreparedContextEstimate {
    pub step_id: String,
    pub epoch: u32,
    pub provider: String,
    pub model: String,
    pub window: Option<u64>,
    pub estimated_input_tokens: u64,
    pub estimated_input_utilization: Option<f64>,
    pub sources: Vec<ContextSourceEstimate>,
}

impl PreparedContextEstimate {
    pub(super) fn capture(
        request: &cyber_llm::LlmRequest,
        resolved: &super::ResolvedModel,
        epoch: u32,
        compacted: bool,
        step_id: String,
    ) -> Self {
        let window = (resolved.context_limit > 0).then_some(resolved.context_limit);
        let estimated_input_tokens =
            view::estimate_tokens(&request.system, &request.messages, &request.tools);
        let mut sources = estimates(&request.system[..2], &request.messages, compacted);
        let turn_bytes = request
            .system
            .iter()
            .skip(2)
            .map(String::len)
            .sum::<usize>();
        if turn_bytes > 0 {
            sources.push(source(
                "turn_instructions",
                "request_assembly",
                (turn_bytes / 4) as u64,
            ));
        }
        sources.push(source(
            "tool_schemas",
            "request_assembly",
            serde_json::to_string(&request.tools).map_or(0, |text| (text.len() / 4) as u64),
        ));
        sort_sources(&mut sources);
        Self {
            step_id,
            epoch,
            provider: resolved.provider.clone(),
            model: resolved.model.clone(),
            window,
            estimated_input_tokens,
            estimated_input_utilization: window
                .map(|window| estimated_input_tokens as f64 / window as f64),
            sources,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContextInspection {
    /// Final assembly of the most recently prepared request, including schemas
    /// and turn instructions. Historical events may have no estimate.
    pub last_prepared_request: Option<PreparedContextEstimate>,
    /// Latest recorded source observations, without source content.
    pub effective_sources: Vec<EffectiveContextSource>,
    pub omitted_sources: usize,
    /// Sources in the stored projection, ordered by estimated size.
    pub sources: Vec<ContextSourceEstimate>,
    /// Estimate of the epoch system text and projected messages. Tool schemas
    /// and instructions added only while assembling a turn are excluded.
    pub estimated_input_tokens: Option<u64>,
    pub window: Option<u64>,
    pub estimated_input_utilization: Option<f64>,
    /// Measured prompt tokens of the last finished request, when available.
    pub last_request_tokens: Option<u64>,
    /// The next turn needs to rebuild the epoch.
    pub epoch_pending: bool,
    pub queued_inputs: usize,
    pub held_inputs: usize,
    pub delivered_rule_count: usize,
}

impl Runtime {
    pub fn inspect_context(&self, state: &SessionState) -> ContextInspection {
        let model = self.inner.resolve(&state.info.model).ok();
        let window = model
            .as_ref()
            .and_then(|model| (model.context_limit > 0).then_some(model.context_limit));
        let mut inspection = ContextInspection {
            last_prepared_request: state.prepared_context.clone(),
            effective_sources: Vec::new(),
            omitted_sources: 0,
            sources: Vec::new(),
            estimated_input_tokens: None,
            window,
            estimated_input_utilization: None,
            last_request_tokens: state
                .last_step
                .as_ref()
                .map(|step| step.usage.context_tokens()),
            epoch_pending: model
                .as_ref()
                .map_or(state.epoch_stale || state.epoch.is_none(), |model| {
                    super::context::epoch_pending(state, model)
                }),
            queued_inputs: count_inputs(state, InputStatus::Pending),
            held_inputs: count_inputs(state, InputStatus::Held),
            delivered_rule_count: state
                .epoch
                .as_ref()
                .map_or(0, |epoch| epoch.reminded_rules.len()),
        };
        if let Some(epoch) = &state.epoch {
            let ids: std::collections::BTreeSet<_> =
                epoch.snapshot.keys().chain(epoch.sources.keys()).collect();
            inspection.omitted_sources = ids.len().saturating_sub(256);
            inspection.effective_sources = ids
                .into_iter()
                .map(|id| {
                    let metadata =
                        epoch
                            .sources
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| SourceMetadata {
                                origin: "unknown".into(),
                                status: SourceStatus::Unknown,
                                pending_update: false,
                                estimated_tokens: epoch
                                    .snapshot
                                    .get(id)
                                    .map(|text| (text.len() / 4) as u64),
                                file_paths: Vec::new(),
                                omitted_file_paths: 0,
                            });
                    EffectiveContextSource {
                        id: id.clone(),
                        metadata,
                    }
                })
                .collect();
            inspection.effective_sources.sort_by(|a, b| {
                b.metadata
                    .estimated_tokens
                    .cmp(&a.metadata.estimated_tokens)
                    .then(a.id.cmp(&b.id))
            });
            inspection.effective_sources.truncate(256);
        }
        let (Some(epoch), Some(model)) = (&state.epoch, model) else {
            return inspection;
        };
        let system = super::context::request_system(epoch, model.family);
        let messages = view::messages(state, &view::Replay::of(&model));
        inspection.sources = estimates(&system, &messages, state.compacted.is_some());
        let estimate = view::estimate_tokens(&system, &messages, &[]);
        inspection.estimated_input_tokens = Some(estimate);
        inspection.estimated_input_utilization =
            window.map(|window| estimate as f64 / window as f64);
        inspection
    }
}

fn count_inputs(state: &SessionState, status: InputStatus) -> usize {
    state
        .inbox
        .iter()
        .filter(|input| input.status == status)
        .count()
}

fn estimates(
    system: &[String],
    messages: &[Message],
    compacted: bool,
) -> Vec<ContextSourceEstimate> {
    let mut sources = vec![
        source(
            "system_prefix",
            "context_epoch",
            (system[0].len() / 4) as u64,
        ),
        source("baseline", "context_epoch", (system[1].len() / 4) as u64),
    ];
    let history = if compacted && let Some((summary, tail)) = messages.split_first() {
        sources.push(source(
            "summary_and_task",
            "compaction",
            view::estimate_tokens(&[], std::slice::from_ref(summary), &[]),
        ));
        tail
    } else {
        messages
    };
    if !history.is_empty() {
        sources.push(source(
            "history",
            "session_replay",
            view::estimate_tokens(&[], history, &[]),
        ));
    }
    sort_sources(&mut sources);
    sources
}

fn sort_sources(sources: &mut [ContextSourceEstimate]) {
    sources.sort_by(|a, b| {
        b.estimated_tokens
            .cmp(&a.estimated_tokens)
            .then(a.id.cmp(&b.id))
    });
}

fn source(id: &str, origin: &str, estimated_tokens: u64) -> ContextSourceEstimate {
    ContextSourceEstimate {
        id: id.into(),
        origin: origin.into(),
        estimated_tokens,
    }
}
