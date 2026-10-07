//! Forked histories are durable observations, with no copied execution or billing ownership.
use super::model::{Compacted, Epoch};
use super::{
    AssistantEntry, CallState, CallStatus, Entry, Runtime, RuntimeError, SessionState, TaskState,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ForkContext {
    pub epoch: Option<Epoch>,
    pub epoch_stale: bool,
    pub compacted: Option<Compacted>,
    pub task: TaskState,
}

pub(super) struct ForkHistory {
    pub entries: Vec<Entry>,
    pub calls: Vec<CallState>,
    pub context: ForkContext,
}

pub(super) fn history(state: &SessionState, cut: usize, context: &SessionState) -> ForkHistory {
    let mut entries = Vec::new();
    let mut calls = Vec::new();
    let mut ids = HashMap::new();
    for entry in &state.entries[..cut] {
        let fresh = cyber_core::ids::new_id("msg");
        ids.insert(entry.id().to_string(), fresh.clone());
        let copy = match entry {
            Entry::User { parts, source, .. } => Entry::User {
                id: fresh,
                parts: parts.clone(),
                source: source.clone(),
            },
            Entry::System { text, epoch, .. } => Entry::System {
                id: fresh,
                text: text.clone(),
                epoch: *epoch,
            },
            Entry::Assistant(a) => {
                let copied = copy_calls(a, state, &fresh);
                let references = copied.iter().map(|call| call.call_id.clone()).collect();
                calls.extend(copied);
                Entry::Assistant(AssistantEntry {
                    id: fresh,
                    calls: references,
                    ..a.clone()
                })
            }
        };
        entries.push(copy);
    }
    let mut task = context.task.clone();
    task.objective = task.objective.and_then(|mut source| {
        source.message_id = ids.get(&source.message_id)?.clone();
        Some(source)
    });
    task.instructions = task
        .instructions
        .into_iter()
        .filter_map(|mut source| {
            source.message_id = ids.get(&source.message_id)?.clone();
            Some(source)
        })
        .collect();
    if task.objective.is_none() {
        task.objective = task.instructions.first().cloned();
    }
    let compacted = context
        .compacted
        .as_ref()
        .filter(|summary| summary.tail_start <= cut)
        .map(|summary| Compacted {
            summary_id: cyber_core::ids::new_id("msg"),
            ..summary.clone()
        });
    ForkHistory {
        entries,
        calls,
        context: ForkContext {
            epoch: context.epoch.clone(),
            epoch_stale: context.epoch_stale
                || (context.compacted.is_some() && compacted.is_none()),
            compacted,
            task,
        },
    }
}

fn copy_calls(answer: &AssistantEntry, state: &SessionState, message: &str) -> Vec<CallState> {
    answer.calls.iter().filter_map(|id| state.calls.get(id)).map(|original| {
        let mut call = original.clone();
        call.call_id = cyber_core::ids::new_id("call");
        call.message_id = message.into();
        if !call.status.is_settled() {
            call.status = CallStatus::Interrupted;
            call.output = Some("Copied observation: source call was still active; it is not executed by this fork".into());
        }
        call
    }).collect()
}

impl Runtime {
    pub(super) fn fork_boundary(
        &self,
        source: &SessionState,
        before: Option<&str>,
    ) -> Result<SessionState, RuntimeError> {
        let Some(before) = before else {
            return Ok(source.clone());
        };
        let events = self.inner.load_events(&source.info.id)?;
        let events: Vec<_> = events
            .into_iter()
            .take_while(|event| event.seq <= source.last_seq)
            .collect();
        let boundary = events.iter().position(|event| {
            matches!(
                event.kind.as_str(),
                super::events::PROMOTED
                    | super::events::STEP_STARTED
                    | super::events::SYSTEM_ADDED
                    | super::events::CONTEXT_UPDATED
            ) && event.data["message_id"].as_str() == Some(before)
        });
        match boundary {
            Some(index) if index > 0 => {
                SessionState::replay(&events[..index]).map_err(RuntimeError::Corrupt)
            }
            _ => Ok(source.clone()),
        }
    }
}
