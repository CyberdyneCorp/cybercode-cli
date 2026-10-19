use super::*;

use cyber_core::paths::DatabaseLocation;
use cyber_store::{Expected, Store, StoreOptions};
use serde_json::json;

use crate::runtime::model::AssistantEntry;

fn store() -> Store {
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::Memory,
        Runtime::registry(),
    ))
    .unwrap();
    store
        .transaction(|tx| {
            tx.execute_batch(
                "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
                 VALUES ('ses_a', 'coordinator', '/r', 'build', 'test/main', 'default', 1, 1, -1);",
            )?;
            Ok(())
        })
        .unwrap();
    store
}

fn create(store: &Store, id: &str, expires_ms: i64) {
    let created = json!({"watch_id": id, "target": "migration", "target_id": "ses_b", "expires_ms": expires_ms});
    store
        .append(
            "ses_a",
            Expected::Any,
            vec![NewEvent::new(WATCH_CREATED, created)],
        )
        .unwrap();
}

fn due_at(store: &Store, now_ms: i64) -> Vec<String> {
    store
        .read(move |db| Ok(query::due(db, now_ms)?))
        .unwrap()
        .into_iter()
        .map(|w| w.id)
        .collect()
}

/// Task 8.2: the sweep's clock is injected; a watch is due once its 12 hours have passed,
/// and a fired or expired watch is never due again.
#[test]
fn watches_fall_due_after_their_expiry_with_an_injected_clock() {
    let store = store();
    let created_at = 1_000;
    create(&store, "wch_early", created_at + WATCH_TTL_MS);
    create(&store, "wch_late", created_at + WATCH_TTL_MS + 60_000);
    create(&store, "wch_fired", created_at + WATCH_TTL_MS);
    let fired = json!({"watch_id": "wch_fired", "notice_id": "msgx_watch_wch_fired", "status": "done", "finished_at": "t", "hops": 1});
    store
        .append(
            "ses_a",
            Expected::Any,
            vec![NewEvent::new(WATCH_FIRED, fired)],
        )
        .unwrap();

    assert!(due_at(&store, created_at + WATCH_TTL_MS - 1).is_empty());
    assert_eq!(due_at(&store, created_at + WATCH_TTL_MS), ["wch_early"]);
    assert_eq!(
        due_at(&store, created_at + WATCH_TTL_MS + 60_000),
        ["wch_early", "wch_late"]
    );
    let expired =
        json!({"watch_id": "wch_early", "reason": "expired", "notice_id": "msgx_watch_wch_early"});
    store
        .append(
            "ses_a",
            Expected::Any,
            vec![NewEvent::new(WATCH_EXPIRED, expired.clone())],
        )
        .unwrap();
    assert_eq!(due_at(&store, i64::MAX), ["wch_late"]);
    let row = store
        .read(|db| Ok(query::by_id(db, "wch_early")?))
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.status.as_str(), row.notice_id.as_deref()),
        ("expired", Some("msgx_watch_wch_early"))
    );
    // A watch ends once: a second expiry or fire fails the commit.
    assert!(
        store
            .append(
                "ses_a",
                Expected::Any,
                vec![NewEvent::new(WATCH_EXPIRED, expired)]
            )
            .is_err()
    );
    let targets = store.read(|db| Ok(query::watched_targets(db)?)).unwrap();
    assert_eq!(targets, ["ses_b"]);
    let active = store
        .read(|db| Ok(query::active_count(db, "ses_a")?))
        .unwrap();
    assert_eq!(active, 1);
}

fn step(text: &str, error: Option<&str>, kind: Option<&str>, finished: bool) -> Entry {
    Entry::Assistant(AssistantEntry {
        id: "msg_1".into(),
        text: text.into(),
        finished,
        error: error.map(str::to_owned),
        error_kind: kind.map(str::to_owned),
        ended_ms: Some(1_700_000_000_000),
        ..AssistantEntry::default()
    })
}

fn user() -> Entry {
    Entry::User {
        id: "msg_u".into(),
        parts: Vec::new(),
        source: "user".into(),
        promoted_ms: None,
    }
}

fn status(entries: Vec<Entry>) -> String {
    let info: crate::runtime::model::SessionInfo = serde_json::from_value(json!({
        "id": "ses_t", "title": "t", "directory": "/r", "parent_id": null,
        "agent": "build", "model": "test/main", "mode": "default", "created_ms": 1,
    }))
    .unwrap();
    let mut state = SessionState::new(info);
    state.entries = entries;
    last_run(&state).0
}

/// Task 8.1: the status line is the first line of the last run's text, at most 200
/// characters, else how the run ended; text from before the newest input never counts.
#[test]
fn the_status_line_reports_the_last_run() {
    assert_eq!(
        status(vec![step(
            "\n  migration finished: 12 tables moved\nmore",
            None,
            None,
            true
        )]),
        "migration finished: 12 tables moved"
    );
    assert_eq!(
        status(vec![
            step("all done", None, None, true),
            step("", None, None, true)
        ]),
        "all done",
        "a final step without text reports the run's last text"
    );
    let long = status(vec![step(&"x".repeat(500), None, None, true)]);
    assert_eq!(long.chars().count(), STATUS_LIMIT);
    assert!(long.ends_with('…'));
    assert_eq!(
        status(vec![step("", Some("rate limited"), Some("model"), true)]),
        "failed: rate limited"
    );
    assert_eq!(
        status(vec![step(
            "half",
            Some("stopped"),
            Some("interrupted"),
            true
        )]),
        "interrupted"
    );
    assert_eq!(status(vec![step("half", None, None, false)]), "interrupted");
    assert_eq!(
        status(vec![step("old answer", None, None, true), user()]),
        "idle"
    );
    assert_eq!(status(Vec::new()), "idle");
}

#[test]
fn watch_and_notice_ids_are_derived_and_validated() {
    assert_eq!(watch_id_for("msgx_01JABC"), "wch_01JABC");
    assert_eq!(notice_id("wch_01JABC"), "msgx_watch_wch_01JABC");
    let id = idempotent_watch_id("ses_a", "call_1");
    assert_eq!(id, idempotent_watch_id("ses_a", "call_1"));
    assert_ne!(id, idempotent_watch_id("ses_a", "call_2"));
    assert!(valid_watch_id(&id) && valid_watch_id(&watch_id_for("msgx_a-b_c")));
    for bad in [
        "wch_",
        "wch_a b",
        "msgx_1",
        &format!("wch_{}", "a".repeat(121)),
    ] {
        assert!(!valid_watch_id(bad), "{bad}");
    }
}
