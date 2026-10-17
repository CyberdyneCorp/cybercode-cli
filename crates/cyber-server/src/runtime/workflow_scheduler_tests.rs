//! The "Fair agent scheduling" scenarios, driven synchronously: no runtime, no clock.

use std::collections::VecDeque;

use super::*;

/// Issued calls and granted slots, by run, in issue order.
struct Harness {
    scheduler: WorkflowScheduler,
    pending: Vec<(String, String, Waiter)>,
    running: Vec<(String, String, Slot)>,
}

impl Harness {
    fn new(capacity: usize) -> Self {
        Self {
            scheduler: WorkflowScheduler::new(capacity),
            pending: Vec::new(),
            running: Vec::new(),
        }
    }

    /// Issue `count` calls of `run` labelled `<run><n>`, numbered from `from`.
    fn issue(&mut self, run: &str, share: usize, from: usize, count: usize) -> Vec<String> {
        for n in from..from + count {
            let waiter = self.scheduler.enqueue(run, share);
            self.pending.push((run.into(), format!("{run}{n}"), waiter));
        }
        self.collect()
    }

    /// Move granted calls to `running`; the labels granted, in issue order.
    fn collect(&mut self) -> Vec<String> {
        let mut granted = Vec::new();
        let mut kept = Vec::new();
        for (run, label, mut waiter) in std::mem::take(&mut self.pending) {
            match waiter.try_take() {
                Ok(Some(slot)) => {
                    assert_eq!(slot.run_id(), run);
                    granted.push(label.clone());
                    self.running.push((run, label, slot));
                }
                Ok(None) => kept.push((run, label, waiter)),
                Err(Closed) => {}
            }
        }
        self.pending = kept;
        granted
    }

    /// End the oldest running agent of `run`; the labels granted in its place.
    fn release(&mut self, run: &str) -> Vec<String> {
        let at = self
            .running
            .iter()
            .position(|(owner, _, _)| owner == run)
            .expect("an agent of the run is running");
        drop(self.running.remove(at));
        self.collect()
    }

    /// End the oldest running agent of any run.
    fn release_oldest(&mut self) -> Vec<String> {
        drop(self.running.remove(0));
        self.collect()
    }

    fn running_of(&self, run: &str) -> usize {
        self.running
            .iter()
            .filter(|(owner, _, _)| owner == run)
            .count()
    }

    fn queued_of(&self, run: &str) -> usize {
        self.pending
            .iter()
            .filter(|(owner, _, _)| owner == run)
            .count()
    }
}

#[test]
fn a_small_run_is_not_starved_by_a_large_one() {
    let mut h = Harness::new(16);
    assert_eq!(h.issue("a", 16, 1, 100).len(), 16);
    assert!(h.issue("b", 16, 1, 2).is_empty());
    assert_eq!(
        h.scheduler.snapshot("a"),
        RunSlots {
            queued: 84,
            running: 16
        }
    );
    assert_eq!(h.release("a"), ["b1"]);
    assert_eq!(h.release("a"), ["a17"]);
    assert_eq!(h.release("a"), ["b2"]);
    assert_eq!(h.release("a"), ["a18"]);
}

#[test]
fn contended_slots_split_equally_and_stay_split() {
    let mut h = Harness::new(4);
    assert_eq!(h.issue("a", 4, 1, 14).len(), 4);
    assert!(h.issue("b", 4, 1, 10).is_empty());
    for _ in 0..4 {
        assert_eq!(h.release_oldest().len(), 1);
    }
    assert_eq!((h.running_of("a"), h.running_of("b")), (2, 2));
    while h.queued_of("a") > 0 && h.queued_of("b") > 0 {
        assert_eq!(h.release_oldest().len(), 1);
        assert_eq!((h.running_of("a"), h.running_of("b")), (2, 2));
    }
}

#[test]
fn idle_slots_are_used_by_the_only_waiting_run() {
    let mut h = Harness::new(16);
    assert_eq!(h.issue("a", 16, 1, 20).len(), 16);
    assert_eq!(h.queued_of("a"), 4);
}

#[test]
fn a_per_run_share_caps_the_run_and_others_use_the_rest() {
    let mut h = Harness::new(16);
    assert_eq!(h.issue("a", 4, 1, 20).len(), 4);
    assert_eq!(h.issue("b", 16, 1, 20).len(), 12);
    for _ in 0..20 {
        h.release_oldest();
        assert!(h.running_of("a") <= 4);
        if h.queued_of("a") > 0 && h.queued_of("b") > 0 {
            assert_eq!(h.running.len(), 16);
        }
    }
    // A share above the capacity is capped at it.
    let mut h = Harness::new(2);
    assert_eq!(h.issue("c", 100, 1, 3).len(), 2);
}

#[test]
fn a_raised_share_is_used_at_once() {
    let mut h = Harness::new(8);
    assert_eq!(h.issue("a", 2, 1, 6).len(), 2);
    h.scheduler.set_share("a", 5);
    assert_eq!(h.collect().len(), 3);
    assert_eq!(h.running_of("a"), 5);
}

#[test]
fn calls_of_one_run_start_in_issue_order() {
    let mut h = Harness::new(1);
    assert_eq!(h.issue("c", 1, 0, 1), ["c0"]);
    assert!(h.issue("c", 1, 1, 3).is_empty());
    assert_eq!(h.release("c"), ["c1"]);
    assert_eq!(h.release("c"), ["c2"]);
    assert_eq!(h.release("c"), ["c3"]);
}

#[test]
fn suspending_a_run_drops_its_waiters_until_it_is_unsuspended() {
    let mut h = Harness::new(1);
    assert_eq!(h.issue("a", 1, 1, 1), ["a1"]);
    assert!(h.issue("a", 1, 2, 2).is_empty());
    assert!(h.issue("b", 1, 1, 1).is_empty());
    h.scheduler.suspend("a");
    h.collect();
    assert_eq!(h.queued_of("a"), 0, "the waiters are closed at once");
    assert_eq!(h.scheduler.snapshot("a").queued, 0);
    let mut late = h.scheduler.enqueue("a", 1);
    assert_eq!(late.try_take().err(), Some(Closed));
    // Its running agent finishes; the slot goes to the other run.
    assert_eq!(h.release("a"), ["b1"]);
    h.scheduler.unsuspend("a");
    assert!(h.issue("a", 1, 4, 1).is_empty());
    assert_eq!(h.release("b"), ["a4"]);
}

#[test]
fn a_dropped_waiter_does_not_leak_a_slot() {
    let mut h = Harness::new(1);
    assert_eq!(h.issue("a", 1, 1, 1), ["a1"]);
    drop(h.scheduler.enqueue("b", 1));
    assert!(h.issue("c", 1, 1, 1).is_empty());
    assert_eq!(h.scheduler.snapshot("b").queued, 0);
    assert_eq!(h.release("a"), ["c1"]);
    assert_eq!(h.release("c"), Vec::<String>::new());
    assert_eq!(h.issue("d", 1, 1, 1), ["d1"], "the slot is free again");
}

#[test]
fn the_last_grant_is_kept_while_a_queue_is_empty() {
    let mut h = Harness::new(2);
    assert_eq!(h.issue("b", 2, 1, 1), ["b1"]);
    assert_eq!(h.issue("a", 2, 1, 1), ["a1"]);
    // A drained and refills; it does not jump ahead of B, served before it.
    assert!(h.issue("b", 2, 2, 1).is_empty());
    assert!(h.issue("a", 2, 2, 1).is_empty());
    assert_eq!(h.release("b"), ["b2"]);
    assert_eq!(h.release("a"), ["a2"]);
}

#[test]
fn never_granted_runs_go_first_by_eligibility_then_run_id() {
    let mut h = Harness::new(1);
    assert_eq!(h.issue("x", 1, 1, 2), ["x1"]);
    assert!(h.issue("b", 1, 1, 1).is_empty());
    assert!(h.issue("a", 1, 1, 1).is_empty());
    assert_eq!(h.release("x"), ["b1"], "b became eligible before a");
    assert_eq!(h.release("b"), ["a1"]);
    assert_eq!(h.release("a"), ["x2"]);
}

#[test]
fn a_removed_run_keeps_counting_its_slots_until_they_are_released() {
    let mut h = Harness::new(2);
    assert_eq!(h.issue("a", 2, 1, 3).len(), 2);
    h.scheduler.remove("a");
    h.collect();
    assert_eq!(h.queued_of("a"), 0);
    assert_eq!(h.scheduler.snapshot("a").running, 2);
    // A new attempt of the run still sees its two slots.
    assert!(h.issue("a", 2, 4, 1).is_empty());
    assert_eq!(h.release("a"), ["a4"]);
    h.scheduler.remove("a");
    h.release("a");
    h.release("a");
    assert_eq!(h.scheduler.snapshot("a"), RunSlots::default());
    assert_eq!(h.issue("b", 2, 1, 2).len(), 2);
}

/// xorshift64*: deterministic random sequences without a dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }
}

/// One random sequence: the waiting calls and granted slots of `runs` runs.
struct Sequence {
    seed: u64,
    capacity: usize,
    scheduler: WorkflowScheduler,
    waiting: Vec<VecDeque<Waiter>>,
    running: Vec<Slot>,
    /// Grants to other runs since each run's current oldest call started waiting.
    passed: Vec<usize>,
}

impl Sequence {
    fn new(seed: u64, runs: usize, capacity: usize) -> Self {
        Self {
            seed,
            capacity,
            scheduler: WorkflowScheduler::new(capacity),
            waiting: (0..runs).map(|_| VecDeque::new()).collect(),
            running: Vec::new(),
            passed: vec![0; runs],
        }
    }

    /// Apply one random change: an arrival, a release or a dropped call.
    fn change(&mut self, rng: &mut Rng) {
        let run = rng.below(self.waiting.len());
        match rng.below(10) {
            0..=4 => {
                if self.waiting[run].is_empty() {
                    self.passed[run] = 0;
                }
                let waiter = self.scheduler.enqueue(&format!("r{run}"), self.capacity);
                self.waiting[run].push_back(waiter);
            }
            5..=7 if !self.running.is_empty() => {
                drop(self.running.remove(rng.below(self.running.len())));
            }
            8 if !self.waiting[run].is_empty() => {
                let at = rng.below(self.waiting[run].len());
                self.waiting[run].remove(at);
                if at == 0 {
                    self.passed[run] = 0;
                }
            }
            _ => {}
        }
    }

    /// At most one grant follows one change, to the oldest call of its run.
    fn collect(&mut self) {
        let seed = self.seed;
        let mut granted = None;
        for (run, queue) in self.waiting.iter_mut().enumerate() {
            for (at, waiter) in queue.iter_mut().enumerate() {
                if let Ok(Some(slot)) = waiter.try_take() {
                    assert_eq!(at, 0, "seed {seed}: run r{run} granted out of order");
                    assert!(granted.is_none(), "seed {seed}: two grants for one change");
                    granted = Some((run, slot));
                }
            }
        }
        let Some((run, slot)) = granted else {
            return;
        };
        self.waiting[run].pop_front();
        self.running.push(slot);
        for (other, queue) in self.waiting.iter().enumerate() {
            if other != run && !queue.is_empty() {
                self.passed[other] += 1;
            }
        }
        self.passed[run] = 0;
    }

    fn check(&self) {
        let seed = self.seed;
        let runs = self.waiting.len();
        let waiting = self.waiting.iter().filter(|q| !q.is_empty()).count();
        assert!(
            self.running.len() == self.capacity || waiting == 0,
            "seed {seed}: {} of {} slots used while {waiting} runs wait",
            self.running.len(),
            self.capacity
        );
        for (run, queue) in self.waiting.iter().enumerate() {
            let passed = self.passed[run];
            assert!(
                queue.is_empty() || passed < runs,
                "seed {seed}: run r{run} passed over {passed} times with {runs} runs"
            );
        }
    }
}

/// Random arrivals, releases and dropped calls with up to 8 runs: no slot is idle while a
/// run waits, calls of a run start in issue order, and each run's oldest call is granted
/// within R grants.
#[test]
fn random_sequences_are_work_conserving_and_bounded() {
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let runs = 1 + rng.below(8);
        let capacity = 1 + rng.below(6);
        let mut sequence = Sequence::new(seed, runs, capacity);
        for _ in 0..400 {
            sequence.change(&mut rng);
            sequence.collect();
            sequence.check();
        }
    }
}

/// Every change to a run's queued or running count marks it changed for the progress
/// event: a call issued, granted, dropped while queued or released, and a suspension.
#[test]
fn queued_and_running_changes_mark_the_run_changed() {
    let mut h = Harness::new(1);
    assert!(h.scheduler.take_changed().is_empty());
    assert_eq!(h.issue("a", 1, 1, 2), ["a1"]);
    assert_eq!(h.scheduler.take_changed(), ["a"]);
    assert!(h.scheduler.take_changed().is_empty(), "taken once");
    // B's call waits; releasing A's slot grants A's second call.
    assert!(h.issue("b", 1, 1, 1).is_empty());
    assert_eq!(h.scheduler.take_changed(), ["b"]);
    assert_eq!(h.release("a"), ["b1"]);
    assert_eq!(h.scheduler.take_changed(), ["a", "b"]);
    // A queued call that is dropped leaves the queue.
    let (_, _, waiter) = h.pending.remove(0);
    drop(waiter);
    assert_eq!(h.scheduler.take_changed(), ["a"]);
    assert_eq!(h.scheduler.snapshot("a"), RunSlots::default());
    h.scheduler.suspend("b");
    assert_eq!(h.scheduler.take_changed(), ["b"]);
    // Releasing B's slot marks B; its waiter, dropped after the grant, marked nothing.
    h.scheduler.unsuspend("b");
    let _ = h.scheduler.take_changed();
    drop(h.running.remove(0));
    assert_eq!(h.scheduler.take_changed(), ["b"]);
}

#[tokio::test]
async fn a_change_wakes_the_waiting_consumer_even_when_marked_first() {
    let scheduler = WorkflowScheduler::new(1);
    let _waiter = scheduler.enqueue("a", 1);
    // Marked before anyone waits: the stored wake-up completes the next wait at once.
    tokio::time::timeout(std::time::Duration::from_secs(1), scheduler.changed())
        .await
        .expect("a stored wake-up");
    assert_eq!(scheduler.take_changed(), ["a"]);
}
