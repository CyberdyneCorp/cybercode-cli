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
    let baseline = open(dir.path(), registry());
    let pages: u32 = baseline
        .read(|db| Ok(db.query_row("PRAGMA page_count", [], |row| row.get(0))?))
        .unwrap();
    drop(baseline);
    options.max_page_count = Some(pages + 8);
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
        acked > 0,
        "the fixture must permit writes before the disk fills"
    );
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

#[test]
fn checked_append_rolls_back_decision_mutations_when_events_are_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    store
        .transaction(|tx| {
            tx.execute("CREATE TABLE decision(value INTEGER NOT NULL)", [])?;
            Ok(())
        })
        .unwrap();
    let error = store
        .append_checked("ses_decision", Expected::Seq(-1), |tx| {
            tx.execute("INSERT INTO decision VALUES (1)", [])?;
            Ok((vec![NewEvent::new("unknown.event.1", json!({}))], ()))
        })
        .unwrap_err();
    assert!(matches!(error, StoreError::UnregisteredEvent(_)));
    let count: i64 = store
        .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM decision", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(store.aggregate_seq("ses_decision").unwrap(), None);
    let (stored, value) = store
        .append_checked("ses_decision", Expected::Seq(-1), |tx| {
            tx.execute("INSERT INTO decision VALUES (2)", [])?;
            Ok((vec![event(2)], "committed"))
        })
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(value, "committed");
    let stored_value: i64 = store
        .read(|conn| Ok(conn.query_row("SELECT value FROM decision", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(stored_value, 2);
}

fn pair_owners() -> [(String, i64); 2] {
    [
        ("native_worker".into(), -1),
        ("resolution_receipt".into(), -1),
    ]
}

#[test]
fn paired_append_commits_both_owners_before_notifying_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    let notifications = store.subscribe();
    let (events, result) = store
        .append_pair_checked(pair_owners(), |tx| {
            let count: i64 = tx.query_row("SELECT count(*) FROM event", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            Ok(([vec![event(1), event(2)], vec![event(3)]], "acknowledged"))
        })
        .unwrap();
    assert_eq!(result, "acknowledged");
    assert_eq!(events[0].iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(events[1][0].seq, 0);
    for (owner, sequence) in [("native_worker", 1), ("resolution_receipt", 0)] {
        let notification = notifications.try_recv().unwrap();
        assert_eq!(notification.aggregate_id, owner);
        assert_eq!(notification.seq, sequence);
        // Even the first owner's notification exposes the complete committed pair.
        assert_eq!(store.aggregate_seq("native_worker").unwrap(), Some(1));
        assert_eq!(store.aggregate_seq("resolution_receipt").unwrap(), Some(0));
    }
    assert!(notifications.try_recv().is_err());
    drop(store);
    let reopened = open(dir.path(), registry());
    assert_eq!(
        reopened
            .read_events("native_worker", -1, 10)
            .unwrap()
            .events
            .len(),
        2
    );
    assert_eq!(
        reopened
            .read_events("resolution_receipt", -1, 10)
            .unwrap()
            .events
            .len(),
        1
    );
}

#[test]
fn paired_append_late_failure_rolls_back_decision_events_sequences_and_projections() {
    for failure in ["sequence", "validation", "projector"] {
        assert_paired_failure_rolls_back(failure);
    }
}

fn assert_paired_failure_rolls_back(failure: &'static str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut registry = registry();
    registry
        .register_with("test.invalid.data.1", |_| Err("refused payload".into()))
        .unwrap();
    let projected = Arc::new(AtomicUsize::new(0));
    let seen = projected.clone();
    registry.projector(move |tx, event| {
        seen.fetch_add(1, Ordering::SeqCst);
        tx.execute(
            "INSERT INTO paired_projection VALUES (?1)",
            [&event.aggregate_id],
        )
        .map_err(|error| error.to_string())?;
        if failure == "projector" && event.aggregate_id == "resolution_receipt" {
            return Err("refused resolution projection".into());
        }
        Ok(())
    });
    let store = open(dir.path(), registry);
    store.transaction(|tx| {
            tx.execute_batch("CREATE TABLE paired_projection(owner TEXT); CREATE TABLE paired_decision(value INTEGER)")?;
            Ok(())
        }).unwrap();
    let notifications = store.subscribe();
    let mut owners = pair_owners();
    if failure == "sequence" {
        owners[1].1 = 0;
    }
    let result = store.append_pair_checked(owners, move |tx| {
        tx.execute("INSERT INTO paired_decision VALUES (1)", [])?;
        let second = if failure == "validation" {
            NewEvent::new("test.invalid.data.1", json!({}))
        } else {
            event(2)
        };
        Ok(([vec![event(1)], vec![second]], ()))
    });
    match failure {
        "sequence" => assert!(matches!(result, Err(StoreError::Concurrency { .. }))),
        "validation" => assert!(matches!(result, Err(StoreError::InvalidEvent { .. }))),
        _ => assert!(matches!(result, Err(StoreError::Projector { .. }))),
    }
    assert!(
        projected.load(Ordering::SeqCst) >= 1,
        "first append must actually reach its projector"
    );
    assert_eq!(store.aggregate_seq("native_worker").unwrap(), None);
    assert_eq!(store.aggregate_seq("resolution_receipt").unwrap(), None);
    let counts: (i64, i64, i64) = store.read(|db| Ok(db.query_row(
            "SELECT (SELECT count(*) FROM event), (SELECT count(*) FROM paired_projection), (SELECT count(*) FROM paired_decision)",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)).unwrap();
    assert_eq!(counts, (0, 0, 0));
    assert!(notifications.try_recv().is_err());
    assert!(
        !store.has_failed(),
        "a rejected decision must not disable the writer"
    );
}

#[test]
fn paired_append_rejected_commit_has_no_partial_receipt_or_notification() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    store.transaction(|tx| {
        tx.execute_batch("CREATE TABLE paired_parent(id INTEGER PRIMARY KEY); CREATE TABLE paired_child(parent_id INTEGER REFERENCES paired_parent(id) DEFERRABLE INITIALLY DEFERRED)")?;
        Ok(())
    }).unwrap();
    let notifications = store.subscribe();
    let result = store.append_pair_checked(pair_owners(), |tx| {
        tx.execute("INSERT INTO paired_child VALUES (1)", [])?;
        Ok(([vec![event(1)], vec![event(2)]], ()))
    });
    assert!(matches!(result, Err(StoreError::Sqlite(_))));
    for (owner, _) in pair_owners() {
        assert_eq!(store.aggregate_seq(&owner).unwrap(), None);
    }
    assert!(notifications.try_recv().is_err());
    assert!(!store.has_failed());
    store
        .append_pair_checked(pair_owners(), |_| {
            Ok(([vec![event(1)], vec![event(2)]], ()))
        })
        .unwrap();
    assert_eq!(store.aggregate_seq("native_worker").unwrap(), Some(0));
    assert_eq!(store.aggregate_seq("resolution_receipt").unwrap(), Some(0));
}

#[test]
fn paired_append_rejects_invalid_owners_without_running_the_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    for owners in [
        [("same".into(), -1), ("same".into(), -1)],
        [(String::new(), -1), ("receipt".into(), -1)],
        [("x".repeat(257), -1), ("receipt".into(), -1)],
        [("native".into(), -2), ("receipt".into(), -1)],
    ] {
        let result =
            store.append_pair_checked::<(), _>(owners, |_| panic!("invalid owner admitted"));
        assert!(matches!(result, Err(StoreError::InvalidAppend(_))));
    }
    assert!(!store.has_failed());
}

#[test]
fn paired_append_bounds_roll_back_decision_writes_without_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    let notifications = store.subscribe();
    for events in [
        [(0..65).map(event).collect(), vec![]],
        [
            vec![NewEvent::new(
                KIND,
                json!({"payload": "x".repeat(16 * 1024 * 1024)}),
            )],
            vec![],
        ],
        [
            vec![NewEvent::new(
                KIND,
                json!({"payload": "x".repeat(8 * 1024 * 1024)}),
            )],
            vec![NewEvent::new(
                KIND,
                json!({"payload": "x".repeat(8 * 1024 * 1024)}),
            )],
        ],
    ] {
        let result = store.append_pair_checked(pair_owners(), move |tx| {
            tx.execute_batch("CREATE TABLE rejected_pair(value INTEGER)")?;
            Ok((events, ()))
        });
        assert!(matches!(result, Err(StoreError::InvalidAppend(_))));
        let count: i64 = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='rejected_pair'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(store.aggregate_seq("native_worker").unwrap(), None);
        assert!(notifications.try_recv().is_err());
        assert!(!store.has_failed());
    }
}

#[test]
fn paired_append_empty_retry_still_checks_both_owners_and_does_not_notify() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), registry());
    let owners = [("n".repeat(256), -1), ("r".repeat(256), -1)];
    store
        .append_pair_checked(owners.clone(), |_| {
            Ok((
                [(0..32).map(event).collect(), (32..64).map(event).collect()],
                (),
            ))
        })
        .unwrap();
    let notifications = store.subscribe();
    let owned = [(owners[0].0.clone(), 31), (owners[1].0.clone(), 31)];
    let (_, result) = store
        .append_pair_checked(owned.clone(), |_| {
            Ok(([vec![], vec![]], "retained receipt"))
        })
        .unwrap();
    assert_eq!(result, "retained receipt");
    assert!(notifications.try_recv().is_err());
    let stale = [(owned[0].0.clone(), 31), (owned[1].0.clone(), 30)];
    assert!(matches!(
        store.append_pair_checked(stale, |_| Ok(([vec![], vec![]], ()))),
        Err(StoreError::Concurrency { .. })
    ));
    let error = store.append_pair_checked::<(), _>(owned, |tx| {
        tx.execute_batch("CREATE TABLE refused_decision(value INTEGER)")?;
        Err(StoreError::InvalidAppend("decision refused".into()))
    });
    assert!(matches!(error, Err(StoreError::InvalidAppend(_))));
    let count: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='refused_decision'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 0);
    assert!(notifications.try_recv().is_err());
    assert!(!store.has_failed());
}

#[test]
fn paired_append_concurrent_attempts_cannot_duplicate_either_owner() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(open(dir.path(), registry()));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.append_pair_checked(pair_owners(), |_| {
                    Ok(([vec![event(1)], vec![event(2)]], ()))
                })
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Concurrency { .. })))
            .count(),
        1
    );
    for (owner, _) in pair_owners() {
        assert_eq!(store.read_events(&owner, -1, 10).unwrap().events.len(), 1);
    }
}

/// A projection rebuilt from its events: the tables are cleared and only events of the
/// prefix are re-projected, upcast and in commit order; a failing projector leaves the
/// previous rows in place.
#[test]
fn reproject_rebuilds_a_projection_from_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = registry();
    reg.register("other.thing.happened.1").unwrap();
    reg.register("test.thing.renamed.1").unwrap();
    reg.register("test.thing.renamed.2").unwrap();
    reg.upcaster("test.thing.renamed.1", |mut data| {
        data["i"] = json!(data["old"].as_i64().unwrap_or(0) * 10);
        data
    })
    .unwrap();
    let store = open(dir.path(), reg);
    store
        .transaction(|tx| {
            tx.execute(
                "CREATE TABLE seen (aggregate TEXT, seq INTEGER, i INTEGER)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    store.append("b", Expected::Any, vec![event(1)]).unwrap();
    store
        .append(
            "a",
            Expected::Any,
            vec![
                NewEvent::new("other.thing.happened.1", json!({ "i": 99 })),
                NewEvent::new("test.thing.renamed.1", json!({ "old": 2 })),
            ],
        )
        .unwrap();
    store
        .transaction(|tx| {
            tx.execute("INSERT INTO seen VALUES ('stale', 0, 0)", [])?;
            Ok(())
        })
        .unwrap();
    let project: cyber_store::Projector = Arc::new(|tx, e| {
        tx.execute(
            "INSERT INTO seen VALUES (?1, ?2, ?3)",
            rusqlite::params![e.aggregate_id, e.seq, e.data["i"].as_i64()],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    assert_eq!(store.reproject(&["seen"], "test.", project).unwrap(), 2);
    let db = dir.path().join("cyber.db");
    let rows = cyber_store::query_readonly(&db, "SELECT aggregate, seq, i FROM seen").unwrap();
    assert_eq!(
        rows.rows,
        [
            vec![json!("b"), json!(0), json!(1)],
            vec![json!("a"), json!(1), json!(20)]
        ]
    );

    let failing: cyber_store::Projector = Arc::new(|_, _| Err("broken".into()));
    let err = store.reproject(&["seen"], "test.", failing).unwrap_err();
    assert!(matches!(err, StoreError::Projector { .. }), "{err}");
    let kept = cyber_store::query_readonly(&db, "SELECT count(*) FROM seen").unwrap();
    assert_eq!(kept.rows[0][0], json!(2));
    let noop: cyber_store::Projector = Arc::new(|_, _| Ok(()));
    assert!(
        store
            .reproject(&["seen; DROP TABLE event"], "test.", noop)
            .is_err()
    );
}

/// The crash-injection hook fails matching appends before anything is written, leaves
/// other aggregates alone, and stops once cleared.
#[test]
fn an_append_fault_fails_matching_appends_without_writing() {
    let store = Store::open(StoreOptions::new(DatabaseLocation::Memory, registry())).unwrap();
    store.set_append_fault(Some(Arc::new(|aggregate, e| {
        aggregate == "ses_B" && e.data["i"] == 1
    })));
    let failed = store.append("ses_B", Expected::Any, vec![event(0), event(1)]);
    assert!(
        matches!(failed, Err(StoreError::Unavailable(_))),
        "{failed:?}"
    );
    assert!(
        !store.has_failed(),
        "an injected fault is not a storage failure"
    );
    assert_eq!(store.aggregate_seq("ses_B").unwrap(), None);
    store
        .append("ses_A", Expected::Any, vec![event(1)])
        .unwrap();
    store.set_append_fault(None);
    store
        .append("ses_B", Expected::Any, vec![event(1)])
        .unwrap();
    assert_eq!(store.aggregate_seq("ses_B").unwrap(), Some(0));
}
