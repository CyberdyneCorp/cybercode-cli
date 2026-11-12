//! Read-only inspection of the Session state used by request assembly.

use cyber_llm::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context_sources::{FileStamp, SourceMetadata, SourceStatus};

/// Files whose current metadata one inspection compares with their recorded stamps.
const STAMP_CHECKS: usize = 512;
use super::{InputStatus, Runtime, SessionState, view};

#[derive(Debug, Serialize, JsonSchema)]
pub struct EffectiveContextSource {
    pub id: String,
    #[serde(flatten)]
    pub metadata: SourceMetadata,
    /// Whether a file this source was read from has changed size or modification time,
    /// or disappeared, since it was read. The change takes effect at the next request
    /// boundary. `null` when the source has no recorded stamps (it is not read from
    /// files, or was recorded by an older version), when some of its files have no
    /// stamp, or when the inspection's check budget is spent. Files added since the read
    /// are not detected.
    pub changed_on_disk: Option<bool>,
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

/// Why a next-request estimate can differ from the request actually sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimateAssumption {
    /// Tool schemas are sized as in the last prepared request; tool visibility or
    /// configuration may have changed since.
    ToolSchemasFromLastRequest,
    /// The last prepared request used another provider or model.
    ModelChanged,
    /// The next turn rebuilds the Context Epoch from fresh observations.
    EpochRebuildPending,
    /// Files behind a source changed on disk and are read again at the next request.
    SourcesChangedOnDisk,
    /// Instructions added only while a turn is assembled (attachments, structured
    /// output) are not included.
    ExcludesTurnInstructions,
}

/// The stored projection plus the tool schemas of the last prepared request: what the
/// next ordinary request is expected to send, with every assumption listed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NextRequestEstimate {
    pub estimated_input_tokens: u64,
    pub estimated_input_utilization: Option<f64>,
    pub assumptions: Vec<EstimateAssumption>,
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
    /// Listed sources with `changed_on_disk` true.
    pub changed_sources: usize,
    /// Expected input of the next ordinary request. Absent until a request has been
    /// prepared in this Session, because tool schemas are otherwise unknown.
    pub next_request: Option<NextRequestEstimate>,
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
            changed_sources: 0,
            next_request: None,
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
                                file_stamps: Default::default(),
                            });
                    EffectiveContextSource {
                        id: id.clone(),
                        metadata,
                        changed_on_disk: None,
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
            let mut budget = STAMP_CHECKS;
            for source in &mut inspection.effective_sources {
                source.changed_on_disk = changed_on_disk(&source.metadata, &mut budget);
            }
            inspection.changed_sources = inspection
                .effective_sources
                .iter()
                .filter(|source| source.changed_on_disk == Some(true))
                .count();
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
        inspection.next_request = next_request(&inspection, estimate, &model);
        inspection
    }
}

fn next_request(
    inspection: &ContextInspection,
    stored: u64,
    model: &super::ResolvedModel,
) -> Option<NextRequestEstimate> {
    let prepared = inspection.last_prepared_request.as_ref()?;
    let tools = prepared
        .sources
        .iter()
        .find(|source| source.id == "tool_schemas")?
        .estimated_tokens;
    let mut assumptions = vec![EstimateAssumption::ToolSchemasFromLastRequest];
    if prepared.provider != model.provider || prepared.model != model.model {
        assumptions.push(EstimateAssumption::ModelChanged);
    }
    if inspection.epoch_pending {
        assumptions.push(EstimateAssumption::EpochRebuildPending);
    }
    if inspection.changed_sources > 0 {
        assumptions.push(EstimateAssumption::SourcesChangedOnDisk);
    }
    assumptions.push(EstimateAssumption::ExcludesTurnInstructions);
    let estimated_input_tokens = stored.saturating_add(tools);
    Some(NextRequestEstimate {
        estimated_input_tokens,
        estimated_input_utilization: inspection
            .window
            .map(|window| estimated_input_tokens as f64 / window as f64),
        assumptions,
    })
}

/// Compare a source's recorded file stamps with current metadata. Only metadata of
/// files the source was already read from is consulted; no file is opened.
fn changed_on_disk(metadata: &SourceMetadata, budget: &mut usize) -> Option<bool> {
    let in_context = matches!(
        metadata.status,
        SourceStatus::Active | SourceStatus::RetainedUnavailable
    );
    let stamps = &metadata.file_stamps;
    if !in_context || stamps.is_empty() || stamps.len() > *budget {
        return None;
    }
    *budget -= stamps.len();
    let changed = stamps.iter().any(|(path, stamp)| {
        std::fs::metadata(path).map_or(true, |current| FileStamp::of(&current) != *stamp)
    });
    let complete = metadata.omitted_file_paths == 0
        && metadata
            .file_paths
            .iter()
            .all(|path| stamps.contains_key(path));
    (changed || complete).then_some(changed)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(dir: &std::path::Path, files: &[&str], stamped: &[&str]) -> SourceMetadata {
        let path = |name: &str| dir.join(name).display().to_string();
        SourceMetadata {
            origin: "instruction_files".into(),
            status: SourceStatus::Active,
            pending_update: false,
            estimated_tokens: Some(1),
            file_paths: files.iter().map(|name| path(name)).collect(),
            omitted_file_paths: 0,
            file_stamps: stamped
                .iter()
                .map(|name| {
                    let stamp = FileStamp::of(&std::fs::metadata(path(name)).unwrap());
                    (path(name), stamp)
                })
                .collect(),
        }
    }

    #[test]
    fn an_unstamped_file_or_spent_budget_leaves_the_answer_unknown() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.md", "b.md"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let mut budget = STAMP_CHECKS;
        let complete = metadata(dir.path(), &["a.md", "b.md"], &["a.md", "b.md"]);
        assert_eq!(changed_on_disk(&complete, &mut budget), Some(false));
        assert_eq!(budget, STAMP_CHECKS - 2);
        // One listed file has no stamp: unchanged stamped files prove nothing about it.
        let partial = metadata(dir.path(), &["a.md", "b.md"], &["a.md"]);
        assert_eq!(changed_on_disk(&partial, &mut budget), None);
        // A detected change is reported even when another file is unstamped.
        std::fs::write(dir.path().join("a.md"), "edited a.md").unwrap();
        assert_eq!(changed_on_disk(&partial, &mut budget), Some(true));
        let mut spent = 1;
        assert_eq!(changed_on_disk(&complete, &mut spent), None);
        assert_eq!(spent, 1);
        let absent = SourceMetadata {
            status: SourceStatus::Absent,
            ..complete
        };
        assert_eq!(changed_on_disk(&absent, &mut budget), None);
    }
}
