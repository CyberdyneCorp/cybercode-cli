//! The branch projection folded from hand-written event logs: a log written before
//! branches existed replays into a tree with the active path older builds showed.

use cyber_store::StoredEvent;
use serde_json::{Value, json};

use super::*;
use crate::runtime::model::SessionInfo;

/// A log of user messages, each answered by one step, with optional rewinds.
struct Log {
    events: Vec<StoredEvent>,
}

impl Log {
    fn new() -> Self {
        Self { events: Vec::new() }
    }

    fn push(&mut self, kind: &str, data: Value) -> &mut Self {
        let seq = self.events.len() as i64 + 1;
        self.events.push(StoredEvent {
            id: format!("evt_{seq}"),
            aggregate_id: "ses_t".into(),
            seq,
            kind: kind.into(),
            data,
            time_ms: 1_700_000_000_000 + seq,
            causation_id: None,
        });
        self
    }

    /// User message `id` and the step `<id>_reply` answering it.
    fn turn(&mut self, id: &str) -> &mut Self {
        self.attempt(id, &format!("{id}_reply"), id)
    }

    /// User message `id` with `text`, and the step `reply` answering it.
    fn attempt(&mut self, id: &str, reply: &str, text: &str) -> &mut Self {
        self.push(
            "session.prompt.admitted.1",
            json!({ "message_id": id, "parts": [{ "type": "text", "text": text }],
                "delivery": "queue", "source": "user", "digest": format!("sha256:{text}") }),
        )
        .push("session.prompt.promoted.1", json!({ "message_id": id }))
        .push(
            "session.step.started.1",
            json!({ "message_id": reply, "provider": "test", "model": "main",
                "tools": false, "snapshot": format!("tree_pre_{reply}") }),
        )
        .push(
            "snapshot.taken.1",
            json!({ "session_id": "ses_t", "step_id": reply,
                "tree": format!("tree_{reply}"), "changed": [] }),
        )
        .push(
            "session.step.ended.1",
            json!({ "message_id": reply, "finish": "stop",
                "usage": { "input": 1, "output": 1, "reasoning": 0, "cache_read": 0, "cache_write": 0 }, "cost": null }),
        )
    }

    /// A conversation rewind to `id`, committed (the shape every build has written).
    fn rewind(&mut self, id: &str) -> &mut Self {
        for phase in ["stage", "commit"] {
            self.push(
                "session.reverted.1",
                json!({ "session_id": "ses_t", "message_id": id,
                    "target": "conversation", "phase": phase }),
            );
        }
        self
    }

    fn state(&self) -> SessionState {
        let info: SessionInfo = serde_json::from_value(json!({
            "id": "ses_t", "title": "t", "directory": "/r", "parent_id": null,
            "agent": "build", "model": "test/main", "mode": "default", "created_ms": 1,
        }))
        .unwrap();
        let mut state = SessionState::new(info);
        for event in &self.events {
            state.apply(event).unwrap();
        }
        state
    }
}

fn path(state: &SessionState) -> Vec<&str> {
    state.entries.iter().map(Entry::id).collect()
}

/// The `session-runtime` scenario "Rewind keeps the old attempt", and task 1.1's replay
/// of a log written before branches: the same active path, plus the cut branch.
#[test]
fn a_rewind_written_before_branches_replays_into_two_branches() {
    let mut log = Log::new();
    log.turn("m1")
        .turn("m2")
        .turn("m3")
        .rewind("m2")
        .turn("m2b");
    let state = log.state();
    assert_eq!(path(&state), ["m1", "m1_reply", "m2b", "m2b_reply"]);
    assert_eq!(state.task.instructions.len(), 2);
    assert_eq!(state.steps.len(), 2);
    let tree = state.branches();
    assert_eq!(tree.tips(), ["m3_reply", "m2b_reply"]);
    assert_eq!(tree.parent("m2"), Some("m1_reply"));
    assert_eq!(tree.parent("m2b"), Some("m1_reply"), "m2b is m2's sibling");
    assert_eq!(tree.stashed("m3").map(Entry::id), Some("m3"));
    assert_eq!(state.active_tip(), Some("m2b_reply"));
}

/// The `session-runtime` scenario "Return to an earlier attempt": the stashed messages
/// and what was derived from them come back in path order; the other branch leaves.
#[test]
fn checkout_restores_the_branch_and_stashes_the_other() {
    let mut log = Log::new();
    log.turn("m1")
        .turn("m2")
        .turn("m3")
        .rewind("m2")
        .turn("m2b");
    log.push(
        "session.branch.checked_out.1",
        json!({ "tip_message_id": "m3_reply", "code_restored": false }),
    );
    let state = log.state();
    assert_eq!(
        path(&state),
        ["m1", "m1_reply", "m2", "m2_reply", "m3", "m3_reply"]
    );
    let steps: Vec<&str> = state.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(steps, ["m1_reply", "m2_reply", "m3_reply"]);
    let instructions: Vec<&str> = state
        .task
        .instructions
        .iter()
        .map(|i| i.message_id.as_str())
        .collect();
    assert_eq!(instructions, ["m1", "m2", "m3"]);
    assert_eq!(state.task.objective.as_ref().unwrap().message_id, "m1");
    assert!(state.input("m2").is_some() && state.input("m2b").is_none());
    assert!(state.epoch_stale, "the next Turn renders a fresh baseline");
    assert_eq!(state.branches().stashed("m2b").map(Entry::id), Some("m2b"));
}

/// Rewinding to the first message leaves an empty active path; the next prompt is a
/// second root.
#[test]
fn a_rewind_to_the_first_message_starts_a_new_root() {
    let mut log = Log::new();
    log.turn("m1").rewind("m1").turn("n1");
    let state = log.state();
    assert_eq!(path(&state), ["n1", "n1_reply"]);
    assert_eq!(state.branches().parent("n1"), None);
    assert_eq!(state.branches().tips(), ["m1_reply", "n1_reply"]);
}

/// `exact` keeps a message with descendants as the tip; the newest tip under a message
/// is the one created last.
#[test]
fn newest_tip_and_exact_checkout() {
    let mut log = Log::new();
    log.turn("m1")
        .turn("m2")
        .rewind("m2")
        .turn("m2b")
        .rewind("m2b")
        .turn("m2c");
    let state = log.state();
    let tree = state.branches();
    assert_eq!(tree.newest_tip("m1"), Some("m2c_reply"));
    assert_eq!(tree.newest_tip("m2"), Some("m2_reply"));
    log.push(
        "session.branch.checked_out.1",
        json!({ "tip_message_id": "m1", "code_restored": false }),
    );
    let state = log.state();
    assert_eq!(path(&state), ["m1"]);
    assert_eq!(state.active_tip(), Some("m1"));
}

/// Labels are set and cleared by `session.branch.labeled.1`; a checkout of an unknown
/// message is a corrupt log, not a silent no-op.
#[test]
fn labels_fold_and_unknown_checkouts_fail() {
    let mut log = Log::new();
    log.turn("m1")
        .push(
            "session.branch.labeled.1",
            json!({ "name": "fix-a", "message_id": "m1" }),
        )
        .push(
            "session.branch.labeled.1",
            json!({ "name": "gone", "message_id": "m1" }),
        )
        .push(
            "session.branch.labeled.1",
            json!({ "name": "gone", "message_id": null }),
        );
    let state = log.state();
    let labels: Vec<(&str, &str)> = state
        .branches()
        .labels()
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(labels, [("fix-a", "m1")]);
    let info = state.info.clone();
    log.push(
        "session.branch.checked_out.1",
        json!({ "tip_message_id": "nope", "code_restored": false }),
    );
    let mut state = SessionState::new(info);
    let failed = log.events.iter().try_for_each(|e| state.apply(e));
    assert!(failed.unwrap_err().contains("unknown message nope"));
}

#[test]
fn label_grammar() {
    for good in ["fix-a", "v1.2_rc", &"x".repeat(48)] {
        assert!(valid_label(good), "{good}");
    }
    for bad in ["", "has space", "ü", "a/b", &"x".repeat(49)] {
        assert!(!valid_label(bad), "{bad}");
    }
}

fn summarized(log: &mut Log, id: &str, summary: &str, tail_start_id: &str) {
    log.push(
        "session.compaction.completed.1",
        json!({ "message_id": id, "summary": summary, "tail_start_id": tail_start_id,
            "tokens_before": 10, "tokens_after": 2, "trigger": "manual" }),
    );
}

fn checked_out(log: &mut Log, tip: &str) -> SessionState {
    log.push(
        "session.branch.checked_out.1",
        json!({ "tip_message_id": tip, "code_restored": false }),
    );
    log.state()
}

/// Task 2.1: a summary is reused only while the messages it replaced are an ancestor
/// prefix of the active path. Leaving them drops it (the epoch goes stale), and
/// returning to its branch brings it back at the right tail.
#[test]
fn a_summary_is_reused_only_on_the_path_it_replaced() {
    let mut log = Log::new();
    log.turn("m1")
        .turn("m2")
        .rewind("m2")
        .turn("m2b")
        .turn("m3b");
    summarized(&mut log, "sum_b", "SECRET-B", "m3b");
    assert!(log.state().compacted.is_some());
    let state = checked_out(&mut log, "m3b_reply");
    assert_eq!(
        state.compacted.map(|c| c.tail_start),
        Some(4),
        "the summary is of this path"
    );

    let state = checked_out(&mut log, "m2_reply");
    assert!(state.compacted.is_none());
    assert!(state.epoch_stale);
    assert_eq!(path(&state), ["m1", "m1_reply", "m2", "m2_reply"]);

    let state = checked_out(&mut log, "m3b_reply");
    let compacted = state.compacted.unwrap();
    assert_eq!(
        (compacted.summary.as_str(), compacted.tail_start),
        ("SECRET-B", 4)
    );
}

/// Of the summaries on the active path the one covering the longest prefix is reused,
/// even when a shorter one, of the shared prefix only, was written later on a sibling.
#[test]
fn the_summary_of_the_longest_ancestor_prefix_wins() {
    let mut log = Log::new();
    log.turn("m1").turn("m2").turn("m3");
    summarized(&mut log, "sum_a", "SUMMARY-A", "m3");
    log.rewind("m2").turn("m2b");
    assert!(log.state().compacted.is_none(), "m2 left the path");
    summarized(&mut log, "sum_shared", "SUMMARY-SHARED", "m2b");

    let state = checked_out(&mut log, "m3_reply");
    let compacted = state.compacted.unwrap();
    assert_eq!(
        (compacted.summary.as_str(), compacted.tail_start),
        ("SUMMARY-A", 4)
    );
    let state = checked_out(&mut log, "m2b_reply");
    let compacted = state.compacted.unwrap();
    assert_eq!(
        (compacted.summary.as_str(), compacted.tail_start),
        ("SUMMARY-SHARED", 2)
    );
    // At the shared message itself only the shared summary is an ancestor prefix.
    let state = checked_out(&mut log, "m1_reply");
    assert_eq!(
        state.compacted.map(|c| c.summary),
        Some("SUMMARY-SHARED".into())
    );
}

/// Task 3.1's tree nodes: user messages only, linked to the nearest user message, with
/// labels on replies shown on their user message, and an `exact` checkout's message a
/// tip even though branches continue below it.
#[test]
fn tree_nodes_show_user_messages_with_labels_and_tips() {
    let mut log = Log::new();
    log.turn("m1").turn("m2").rewind("m2").turn("m2b").push(
        "session.branch.labeled.1",
        json!({ "name": "fix-b", "message_id": "m2b_reply" }),
    );
    let state = log.state();
    let nodes = super::super::branch_nodes(&state);
    let shape: Vec<(&str, Option<&str>, bool, bool, usize)> = nodes
        .iter()
        .map(|n| {
            let parent = n.parent_id.as_deref();
            (n.id.as_str(), parent, n.active, n.tip, n.descendants)
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("m1", None, true, false, 2),
            ("m2", Some("m1"), false, true, 0),
            ("m2b", Some("m1"), true, true, 0),
        ]
    );
    assert_eq!(nodes[2].labels, ["fix-b"]);
    assert_eq!(nodes[1].preview, "m2");
    let state = checked_out(&mut log, "m1");
    let nodes = super::super::branch_nodes(&state);
    assert!(nodes[0].tip && nodes[0].active);
    assert!(!nodes[2].active);
}

fn user_text(state: &SessionState, id: &str) -> String {
    match state.message(id) {
        Some(Entry::User { parts, .. }) => serde_json::to_value(&parts[0]).unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned(),
        other => panic!("{id}: {other:?}"),
    }
}

/// The log of [`a_rewound_id_admitted_again_by_an_old_build_keeps_both_attempts`]:
/// `x` is rewound, `y` prompted, then `x` admitted again.
fn readmitted_log() -> Log {
    let mut log = Log::new();
    log.turn("m1")
        .attempt("x", "x_reply_old", "first try")
        .rewind("x")
        .turn("y")
        .attempt("x", "x_reply_new", "second try");
    log
}

/// The steps of the active path with the user message each answers.
fn step_owners(state: &SessionState) -> Vec<(String, Option<String>)> {
    state
        .steps
        .iter()
        .map(|s| (s.step_id.clone(), s.user_message_id.clone()))
        .collect()
}

/// Regression: a log written before branches could admit a rewound message ID again
/// (workflow handbacks, job handbacks and client IDs do). Both attempts are kept, the
/// active path is the one older builds showed, and the stale attempt moves to a derived
/// ID under its old parent.
#[test]
fn a_rewound_id_admitted_again_by_an_old_build_keeps_both_attempts() {
    let state = readmitted_log().state();
    let linear = ["m1", "m1_reply", "y", "y_reply", "x", "x_reply_new"];
    assert_eq!(path(&state), linear);
    let tree = state.branches();
    assert_eq!(tree.parent("x"), Some("y_reply"));
    assert_eq!(tree.path_to("x_reply_new"), linear);
    let stale = tree.parent("x_reply_old").unwrap().to_owned();
    assert_ne!(stale, "x");
    assert_eq!(tree.parent(&stale), Some("m1_reply"));
    assert_eq!(user_text(&state, "x"), "second try");
    assert_eq!(user_text(&state, &stale), "first try");
    let digest = state.input("x").map(|r| r.digest.clone());
    assert_eq!(digest.as_deref(), Some("sha256:second try"));
}

/// Checking the stale attempt of a re-admitted ID out and back restores each attempt's
/// own text, reply and snapshot.
#[test]
fn both_attempts_of_a_readmitted_id_check_out() {
    let mut log = readmitted_log();
    let linear = path(&log.state()).join(" ");
    let stale = log
        .state()
        .branches()
        .parent("x_reply_old")
        .unwrap()
        .to_owned();
    let state = checked_out(&mut log, "x_reply_old");
    assert_eq!(path(&state), ["m1", "m1_reply", &stale, "x_reply_old"]);
    assert_eq!(user_text(&state, &stale), "first try");
    assert_eq!(
        user_text(&state, "x"),
        "second try",
        "the new attempt is stashed"
    );
    let owners = [("m1_reply", "m1"), ("x_reply_old", stale.as_str())]
        .map(|(step, user)| (step.to_owned(), Some(user.to_owned())));
    assert_eq!(step_owners(&state), owners);

    let state = checked_out(&mut log, "x_reply_new");
    assert_eq!(path(&state).join(" "), linear);
    assert_eq!(user_text(&state, "x"), "second try");
    let post = state.steps.last().and_then(|s| s.post.clone());
    assert_eq!(post.as_deref(), Some("tree_x_reply_new"));
    assert_eq!(user_text(&state, &stale), "first try");
}

/// Regression: a summary written for a path that is no longer active (a checkout landed
/// while it was being written) is dropped, not laid over the active path, and a replay
/// keeps it dropped.
#[test]
fn a_summary_of_an_inactive_path_never_replaces_the_active_context() {
    let mut log = Log::new();
    log.turn("m1")
        .turn("m2")
        .turn("m3")
        .rewind("m2")
        .turn("m2b");
    log.push(
        "session.branch.checked_out.1",
        json!({ "tip_message_id": "m3_reply", "code_restored": false }),
    );
    log.push(
        "session.compaction.started.1",
        json!({ "trigger": "manual" }),
    );
    let state = checked_out(&mut log, "m2b_reply");
    summarized(&mut log, "sum_a", "SUMMARY-A", "m3");
    let state_after = log.state();
    assert_eq!(state_after.compacted, None);
    assert_eq!(path(&state_after), path(&state));
    let state = checked_out(&mut log, "m3_reply");
    assert_eq!(state.compacted, None, "the summary was never recorded");
}

/// Regression: the tree resolves each message through one index over the active path and
/// the stash. A lookup per node scanned the path and `GET .../tree` of a long Session
/// took quadratic time (about 1.6e9 comparisons here); linear work stays far below the
/// bound even in a debug build.
#[test]
fn a_long_session_tree_is_built_in_linear_time() {
    const TURNS: usize = 20_000;
    let mut log = Log::new();
    for i in 0..TURNS {
        log.turn(&format!("m{i}"));
    }
    let last = format!("m{}", TURNS - 1);
    log.rewind(&last).turn("tail");
    let state = log.state();
    assert_eq!(state.entries.len(), 2 * TURNS);
    let started = std::time::Instant::now();
    let nodes = super::super::branch_nodes(&state);
    let elapsed = started.elapsed();
    assert_eq!(nodes.len(), TURNS + 1);
    assert_eq!(nodes[TURNS - 1].preview, last);
    assert!(!nodes[TURNS - 1].active, "the rewound message is stashed");
    assert_eq!(state.messages_by_id().len(), 2 * TURNS + 2);
    assert!(elapsed < std::time::Duration::from_secs(3), "{elapsed:?}");
}
