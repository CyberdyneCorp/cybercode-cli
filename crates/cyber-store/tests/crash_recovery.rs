//! Process-kill recovery (`storage-events` → Durability boundary and storage failure).
//!
//! SIGKILL of the writer process only. This is not a power-loss or filesystem fault
//! simulation; those require a fault-injecting VFS and are tracked separately.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use cyber_core::paths::DatabaseLocation;
use cyber_store::{EventRegistry, Store, StoreOptions};

#[test]
fn process_kill_preserves_acknowledged_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cyber.db");
    let mut child = Command::new(env!("CARGO_BIN_EXE_crash_writer"))
        .arg(&db)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut acked = Vec::new();
    while acked.len() < 200 {
        let line = lines.next().expect("writer exited early").unwrap();
        acked.push(parse_ack(&line));
    }
    child.kill().unwrap();
    // Acknowledgements already in the pipe were also committed before being printed.
    acked.extend(lines.map_while(Result::ok).map(|l| parse_ack(&l)));
    child.wait().unwrap();

    let mut registry = EventRegistry::default();
    registry.register("test.counter.incremented.1").unwrap();
    let store = Store::open(StoreOptions::new(DatabaseLocation::File(db), registry)).unwrap();
    let mut stored = Vec::new();
    let mut after = -1;
    loop {
        let page = store.read_events("agg_crash", after, 500).unwrap();
        stored.extend(page.events.iter().map(|e| e.seq));
        after = *stored.last().unwrap_or(&after);
        if !page.has_more {
            break;
        }
    }
    let max_acked = *acked.last().unwrap();
    assert_eq!(
        stored,
        (0..stored.len() as i64).collect::<Vec<_>>(),
        "sequence has gaps"
    );
    assert!(
        *stored.last().unwrap() >= max_acked,
        "acknowledged seq {max_acked} was lost"
    );
}

fn parse_ack(line: &str) -> i64 {
    line.strip_prefix("acked ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("bad line {line:?}"))
}
