use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn pending() -> Pending {
    let module = cyber_workflow::WorkflowModule::inline(
        "export const meta = {name: \"q\", description: \"d\"};\nexport default async () => 1;",
    )
    .unwrap();
    Pending {
        engine: Engine {
            module,
            limits: cyber_workflow::Limits::default(),
            tiers: Default::default(),
        },
        replay: Replay::fresh(1),
    }
}

#[test]
fn runs_are_admitted_until_the_limit_then_queue() {
    let queue = WorkflowQueue::new(2);
    let a = queue.try_admit().expect("first slot");
    let _b = queue.try_admit().expect("second slot");
    assert!(queue.try_admit().is_none(), "every slot is taken");
    queue.push("run_c", 10, pending());
    assert!(queue.next().is_none(), "no slot is free for the queued run");
    drop(a);
    // A free slot goes to the queued run, not to a new start.
    assert!(queue.try_admit().is_none());
    let (id, _, _slot) = queue.next().expect("dispatched");
    assert_eq!(id, "run_c");
    assert!(queue.next().is_none());
}

#[test]
fn dispatch_is_fifo_by_queue_time_then_run_id() {
    let queue = WorkflowQueue::new(1);
    let held = queue.try_admit().unwrap();
    queue.push("run_late", 30, pending());
    queue.push("run_b", 20, pending());
    queue.push("run_a", 20, pending());
    assert_eq!(queue.queued(), ["run_a", "run_b", "run_late"]);
    drop(held);
    let (first, _, slot) = queue.next().unwrap();
    assert_eq!(first, "run_a");
    drop(slot);
    let (second, _, _slot) = queue.next().unwrap();
    assert_eq!(second, "run_b");
}

#[test]
fn a_removed_run_leaves_the_queue_and_keeps_the_others_order() {
    let queue = WorkflowQueue::new(1);
    let held = queue.try_admit().unwrap();
    for (id, at) in [("run_1", 1), ("run_2", 2), ("run_3", 3)] {
        queue.push(id, at, pending());
    }
    assert!(queue.remove("run_2"));
    assert!(!queue.remove("run_2"), "already gone");
    assert_eq!(queue.queued(), ["run_1", "run_3"]);
    drop(held);
    assert_eq!(queue.next().unwrap().0, "run_1");
}

/// A run queued again (a resume while its start's entry was still queued) is queued once,
/// at its new place and with what it was queued with last.
#[test]
fn a_run_queued_again_replaces_its_earlier_entry() {
    let queue = WorkflowQueue::new(1);
    let held = queue.try_admit().unwrap();
    queue.push("run_a", 10, pending());
    queue.push("run_b", 20, pending());
    let mut resumed = pending();
    resumed.replay = Replay::fresh(2);
    queue.push("run_a", 30, resumed);
    assert_eq!(queue.queued(), ["run_b", "run_a"]);
    drop(held);
    let (first, _, slot) = queue.next().unwrap();
    assert_eq!(first, "run_b");
    drop(slot);
    let (second, pending, _slot) = queue.next().unwrap();
    assert_eq!(second, "run_a");
    assert_eq!(pending.replay.revision, 2, "the latest entry is dispatched");
    assert!(queue.next().is_none(), "the earlier entry is gone");
}

fn reserved(queue: &WorkflowQueue, run_id: &str, queued_ms: i64) -> Reservation {
    match queue.admit_or_reserve(run_id, queued_ms) {
        Admission::Queued(place) => place,
        Admission::Slot(_) => panic!("{run_id} got a slot"),
    }
}

/// A run refused a slot holds its place while its start is still being recorded: a slot
/// freed meanwhile goes neither to a later start nor past it, and is its own once it is
/// filled (regression: the place was taken only when the start had run its hook).
#[test]
fn a_reserved_place_holds_back_later_starts_and_dispatch_until_filled() {
    let queue = WorkflowQueue::new(1);
    let Admission::Slot(held) = queue.admit_or_reserve("run_a", 0) else {
        panic!("the first run gets the slot");
    };
    let place = reserved(&queue, "run_q", 10);
    assert_eq!(place.queued_ms(), 10);
    drop(held);
    assert!(queue.next().is_none(), "dispatch waits for the reservation");
    let later = reserved(&queue, "run_r", 20);
    later.fill(pending());
    assert!(queue.next().is_none(), "a filled run behind it waits too");
    assert_eq!(queue.queued(), ["run_q", "run_r"]);
    place.fill(pending());
    let (first, _, slot) = queue.next().expect("dispatched");
    assert_eq!(first, "run_q");
    drop(slot);
    assert_eq!(queue.next().unwrap().0, "run_r");
}

/// A start that fails after its place was reserved gives the place up and calls back, so
/// the runs it held back are dispatched.
#[test]
fn a_reservation_dropped_unfilled_gives_up_its_place() {
    let queue = WorkflowQueue::new(1);
    let held = queue.try_admit().unwrap();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&cancelled);
    let place = reserved(&queue, "run_q", 10).on_cancel(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    reserved(&queue, "run_r", 20).fill(pending());
    drop(held);
    drop(place);
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(queue.queued(), ["run_r"]);
    assert_eq!(queue.next().unwrap().0, "run_r");
    // A filled reservation calls nothing back.
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
}

#[test]
fn a_released_slot_runs_its_callback_after_freeing_the_slot() {
    let queue = WorkflowQueue::new(1);
    let fired = Arc::new(AtomicUsize::new(0));
    let probe = queue.clone();
    let count = Arc::clone(&fired);
    let slot = queue.try_admit().unwrap().on_release(move || {
        // The slot is already free when the callback dispatches the next run.
        assert!(probe.try_admit().is_some());
        count.fetch_add(1, Ordering::SeqCst);
    });
    drop(slot);
    assert_eq!(fired.load(Ordering::SeqCst), 1);
}

/// `workflows` → Run queue, "Wall budget waits in the queue", with a fixed clock: a run
/// queued for 5 minutes that then runs for 30 seconds has spent 30 seconds of wall time.
#[test]
fn the_budget_clock_of_a_queued_run_starts_at_dispatch() {
    use cyber_store::{Store, StoredEvent};
    use serde_json::json;

    use super::super::budget::{RunSpend, run_scope};
    use super::super::workflows::{RUN_DISPATCHED, RUN_PAUSED, RUN_RESUMED, RUN_STARTED, project};

    let store = Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::Memory,
        super::super::Runtime::registry(),
    ))
    .unwrap();
    let event = |seq: i64, kind: &str, status: &str, time_ms: i64| StoredEvent {
        id: format!("evt_{seq}"),
        aggregate_id: "run_q".into(),
        seq,
        kind: kind.into(),
        data: json!({"session_id": "ses_owner", "name": "q", "status": status,
            "started_ms": 0, "revision": 1, "digest": "sha256:0",
            "budget": {"max_wall_seconds": 60.0}}),
        time_ms,
        causation_id: None,
    };
    let (minute, second) = (60_000, 1_000);
    let elapsed = |events: Vec<StoredEvent>, now: i64| {
        store
            .transaction(move |tx| {
                for event in &events {
                    project(tx, event).unwrap();
                }
                let scope = run_scope(tx, "run_q", now)?.expect("a budget row");
                Ok(RunSpend::of(&scope, now).elapsed_seconds)
            })
            .unwrap()
    };
    let queued = vec![event(0, RUN_STARTED, "queued", 0)];
    assert_eq!(
        elapsed(queued, 5 * minute),
        0.0,
        "no time accrues while queued"
    );
    let dispatched = vec![event(1, RUN_DISPATCHED, "running", 5 * minute)];
    assert_eq!(elapsed(dispatched, 5 * minute + 30 * second), 30.0);
    // Paused, then resumed into the queue: the clock waits again until dispatch.
    let requeued = vec![
        event(2, RUN_PAUSED, "paused", 5 * minute + 30 * second),
        event(3, RUN_RESUMED, "queued", 6 * minute),
    ];
    assert_eq!(elapsed(requeued, 9 * minute), 30.0);
    let again = vec![event(4, RUN_DISPATCHED, "running", 9 * minute)];
    assert_eq!(elapsed(again, 9 * minute + 10 * second), 40.0);
}
