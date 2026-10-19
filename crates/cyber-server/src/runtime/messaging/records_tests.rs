use super::*;

use cyber_core::paths::DatabaseLocation;
use cyber_store::{Expected, NewEvent, Store, StoreError, StoreOptions};
use serde_json::json;

fn store() -> Store {
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::Memory,
        crate::runtime::Runtime::registry(),
    ))
    .unwrap();
    store
        .transaction(|tx| {
            tx.execute_batch(
                "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
                 VALUES ('ses_a', 'frontend', '/r', 'build', 'test/main', 'default', 1, 1, -1),
                        ('ses_b', 'backend', '/r', 'build', 'test/main', 'default', 1, 1, -1);",
            )?;
            Ok(())
        })
        .unwrap();
    store
}

fn accepted(direction: &str, text: &str, delivery: &str) -> Value {
    json!({
        "message_id": "msgx_1", "direction": direction, "from": "frontend", "to": "backend",
        "peer_session_id": if direction == "in" { "ses_a" } else { "ses_b" },
        "origin_kind": "session", "sender_key": "ses_a", "size": text.len(),
        "sha256": text_sha256(text), "text": text, "depth": 1, "hops": 2, "delivery": delivery,
    })
}

fn refused(direction: &str) -> Value {
    json!({
        "message_id": "msgx_2", "direction": direction, "from": "frontend", "to": "backend",
        "origin_kind": "session", "sender_key": "ses_a", "size": 5,
        "sha256": text_sha256("hello"), "reason": "inbound_rule",
    })
}

fn append(store: &Store, id: &str, kind: &str, data: Value) -> Result<(), StoreError> {
    store
        .append(id, Expected::Any, vec![NewEvent::new(kind, data)])
        .map(drop)
}

fn rows(store: &Store, id: &str) -> Vec<MessageRow> {
    let id = id.to_owned();
    store.read(move |db| Ok(message_rows(db, &id)?)).unwrap()
}

#[test]
fn delivered_and_held_records_project_one_row_per_side() {
    let store = store();
    append(
        &store,
        "ses_b",
        DELIVERED,
        accepted("in", "which port?", "steer"),
    )
    .unwrap();
    append(
        &store,
        "ses_a",
        HELD,
        accepted("out", "which port?", "hold"),
    )
    .unwrap();
    let target = rows(&store, "ses_b");
    assert_eq!(target.len(), 1);
    let row = &target[0];
    assert_eq!(
        (
            row.direction,
            row.peer_address.as_str(),
            row.peer_session_id.as_deref()
        ),
        (Direction::In, "frontend", Some("ses_a"))
    );
    assert_eq!(
        (row.outcome.as_str(), row.text.as_deref()),
        ("delivered", Some("which port?"))
    );
    assert_eq!((row.size, row.depth, row.hops, row.seq), (11, 1, 2, 0));
    assert_eq!(row.sha256, text_sha256("which port?"));
    let sender = rows(&store, "ses_a");
    assert_eq!(
        (
            sender[0].direction,
            sender[0].peer_address.as_str(),
            sender[0].outcome.as_str()
        ),
        (Direction::Out, "backend", "held")
    );
}

#[test]
fn a_refused_record_has_no_text_in_its_payload_or_row() {
    let store = store();
    let mut with_text = refused("in");
    with_text["text"] = json!("hello");
    let err = append(&store, "ses_b", REFUSED, with_text).unwrap_err();
    assert!(matches!(err, StoreError::InvalidEvent { .. }), "{err}");
    append(&store, "ses_b", REFUSED, refused("in")).unwrap();
    let row = &rows(&store, "ses_b")[0];
    assert_eq!(row.outcome, "refused");
    assert_eq!(row.reason.as_deref(), Some("inbound_rule"));
    assert_eq!(row.text, None);
    assert_eq!(row.size, 5);
}

#[test]
fn accepted_records_must_match_their_text() {
    let store = store();
    let mut wrong_size = accepted("in", "abc", "steer");
    wrong_size["size"] = json!(4);
    let mut wrong_hash = accepted("in", "abc", "steer");
    wrong_hash["sha256"] = json!(text_sha256("abd"));
    let mut wrong_id = accepted("in", "abc", "steer");
    wrong_id["message_id"] = json!("msg_1");
    let queued = accepted("in", "abc", "queue");
    for data in [wrong_size, wrong_hash, wrong_id, queued] {
        let err = append(&store, "ses_b", DELIVERED, data).unwrap_err();
        assert!(matches!(err, StoreError::InvalidEvent { .. }), "{err}");
    }
    assert!(rows(&store, "ses_b").is_empty());
}

#[test]
fn release_and_drop_update_the_recorded_side_only() {
    let store = store();
    append(&store, "ses_b", HELD, accepted("in", "hi", "hold")).unwrap();
    append(&store, "ses_a", HELD, accepted("out", "hi", "hold")).unwrap();
    let released = json!({"message_id": "msgx_1", "direction": "in", "delivery": "steer"});
    append(&store, "ses_b", RELEASED, released).unwrap();
    let row = &rows(&store, "ses_b")[0];
    assert_eq!((row.outcome.as_str(), row.seq), ("released", 1));
    assert_eq!(rows(&store, "ses_a")[0].outcome, "held");
    let dropped = json!({"message_id": "msgx_1", "direction": "out", "reason": "session_deleted"});
    append(&store, "ses_a", DROPPED, dropped).unwrap();
    let row = &rows(&store, "ses_a")[0];
    assert_eq!(
        (row.outcome.as_str(), row.reason.as_deref()),
        ("dropped", Some("session_deleted"))
    );
    // A release of a message this Session never recorded is a defect: nothing commits.
    let stray = json!({"message_id": "msgx_9", "direction": "in", "delivery": "steer"});
    let err = append(&store, "ses_b", RELEASED, stray).unwrap_err();
    assert!(matches!(err, StoreError::Projector { .. }), "{err}");
    assert_eq!(store.aggregate_seq("ses_b").unwrap(), Some(1));
}

#[test]
fn watch_records_create_fire_and_expire_the_subscription() {
    let store = store();
    let created = |id: &str| json!({"watch_id": id, "target": "backend", "target_id": "ses_b", "expires_ms": 99});
    append(&store, "ses_a", WATCH_CREATED, created("wch_1")).unwrap();
    append(&store, "ses_a", WATCH_CREATED, created("wch_2")).unwrap();
    let fired = json!({"watch_id": "wch_1", "notice_id": "msgx_watch_wch_1", "status": "done", "finished_at": "2026-10-17T14:02:11Z"});
    append(&store, "ses_a", WATCH_FIRED, fired).unwrap();
    append(
        &store,
        "ses_a",
        WATCH_EXPIRED,
        json!({"watch_id": "wch_2", "reason": "expired"}),
    )
    .unwrap();
    let watches = store.read(|db| Ok(watch_rows(db, "ses_a")?)).unwrap();
    let summary: Vec<_> = watches
        .iter()
        .map(|w| {
            (
                w.id.as_str(),
                w.status.as_str(),
                w.notice_id.as_deref(),
                w.expires_ms,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("wch_1", "fired", Some("msgx_watch_wch_1"), 99),
            ("wch_2", "expired", None, 99)
        ]
    );
    assert!(watches[0].fired_ms.is_some());
    assert_eq!(
        (
            watches[0].target_address.as_str(),
            watches[0].target_id.as_str()
        ),
        ("backend", "ses_b")
    );
    // Another Session cannot fire this subscriber's watch.
    let foreign = json!({"watch_id": "wch_1", "reason": "expired"});
    let err = append(&store, "ses_b", WATCH_EXPIRED, foreign).unwrap_err();
    assert!(matches!(err, StoreError::Projector { .. }), "{err}");
}

#[test]
fn the_fold_keeps_received_chains_until_dropped() {
    let event = |kind: &str, data: Value| StoredEvent {
        id: "evt".into(),
        aggregate_id: "ses_b".into(),
        seq: 0,
        kind: kind.into(),
        data,
        time_ms: 0,
        causation_id: None,
    };
    let (mut chains, mut notices) = (BTreeMap::new(), BTreeMap::new());
    fold(
        &mut chains,
        &mut notices,
        &event(DELIVERED, accepted("in", "hi", "steer")),
    )
    .unwrap();
    let held = event(HELD, accepted("out", "hi", "hold"));
    fold(&mut chains, &mut notices, &held).unwrap();
    fold(&mut chains, &mut notices, &event(REFUSED, refused("in"))).unwrap();
    // A fired idle notice carries its hops under the notice ID, apart from messages.
    let fired = json!({"watch_id": "wch_1", "notice_id": "msgx_watch_wch_1",
        "status": "idle", "finished_at": "2026-10-19T10:00:00Z", "hops": 3});
    fold(&mut chains, &mut notices, &event(WATCH_FIRED, fired)).unwrap();
    assert_eq!(
        notices.into_iter().collect::<Vec<_>>(),
        [("msgx_watch_wch_1".to_string(), 3)]
    );
    assert_eq!(
        chains.into_iter().collect::<Vec<_>>(),
        [("msgx_1".to_string(), MessageChain { depth: 1, hops: 2 })]
    );
    let mut chains = BTreeMap::from([("msgx_1".to_string(), MessageChain::default())]);
    let dropped = json!({"message_id": "msgx_1", "direction": "in"});
    fold(&mut chains, &mut BTreeMap::new(), &event(DROPPED, dropped)).unwrap();
    assert!(chains.is_empty());
}
