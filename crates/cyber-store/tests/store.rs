//! Behavioral tests for `storage-events` requirements.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::paths::DatabaseLocation;
use cyber_store::{
    EventRegistry, Expected, NewEvent, OwnershipLock, Store, StoreError, StoreOptions,
};
use serde_json::{Value, json};

const KIND: &str = "test.thing.happened.1";

fn registry() -> EventRegistry {
    let mut r = EventRegistry::default();
    r.register(KIND).unwrap();
    r
}

fn open(path: &Path, registry: EventRegistry) -> Store {
    Store::open(StoreOptions::new(
        DatabaseLocation::File(path.join("cyber.db")),
        registry,
    ))
    .unwrap()
}

fn event(i: i64) -> NewEvent {
    NewEvent::new(KIND, json!({ "i": i }))
}

#[test]
fn file_store_uses_wal_and_full_synchronous() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    let p = store.pragmas().unwrap();
    assert_eq!(p.journal_mode, "wal");
    assert_eq!(p.synchronous, 2);
    assert!(p.foreign_keys);
    assert_eq!(store.durability(), cyber_store::Durability::Full);
}

#[test]
fn memory_store_reports_ephemeral() {
    let store = Store::open(StoreOptions::new(DatabaseLocation::Memory, registry())).unwrap();
    assert_eq!(store.durability(), cyber_store::Durability::Ephemeral);
    store.append("a", Expected::Any, vec![event(0)]).unwrap();
    assert_eq!(store.read_events("a", -1, 10).unwrap().events.len(), 1);
}

#[test]
fn sequences_start_at_zero_and_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    store
        .append("ses_A", Expected::Seq(-1), (0..5).map(event).collect())
        .unwrap();
    let first = store.read_events("ses_A", -1, 3).unwrap();
    assert_eq!(
        first.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(first.has_more);
    let rest = store.read_events("ses_A", 2, 3).unwrap();
    assert_eq!(
        rest.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert!(!rest.has_more);
    assert_eq!(store.aggregate_seq("ses_A").unwrap(), Some(4));
}

#[test]
fn page_limit_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    assert!(matches!(
        store.read_events("a", -1, 0),
        Err(StoreError::InvalidLimit(0))
    ));
    assert!(matches!(
        store.read_events("a", -1, 501),
        Err(StoreError::InvalidLimit(501))
    ));
}

#[test]
fn concurrent_appenders_get_gapless_sequences() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(open(dir.path(), registry()));
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let s = Arc::clone(&store);
            std::thread::spawn(move || {
                for i in 0..25 {
                    s.append("ses_A", Expected::Any, vec![event(t * 100 + i)])
                        .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let page = store.read_events("ses_A", -1, 500).unwrap();
    let seqs: Vec<i64> = page.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (0..200).collect::<Vec<_>>());
}

#[test]
fn expected_sequence_conflict_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    store
        .append("a", Expected::Seq(-1), vec![event(0)])
        .unwrap();
    let err = store
        .append("a", Expected::Seq(-1), vec![event(1)])
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::Concurrency {
            expected: -1,
            actual: 0,
            ..
        }
    ));
}

#[test]
fn projector_failure_rolls_back_events_and_projections() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = registry();
    reg.register("test.thing.rejected.1").unwrap();
    reg.projector(|tx, e| {
        tx.execute("CREATE TABLE IF NOT EXISTS seen (seq INTEGER)", [])
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO seen (seq) VALUES (?1)", [e.seq])
            .map_err(|e| e.to_string())?;
        if e.kind == "test.thing.rejected.1" {
            Err("refused".into())
        } else {
            Ok(())
        }
    });
    let store = open(dir.path(), reg);
    store.append("a", Expected::Any, vec![event(0)]).unwrap();
    let err = store
        .append(
            "a",
            Expected::Any,
            vec![event(1), NewEvent::new("test.thing.rejected.1", json!({}))],
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::Projector { .. }));
    assert_eq!(store.read_events("a", -1, 10).unwrap().events.len(), 1);
    assert_eq!(store.aggregate_seq("a").unwrap(), Some(0));
    let seen =
        cyber_store::query_readonly(&dir.path().join("cyber.db"), "SELECT count(*) FROM seen")
            .unwrap();
    assert_eq!(seen.rows[0][0], json!(1));
}

#[test]
fn unregistered_and_malformed_types_are_defects() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    let unknown = store.append(
        "a",
        Expected::Any,
        vec![NewEvent::new("test.other.thing.1", json!({}))],
    );
    assert!(matches!(unknown, Err(StoreError::UnregisteredEvent(_))));
    let malformed = store.append("a", Expected::Any, vec![NewEvent::new("Bad", json!({}))]);
    assert!(matches!(malformed, Err(StoreError::InvalidEventType(_))));
    assert_eq!(store.aggregate_seq("a").unwrap(), None);
}

#[test]
fn validators_reject_bad_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = EventRegistry::default();
    reg.register_with(KIND, |v| {
        v.get("i").map(|_| ()).ok_or_else(|| "missing i".into())
    })
    .unwrap();
    let store = open(dir.path(), reg);
    let err = store
        .append("a", Expected::Any, vec![NewEvent::new(KIND, json!({}))])
        .unwrap_err();
    assert!(matches!(err, StoreError::InvalidEvent { .. }));
}

#[test]
fn old_versions_are_upcast_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut v1 = EventRegistry::default();
    v1.register("session.prompt.admitted.1").unwrap();
    open(dir.path(), v1)
        .append(
            "ses_A",
            Expected::Any,
            vec![NewEvent::new(
                "session.prompt.admitted.1",
                json!({"prompt": "hi"}),
            )],
        )
        .unwrap();

    let mut v2 = EventRegistry::default();
    v2.register("session.prompt.admitted.1").unwrap();
    v2.register("session.prompt.admitted.2").unwrap();
    v2.upcaster("session.prompt.admitted.1", |mut data: Value| {
        data["delivery"] = json!("steer");
        data
    })
    .unwrap();
    let events = open(dir.path(), v2)
        .read_events("ses_A", -1, 10)
        .unwrap()
        .events;
    assert_eq!(events[0].kind, "session.prompt.admitted.2");
    assert_eq!(events[0].data, json!({"prompt": "hi", "delivery": "steer"}));
}

#[test]
fn database_from_newer_version_opens_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cyber.db");
    open(dir.path(), registry())
        .append("a", Expected::Any, vec![event(0)])
        .unwrap();
    {
        let conn = rusqlite_connection(&path);
        conn.execute(
            "INSERT INTO migration (id, time_completed) VALUES ('20991231000000_future', 0)",
            [],
        )
        .unwrap();
    }
    let store = open(dir.path(), registry());
    assert_eq!(
        store.too_new(),
        Some(&["20991231000000_future".to_string()][..])
    );
    let err = store
        .append("a", Expected::Any, vec![event(1)])
        .unwrap_err();
    assert!(
        err.to_string()
            .starts_with("DatabaseTooNewError: upgrade cyber"),
        "{err}"
    );
    assert_eq!(store.read_events("a", -1, 10).unwrap().events.len(), 1);
}

#[test]
fn disk_full_stops_mutations_without_acknowledging() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = StoreOptions::new(
        DatabaseLocation::File(dir.path().join("cyber.db")),
        registry(),
    );
    options.max_page_count = Some(40);
    let store = Store::open(options).unwrap();
    let big = "x".repeat(16 * 1024);
    let mut acked = 0;
    let failure = loop {
        match store.append(
            "a",
            Expected::Any,
            vec![NewEvent::new(KIND, json!({ "pad": big }))],
        ) {
            Ok(_) => acked += 1,
            Err(e) => break e,
        }
        assert!(acked < 1000, "the page cap never filled the database");
    };
    assert!(
        failure.is_fatal(),
        "expected StorageUnavailableError, got {failure}"
    );
    assert!(store.has_failed());
    let after = store
        .append("a", Expected::Any, vec![event(1)])
        .unwrap_err();
    assert!(
        after.to_string().starts_with("StorageUnavailableError"),
        "{after}"
    );
    let stored = store.read_events("a", -1, 500).unwrap().events.len();
    assert_eq!(stored, acked);
}

#[test]
fn tail_replays_then_follows_without_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(open(dir.path(), registry()));
    store
        .append("ses_A", Expected::Any, (0..3).map(event).collect())
        .unwrap();
    let mut tail = store.tail("ses_A", 0);
    let writer = {
        let s = Arc::clone(&store);
        std::thread::spawn(move || {
            for i in 3..40 {
                s.append("ses_A", Expected::Any, vec![event(i)]).unwrap();
            }
        })
    };
    let mut seen = Vec::new();
    while seen.len() < 39 {
        let batch = tail.next_batch(Duration::from_secs(5)).unwrap();
        assert!(!batch.is_empty(), "tail stalled at {:?}", seen.last());
        seen.extend(batch.into_iter().map(|e| e.seq));
    }
    writer.join().unwrap();
    assert_eq!(seen, (1..40).collect::<Vec<_>>());
}

#[test]
fn ownership_lock_is_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.lock");
    let held = OwnershipLock::acquire(&path).unwrap();
    assert!(matches!(
        OwnershipLock::acquire(&path),
        Err(StoreError::LockHeld(_))
    ));
    drop(held);
    assert!(OwnershipLock::acquire(&path).is_ok());
}

#[test]
fn readonly_query_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    open(dir.path(), registry())
        .append("a", Expected::Any, vec![event(0)])
        .unwrap();
    let path = dir.path().join("cyber.db");
    let err = cyber_store::query_readonly(&path, "DELETE FROM event").unwrap_err();
    assert!(
        err.to_string()
            .contains("attempt to write a readonly database"),
        "{err}"
    );
    let count = cyber_store::query_readonly(&path, "SELECT count(*) AS n FROM event").unwrap();
    assert_eq!(count.columns, vec!["n"]);
    assert_eq!(count.rows, vec![vec![json!(1)]]);
}

fn rusqlite_connection(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

/// `storage-events` → Backup: an online backup taken while events are being written opens
/// cleanly, passes integrity_check and holds a gapless prefix of the history.
#[test]
fn online_backup_is_consistent_while_writing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(open(dir.path(), registry()));
    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            for i in 0..400 {
                store
                    .append("ses_A", Expected::Any, vec![event(i)])
                    .unwrap();
            }
        })
    };
    std::thread::sleep(Duration::from_millis(20));
    let copy = dir.path().join("backup.db");
    cyber_store::backup::backup(&dir.path().join("cyber.db"), &copy).unwrap();
    writer.join().unwrap();
    assert_eq!(cyber_store::backup::integrity_check(&copy).unwrap(), "ok");
    let restored = Store::open(StoreOptions::new(
        DatabaseLocation::File(copy.clone()),
        registry(),
    ))
    .unwrap();
    let page = restored.read_events("ses_A", -1, 500).unwrap();
    let seqs: Vec<i64> = page.events.iter().map(|e| e.seq).collect();
    assert!(
        seqs.iter().enumerate().all(|(i, s)| *s == i as i64),
        "gapless prefix"
    );
    assert!(
        cyber_store::backup::backup(&dir.path().join("cyber.db"), &copy).is_err(),
        "never overwrites"
    );
}
