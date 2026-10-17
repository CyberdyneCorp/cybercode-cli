//! The run queue (`workflows` → Run queue): at most `workflows.max_running_runs` runs hold
//! an execution attempt at once. A run takes a [`RunSlot`] when it is dispatched and keeps
//! it until its attempt's engine and every agent task have returned, which is when its
//! `Control` stops being busy. A run started or resumed while every slot is taken waits
//! here, `queued`, with the engine it will execute; dispatch takes the oldest by the time
//! it entered the queue, then by run ID. A run refused a slot holds its place from that
//! moment ([`Reservation`]), while its start or resume is still being recorded: no later
//! start takes a slot ahead of it, and dispatch waits for it rather than skipping it.
//!
//! The order is durable (`queued_ms` on the run), so positions are computed from the store
//! on read; this process-local queue only holds what dispatch needs to launch. Its mutex
//! is short and never held across an await or while a run lock is taken.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::workflow_replay::Replay;
use super::workflows::Engine;

/// `workflows.max_running_runs` when unset.
pub(super) const DEFAULT_MAX_RUNNING_RUNS: usize = 8;

/// What a queued run executes once dispatched.
pub(super) struct Pending {
    pub(super) engine: Engine,
    pub(super) replay: Replay,
}

/// The process-local run queue; cheap to clone.
#[derive(Clone)]
pub(super) struct WorkflowQueue {
    state: Arc<Mutex<State>>,
}

struct State {
    capacity: usize,
    /// Slots held by dispatched runs.
    running: usize,
    /// Queued runs by `(queued_ms, run_id)`; `None` is a place held by a [`Reservation`].
    waiting: BTreeMap<(i64, String), Option<Pending>>,
}

/// Whether a run starting or resuming now got a slot or must queue.
pub(super) enum Admission {
    Slot(RunSlot),
    Queued(Reservation),
}

/// A queued run's place, held from the moment it was refused a slot until it is filled
/// with what the run executes. Dropped unfilled (its start or resume failed), the place is
/// given up and `on_cancel` runs, to dispatch the runs it held back.
pub(super) struct Reservation {
    queue: WorkflowQueue,
    key: (i64, String),
    filled: bool,
    on_cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl Reservation {
    /// Run `f` when the reservation is given up unfilled.
    pub(super) fn on_cancel(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.on_cancel = Some(Box::new(f));
        self
    }

    /// When the run entered the queue.
    pub(super) fn queued_ms(&self) -> i64 {
        self.key.0
    }

    /// Queue the run at its place with what it executes.
    pub(super) fn fill(mut self, pending: Pending) {
        self.filled = true;
        self.queue.push(&self.key.1, self.key.0, pending);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.filled {
            return;
        }
        {
            let mut state = self.queue.lock();
            if matches!(state.waiting.get(&self.key), Some(None)) {
                state.waiting.remove(&self.key);
            }
        }
        if let Some(f) = self.on_cancel.take() {
            f();
        }
    }
}

/// One run slot, released when dropped; `on_release` then runs, to dispatch the next run.
pub(super) struct RunSlot {
    queue: WorkflowQueue,
    on_release: Option<Box<dyn FnOnce() + Send>>,
}

impl RunSlot {
    /// Run `f` after the slot is released.
    pub(super) fn on_release(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.on_release = Some(Box::new(f));
        self
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        self.queue.lock().running -= 1;
        if let Some(f) = self.on_release.take() {
            f();
        }
    }
}

impl WorkflowQueue {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                capacity: capacity.max(1),
                running: 0,
                waiting: BTreeMap::new(),
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn slot(&self) -> RunSlot {
        RunSlot {
            queue: self.clone(),
            on_release: None,
        }
    }

    /// A slot for a run starting or resuming now, when one is free and no queued run is
    /// ahead of it; `None` means the run must queue.
    #[cfg(test)]
    fn try_admit(&self) -> Option<RunSlot> {
        let mut state = self.lock();
        Self::take_free_slot(&mut state).then(|| self.slot())
    }

    /// Count a slot taken when one is free and no queued run is ahead.
    fn take_free_slot(state: &mut State) -> bool {
        if !state.waiting.is_empty() || state.running >= state.capacity {
            return false;
        }
        state.running += 1;
        true
    }

    /// A slot for run `run_id` starting or resuming now, as [`Self::try_admit`]; else its
    /// place in the queue, entered at `queued_ms`, is held at once, under the same lock.
    pub(super) fn admit_or_reserve(&self, run_id: &str, queued_ms: i64) -> Admission {
        let mut state = self.lock();
        if Self::take_free_slot(&mut state) {
            drop(state);
            return Admission::Slot(self.slot());
        }
        let key = (queued_ms, run_id.to_owned());
        state.waiting.retain(|(_, id), _| id != run_id);
        state.waiting.insert(key.clone(), None);
        drop(state);
        Admission::Queued(Reservation {
            queue: self.clone(),
            key,
            filled: false,
            on_cancel: None,
        })
    }

    /// Queue a run that entered the queue at `queued_ms`, replacing what it was queued
    /// with before: a run is queued once, with what its latest start or resume validated.
    pub(super) fn push(&self, run_id: &str, queued_ms: i64, pending: Pending) {
        let mut state = self.lock();
        state.waiting.retain(|(_, id), _| id != run_id);
        state
            .waiting
            .insert((queued_ms, run_id.to_owned()), Some(pending));
    }

    /// The oldest queued run with a slot for it, while a slot is free. A reservation at
    /// the head holds dispatch back until it is filled or given up.
    pub(super) fn next(&self) -> Option<(String, Pending, RunSlot)> {
        let mut state = self.lock();
        if state.running >= state.capacity {
            return None;
        }
        let head = state.waiting.first_entry()?;
        head.get().as_ref()?;
        let ((_, run_id), pending) = head.remove_entry();
        let pending = pending.expect("the head is filled");
        state.running += 1;
        drop(state);
        Some((run_id, pending, self.slot()))
    }

    /// Drop a queued run (paused or stopped while queued); true when it was queued here.
    pub(super) fn remove(&self, run_id: &str) -> bool {
        let mut state = self.lock();
        let key = state.waiting.keys().find(|(_, id)| id == run_id).cloned();
        key.is_some_and(|key| state.waiting.remove(&key).is_some())
    }

    /// Runs queued here, oldest first.
    #[cfg(test)]
    fn queued(&self) -> Vec<String> {
        self.lock()
            .waiting
            .keys()
            .map(|(_, id)| id.clone())
            .collect()
    }
}

#[cfg(test)]
#[path = "workflow_queue_tests.rs"]
mod tests;
