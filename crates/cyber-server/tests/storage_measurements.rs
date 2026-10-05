//! Opt-in release probe: cargo test -p cyber-server --release --test storage_measurements -- --ignored --nocapture
mod support;

use cyber_core::paths::DatabaseLocation;
use cyber_server::runtime::{Admission, CompactionConfig, Delivery, Runtime};
use cyber_store::{Expected, NewEvent, Store, StoreOptions};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

fn percentiles(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let at = |p: f64| samples[(samples.len() as f64 * p).ceil() as usize - 1];
    json!({"samples":samples.len(),"p50_ms":at(0.50),"p95_ms":at(0.95),"p99_ms":at(0.99),"max_ms":at(1.0)})
}

#[test]
#[ignore = "hardware-dependent workload probe"]
fn storage_workload() {
    let seconds: usize = std::env::var("CYBER_PROBE_SECONDS")
        .unwrap_or_else(|_| "20".into())
        .parse()
        .unwrap();
    assert!(seconds > 0);
    for sessions in [1, 8, 32] {
        probe(sessions, seconds);
    }
}

fn probe(sessions: usize, seconds: usize) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("cyber.db");
    let mut registry = Runtime::registry();
    registry.register("probe.tool.output.1").unwrap();
    let store = Arc::new(
        Store::open(StoreOptions::new(
            DatabaseLocation::File(database.clone()),
            registry,
        ))
        .unwrap(),
    );
    let models = support::models(vec![], 200_000, &[]);
    let runtime = support::runtime(
        &store,
        &models,
        &support::Tools::new(),
        dir.path(),
        CompactionConfig::default(),
        None,
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ids: Vec<_> = (0..sessions)
        .map(|_| {
            rt.block_on(
                runtime.create_session(cyber_server::runtime::CreateSession {
                    directory: dir.path().display().to_string(),
                    model: "test/main".into(),
                    ..Default::default()
                }),
            )
            .unwrap()
            .id
        })
        .collect();
    let stop = Arc::new(AtomicBool::new(false));
    let max_queue = Arc::new(AtomicUsize::new(0));
    let monitor = {
        let (store, stop, max_queue) = (store.clone(), stop.clone(), max_queue.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                max_queue.fetch_max(store.queue_depth(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let reader = {
        let (store, stop, ids) = (store.clone(), stop.clone(), ids.clone());
        std::thread::spawn(move || {
            let mut pages = 0;
            while !stop.load(Ordering::Relaxed) {
                for id in &ids {
                    replay(&store, id);
                    replay(&store, &format!("load_{id}"));
                    pages += 2;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            pages
        })
    };
    let barrier = Arc::new(Barrier::new(sessions + 1));
    let workers: Vec<_> = ids.into_iter().map(|id| {
        let (store, runtime, barrier) = (store.clone(), runtime.clone(), barrier.clone());
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let mut admission = Vec::new();
            let mut commit = Vec::new();
            barrier.wait();
            let start = Instant::now();
            for i in 0..seconds * 10 {
                std::thread::sleep((start + Duration::from_millis(i as u64 * 100)).saturating_duration_since(Instant::now()));
                let before = Instant::now();
                if i % 10 == 0 {
                    let mut input = Admission::text(format!("Implement task {i}: {}", "constraint ".repeat(100)), Delivery::Queue);
                    input.resume = false;
                    rt.block_on(runtime.admit(&id, input)).unwrap();
                    admission.push(before.elapsed().as_secs_f64() * 1000.0);
                } else {
                    store.append(&format!("load_{id}"), Expected::Any, vec![NewEvent::new("probe.tool.output.1",
                        json!({"call_id":format!("call_{i}"),"output":"tool result\n".repeat(320),"status":"completed"}))]).unwrap();
                    commit.push(before.elapsed().as_secs_f64() * 1000.0);
                }
            }
            (admission, commit)
        })
    }).collect();
    barrier.wait();
    let start = Instant::now();
    let mut admissions = Vec::new();
    let mut commits = Vec::new();
    for worker in workers {
        let (a, c) = worker.join().unwrap();
        admissions.extend(a);
        commits.extend(c);
    }
    let elapsed = start.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    monitor.join().unwrap();
    let replay_passes = reader.join().unwrap();
    let wal_bytes = std::fs::metadata(database.with_extension("db-wal"))
        .unwrap()
        .len();
    let database_bytes = std::fs::metadata(&database).unwrap().len();
    let before = Instant::now();
    let backup = dir.path().join("backup.db");
    cyber_store::backup::backup(&database, &backup).unwrap();
    let backup_ms = before.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(cyber_store::backup::integrity_check(&backup).unwrap(), "ok");
    let count = cyber_store::query_readonly(
        &database,
        "SELECT count(*) FROM event WHERE type IN ('session.prompt.admitted.1','probe.tool.output.1')",
    );
    // Verify persistence through replay; latency never substitutes for correctness.
    assert_eq!(admissions.len() + commits.len(), sessions * seconds * 10);
    println!(
        "P0_STORAGE {}",
        json!({"sessions":sessions,"seconds":seconds,"elapsed_seconds":elapsed,"events_per_session_per_second":10,
        "durability":"WAL/FULL","admission":percentiles(admissions),"append_commit":percentiles(commits),
        "max_queue_depth_sampled_1ms":max_queue.load(Ordering::Relaxed),"replay_passes":replay_passes,
        "wal_bytes":wal_bytes,"database_bytes":database_bytes,"backup_bytes":std::fs::metadata(backup).unwrap().len(),"backup_ms":backup_ms})
    );
    assert_eq!(count.unwrap().rows[0][0], json!(sessions * seconds * 10));
}

fn replay(store: &Store, id: &str) {
    let mut after = -1;
    loop {
        let page = store.read_events(id, after, 500).unwrap();
        after = page.events.last().map_or(after, |e| e.seq);
        if !page.has_more {
            break;
        }
    }
}
