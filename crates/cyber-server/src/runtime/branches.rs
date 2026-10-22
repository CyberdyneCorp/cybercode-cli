//! Session branches (`session-runtime` → Session branches): the projected messages form a
//! tree rebuilt from events, and `entries` is the active path from the root to its tip.
//! A committed conversation rewind leaves the cut messages as an inactive branch, and
//! `session.branch.checked_out.1` makes another path active. Messages off the active path
//! are stashed with what the projection derived from them, so a checkout restores them.
//! Compaction summaries are kept with the last message they replaced and reused only
//! while that message is on the active path, so a sibling's summary never reaches the model.

use std::collections::{BTreeMap, HashMap};

use cyber_store::EventRegistry;
use serde::{Deserialize, Serialize};

use super::host::FileDiff;
use super::model::{
    CallState, CallStatus, Compacted, Entry, InboxRow, SessionState, Sourced, StepSnapshot,
};

pub const CHECKED_OUT: &str = "session.branch.checked_out.1";
pub const LABELED: &str = "session.branch.labeled.1";
const MAX_LABEL_CHARS: usize = 48;

/// `session.branch.checked_out.1`: the path ending at `tip_message_id` is active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedOut {
    pub tip_message_id: String,
    pub code_restored: bool,
    /// The tree the working tree was restored to, when `code_restored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
}

/// `session.branch.labeled.1`: sets `name` on a message, or clears it when null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labeled {
    pub name: String,
    pub message_id: Option<String>,
}

/// Whether `name` matches `[A-Za-z0-9._-]{1,48}`.
pub fn valid_label(name: &str) -> bool {
    (1..=MAX_LABEL_CHARS).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CHECKED_OUT, |v| {
            serde_json::from_value::<CheckedOut>(v.clone())
                .map(drop)
                .map_err(|e| e.to_string())
        })
        .and_then(|r| {
            r.register_with(LABELED, |v| {
                let labeled: Labeled =
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                if valid_label(&labeled.name) {
                    Ok(())
                } else {
                    Err(format!("invalid branch label {:?}", labeled.name))
                }
            })
        })
        .expect("valid branch events");
}

#[derive(Debug, Clone, PartialEq)]
struct Node {
    parent: Option<String>,
    /// Creation order; the newest tip under a message has the highest.
    order: u64,
}

/// A message off the active path with the state the projection derived from it.
#[derive(Debug, Clone, PartialEq)]
struct Stashed {
    entry: Entry,
    calls: Vec<CallState>,
    step: Option<StepSnapshot>,
    diffs: Option<Vec<FileDiff>>,
    instruction: Option<Sourced>,
    input: Option<InboxRow>,
}

impl Stashed {
    /// Refer to message `from` as `to`: the message itself and what names it.
    fn rename(&mut self, from: &str, to: &str) {
        let renamed = |value: &mut String| {
            if value == from {
                *value = to.to_owned();
            }
        };
        match &mut self.entry {
            Entry::User { id, .. } | Entry::System { id, .. } => renamed(id),
            Entry::Assistant(a) => renamed(&mut a.id),
        }
        for call in &mut self.calls {
            renamed(&mut call.message_id);
        }
        if let Some(step) = &mut self.step {
            renamed(&mut step.step_id);
            if let Some(user) = &mut step.user_message_id {
                renamed(user);
            }
        }
        if let Some(instruction) = &mut self.instruction {
            renamed(&mut instruction.message_id);
        }
        if let Some(input) = &mut self.input {
            renamed(&mut input.message_id);
        }
    }
}

/// A compaction summary and the last message it replaced: it covers the path to that
/// message, so it is reusable exactly while that message is on the active path.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    last_replaced: String,
    compacted: Compacted,
}

/// Every projected message with its parent, the labels and the inactive branches' content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchTree {
    nodes: BTreeMap<String, Node>,
    next_order: u64,
    /// Label name to message ID; names are unique per Session.
    labels: BTreeMap<String, String>,
    stash: BTreeMap<String, Stashed>,
    /// Every compaction summary in the order they were written, on any branch.
    summaries: Vec<Summary>,
}

impl BranchTree {
    fn attach(&mut self, id: &str, parent: Option<&str>) {
        if self.nodes.contains_key(id) {
            return;
        }
        self.nodes.insert(
            id.to_owned(),
            Node {
                parent: parent.map(str::to_owned),
                order: self.next_order,
            },
        );
        self.next_order += 1;
    }

    /// A log written before branches could admit a rewound message ID again (the rewind
    /// dropped its inbox row). Both attempts are kept: the stale one, stashed with what
    /// was derived from it, moves to the derived ID `<id>~<order>`, so the new one is
    /// attached at the current tip.
    fn rekey_stale(&mut self, id: &str) {
        let Some(mut stashed) = self.stash.remove(id) else {
            return;
        };
        let node = self
            .nodes
            .remove(id)
            .expect("a stashed message is in the tree");
        let key = format!("{id}~{}", node.order);
        let renamed = |value: &mut String| {
            if value == id {
                value.clone_from(&key);
            }
        };
        for parent in self.nodes.values_mut().filter_map(|n| n.parent.as_mut()) {
            renamed(parent);
        }
        self.nodes.insert(key.clone(), node);
        stashed.rename(id, &key);
        for other in self.stash.values_mut() {
            other.rename(id, &key);
        }
        self.labels.values_mut().for_each(renamed);
        for summary in &mut self.summaries {
            renamed(&mut summary.last_replaced);
        }
        self.stash.insert(key, stashed);
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// The previous message on the path to `id` (`parent_message_id`).
    pub fn parent(&self, id: &str) -> Option<&str> {
        self.nodes.get(id)?.parent.as_deref()
    }

    /// Message IDs from the root to `id`, inclusive.
    pub fn path_to(&self, id: &str) -> Vec<String> {
        let mut path = Vec::new();
        let mut next = self.nodes.contains_key(id).then(|| id.to_owned());
        while let Some(current) = next {
            next = self.parent(&current).map(str::to_owned);
            path.push(current);
        }
        path.reverse();
        path
    }

    /// Messages with no children, oldest first.
    pub fn tips(&self) -> Vec<&str> {
        let parents: std::collections::BTreeSet<&str> = self
            .nodes
            .values()
            .filter_map(|n| n.parent.as_deref())
            .collect();
        let mut tips: Vec<(&str, u64)> = self
            .nodes
            .iter()
            .filter(|(id, _)| !parents.contains(id.as_str()))
            .map(|(id, node)| (id.as_str(), node.order))
            .collect();
        tips.sort_by_key(|(_, order)| *order);
        tips.into_iter().map(|(id, _)| id).collect()
    }

    /// The most recently created tip under `id` (or `id` itself when it is a tip).
    pub fn newest_tip(&self, id: &str) -> Option<&str> {
        self.tips()
            .into_iter()
            .filter(|tip| self.path_to(tip).iter().any(|m| m == id))
            .max_by_key(|tip| self.nodes[*tip].order)
    }

    /// Label names to the messages they mark.
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    fn label(&mut self, labeled: Labeled) {
        match labeled.message_id {
            Some(message) => self.labels.insert(labeled.name, message),
            None => self.labels.remove(&labeled.name),
        };
    }

    /// A message of an inactive branch.
    pub fn stashed(&self, id: &str) -> Option<&Entry> {
        self.stash.get(id).map(|s| &s.entry)
    }

    /// Every message ID, oldest first.
    pub fn ordered(&self) -> Vec<&str> {
        let mut ids: Vec<(&str, u64)> = self
            .nodes
            .iter()
            .map(|(id, node)| (id.as_str(), node.order))
            .collect();
        ids.sort_by_key(|(_, order)| *order);
        ids.into_iter().map(|(id, _)| id).collect()
    }

    /// A tool call inactive message `message_id` made; it left `calls` with the message.
    pub fn stashed_call(&self, message_id: &str, call_id: &str) -> Option<&CallState> {
        self.stash
            .get(message_id)?
            .calls
            .iter()
            .find(|c| c.call_id == call_id)
    }
}

impl SessionState {
    /// Append a message to the active path; its parent is the current tip.
    pub(super) fn push_entry(&mut self, entry: Entry) {
        self.branches.rekey_stale(entry.id());
        let parent = self.entries.last().map(|e| e.id().to_owned());
        self.branches.attach(entry.id(), parent.as_deref());
        self.entries.push(entry);
    }

    /// Link history a Session was created with (a fork's copy) as one path, with the
    /// summary it was created with.
    pub(super) fn attach_history(&mut self) {
        let ids: Vec<String> = self.entries.iter().map(|e| e.id().to_owned()).collect();
        let mut parent: Option<&str> = None;
        for id in &ids {
            self.branches.attach(id, parent);
            parent = Some(id);
        }
        self.record_summary();
    }

    /// Keep the current summary for the path it replaced (`session.compaction.completed`).
    pub(super) fn record_summary(&mut self) {
        let Some(compacted) = &self.compacted else {
            return;
        };
        let Some(last) = compacted
            .tail_start
            .checked_sub(1)
            .and_then(|i| self.entries.get(i))
        else {
            return;
        };
        self.branches.summaries.push(Summary {
            last_replaced: last.id().to_owned(),
            compacted: compacted.clone(),
        });
    }

    /// Reuse the summary covering the longest ancestor prefix of the active path (the
    /// newest on a tie); with none, the context is assembled from the path's messages.
    fn select_summary(&mut self) {
        let position: BTreeMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id(), i))
            .collect();
        let chosen = self
            .branches
            .summaries
            .iter()
            .enumerate()
            .filter_map(|(order, s)| Some((position.get(s.last_replaced.as_str())? + 1, order, s)))
            .max_by_key(|(tail_start, order, _)| (*tail_start, *order))
            .map(|(tail_start, _, s)| Compacted {
                tail_start,
                ..s.compacted.clone()
            });
        if chosen != self.compacted {
            self.compacted = chosen;
            self.epoch_stale = true;
        }
    }

    pub fn branches(&self) -> &BranchTree {
        &self.branches
    }

    /// Every message on any branch by ID, for resolving many IDs in one pass.
    pub fn messages_by_id(&self) -> HashMap<&str, &Entry> {
        let stashed = self.branches.stash.iter();
        self.entries
            .iter()
            .map(|e| (e.id(), e))
            .chain(stashed.map(|(id, s)| (id.as_str(), &s.entry)))
            .collect()
    }

    /// A message on any branch: the active path's, or a stashed one.
    pub fn message(&self, id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.id() == id)
            .or_else(|| self.branches.stashed(id))
    }

    /// The last message of the active path.
    pub fn active_tip(&self) -> Option<&str> {
        self.entries.last().map(Entry::id)
    }

    pub(super) fn on_labeled(&mut self, labeled: Labeled) {
        self.branches.label(labeled);
    }

    pub(super) fn on_checked_out(&mut self, c: CheckedOut) -> Result<(), String> {
        if !self.branches.contains(&c.tip_message_id) {
            return Err(format!("checkout of unknown message {}", c.tip_message_id));
        }
        self.checkout(&c.tip_message_id);
        Ok(())
    }

    /// Make the path ending at `tip` active: messages leaving the path are stashed and
    /// the path's stashed messages come back with what was derived from them.
    pub(crate) fn checkout(&mut self, tip: &str) {
        let path = self.branches.path_to(tip);
        let common = self
            .entries
            .iter()
            .zip(&path)
            .take_while(|(entry, id)| entry.id() == id.as_str())
            .count();
        self.stash_from(common);
        for id in &path[common..] {
            self.unstash(id);
        }
        self.reorder_derived();
        self.select_summary();
        // Context updates belong to the epoch of the path they were written on.
        self.epoch_stale = true;
    }

    /// Move the active path's entries from `index` on into the stash. Unsettled and
    /// unknown-outcome calls stay in `calls` for recovery even though no entry projects
    /// them. A summary covering a moved entry is no longer reused.
    pub(super) fn stash_from(&mut self, index: usize) {
        if index >= self.entries.len() {
            return;
        }
        let removed: Vec<Entry> = self.entries.drain(index..).collect();
        self.select_summary();
        for entry in removed {
            let stashed = self.take_derived(entry);
            self.branches
                .stash
                .insert(stashed.entry.id().to_owned(), stashed);
        }
        self.result.rewind(&self.calls);
        self.reset_task_objective();
        self.steps_since_input = 0;
        self.open_step = None;
    }

    fn take_derived(&mut self, entry: Entry) -> Stashed {
        let id = entry.id().to_owned();
        let calls: Vec<String> = self
            .calls
            .values()
            .filter(|c| c.message_id == id && !retained_for_recovery(c.status))
            .map(|c| c.call_id.clone())
            .collect();
        let step = self.steps.iter().position(|s| s.step_id == id);
        let instruction = self
            .task
            .instructions
            .iter()
            .position(|i| i.message_id == id);
        let input = self
            .inbox
            .iter()
            .position(|r| r.message_id == id && r.promoted_seq.is_some());
        Stashed {
            calls: calls.iter().filter_map(|c| self.calls.remove(c)).collect(),
            step: step.map(|i| self.steps.remove(i)),
            diffs: self.diffs.remove(&id),
            instruction: instruction.map(|i| self.task.instructions.remove(i)),
            input: input.map(|i| self.inbox.remove(i)),
            entry,
        }
    }

    fn unstash(&mut self, id: &str) {
        let Some(stashed) = self.branches.stash.remove(id) else {
            return;
        };
        for call in stashed.calls {
            self.calls.insert(call.call_id.clone(), call);
        }
        self.steps.extend(stashed.step);
        if let Some(diffs) = stashed.diffs {
            self.diffs.insert(id.to_owned(), diffs);
        }
        self.task.instructions.extend(stashed.instruction);
        self.inbox.extend(stashed.input);
        self.entries.push(stashed.entry);
    }

    /// Steps and instructions follow the active path; the inbox follows admission.
    fn reorder_derived(&mut self) {
        let position: BTreeMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id(), i))
            .collect();
        let at = |id: &str| position.get(id).copied();
        self.steps.sort_by_key(|s| at(&s.step_id));
        self.task.instructions.sort_by_key(|i| at(&i.message_id));
        self.inbox.sort_by_key(|r| r.admitted_seq);
        self.result.rewind(&self.calls);
        self.reset_task_objective();
        self.steps_since_input = 0;
        self.open_step = None;
    }

    /// The objective is the first instruction still on the active path.
    fn reset_task_objective(&mut self) {
        let off_path = self.task.objective.as_ref().is_some_and(|o| {
            !self
                .task
                .instructions
                .iter()
                .any(|i| i.message_id == o.message_id)
        });
        if off_path || self.task.objective.is_none() {
            self.task.objective = self.task.instructions.first().cloned();
        }
        self.task.version += 1;
    }
}

fn retained_for_recovery(status: CallStatus) -> bool {
    matches!(status, CallStatus::OutcomeUnknown | CallStatus::Dispatched)
}

#[cfg(test)]
#[path = "branches_tests.rs"]
mod tests;
