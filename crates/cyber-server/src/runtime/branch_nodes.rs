//! The user-message tree clients show (`server-api` → Session branch routes): one node
//! per user message on any branch, linked to the nearest user message before it. Labels
//! on assistant or system messages show on the user message that started their Turn.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cyber_llm::Content;
use serde::Serialize;

use super::model::{Entry, SessionState};

const PREVIEW_CHARS: usize = 80;

/// A user message in the branch tree (`GET .../tree`).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct BranchNode {
    pub id: String,
    /// The nearest user message before this one; null for a first message.
    pub parent_id: Option<String>,
    /// The first line of the message's text, at most 80 characters.
    pub preview: String,
    /// When the message was promoted (ms since the epoch); null for old records.
    pub created_at: Option<i64>,
    pub labels: Vec<String>,
    /// On the active path.
    pub active: bool,
    /// The end of a branch: no user message follows it, or the active path ends here.
    pub tip: bool,
    /// User messages below this one, on every branch.
    pub descendants: usize,
}

/// The user messages of every branch, oldest first.
pub fn branch_nodes(state: &SessionState) -> Vec<BranchNode> {
    let tree = state.branches();
    let order = tree.ordered();
    let messages = state.messages_by_id();
    let anchors = user_anchors(state, &messages, &order);
    let anchor = |id: &str| anchors.get(id).cloned().flatten();
    let active: BTreeSet<&str> = state.entries.iter().map(Entry::id).collect();
    let active_tip = state.active_tip().and_then(anchor);
    let users: Vec<(&str, Option<String>)> = order
        .iter()
        .filter(|id| anchors.get(**id) == Some(&Some((**id).to_owned())))
        .map(|id| (*id, tree.parent(id).and_then(anchor)))
        .collect();
    let descendants = descendant_counts(&users);
    let mut labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, message) in tree.labels() {
        if let Some(user) = anchor(message) {
            labels.entry(user).or_default().push(name.clone());
        }
    }
    users
        .into_iter()
        .map(|(id, parent_id)| {
            let below = descendants.get(id).copied().unwrap_or_default();
            BranchNode {
                id: id.to_owned(),
                parent_id,
                preview: messages.get(id).map(|e| preview(e)).unwrap_or_default(),
                created_at: messages.get(id).and_then(|e| promoted_ms(e)),
                labels: labels.remove(id).unwrap_or_default(),
                active: active.contains(id),
                tip: below == 0 || active_tip.as_deref() == Some(id),
                descendants: below,
            }
        })
        .collect()
}

/// For every message, the nearest user message at or before it on its path. Parents are
/// created before their children, so one pass in creation order fills the map.
fn user_anchors<'a>(
    state: &SessionState,
    messages: &HashMap<&str, &Entry>,
    order: &[&'a str],
) -> BTreeMap<&'a str, Option<String>> {
    let tree = state.branches();
    let mut anchors: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for id in order {
        let anchor = match messages.get(id) {
            Some(Entry::User { .. }) => Some((*id).to_owned()),
            _ => tree
                .parent(id)
                .and_then(|p| anchors.get(p).cloned().flatten()),
        };
        anchors.insert(id, anchor);
    }
    anchors
}

/// User messages below each user message; children come after parents in `users`.
fn descendant_counts(users: &[(&str, Option<String>)]) -> BTreeMap<String, usize> {
    let mut below: BTreeMap<String, usize> = BTreeMap::new();
    for (id, parent) in users.iter().rev() {
        let own = below.get(*id).copied().unwrap_or_default();
        if let Some(parent) = parent {
            *below.entry(parent.clone()).or_default() += own + 1;
        }
    }
    below
}

fn promoted_ms(entry: &Entry) -> Option<i64> {
    match entry {
        Entry::User { promoted_ms, .. } => *promoted_ms,
        _ => None,
    }
}

/// The first non-empty line of a user message's text, without control characters.
pub fn preview(entry: &Entry) -> String {
    let Entry::User { parts, .. } = entry else {
        return String::new();
    };
    let text = parts.iter().find_map(|part| match part {
        Content::Text { text } => text.lines().find(|l| !l.trim().is_empty()),
        _ => None,
    });
    let line: String = text
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    match line.char_indices().nth(PREVIEW_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}
