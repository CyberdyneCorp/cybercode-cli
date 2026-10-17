//! The fair scheduler of Workflow agent slots (`workflows` → Fair agent scheduling,
//! Concurrency limit).
//!
//! `workflows.max_concurrent` slots are shared by every run of a server. Each run has a
//! FIFO queue of issued `agent()` calls waiting for a slot. Whenever a slot is free, it
//! goes to the eligible run that least recently received one (a run never granted one
//! first), ties broken by the time the run became eligible and then by run ID. A run is
//! eligible while it is not suspended (paused, stopped or closing), has a live waiter and
//! runs fewer agents than its share. The scheduler is work-conserving: a slot is never
//! idle while some run is eligible, and a running agent is never preempted.
//!
//! Times are scheduler ticks, not wall time, so the state is a pure function of the calls
//! made and the unit tests drive it synchronously.
//!
//! Every change to a run's queued or running count marks the run changed, for the
//! `workflow.run.progress` live event (`workflows` → Run monitor).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll};

use tokio::sync::{Notify, oneshot};

/// Shared by every Workflow agent of a server; cheap to clone.
#[derive(Clone)]
pub struct WorkflowScheduler {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// Woken when a run is marked changed.
    changed: Notify,
}

struct State {
    capacity: usize,
    /// Slots granted and not yet released, over every run.
    total: usize,
    /// Advances on every grant and every time a run becomes eligible.
    tick: u64,
    runs: BTreeMap<String, RunQueue>,
    /// Runs whose queued or running count may have changed since the last
    /// [`WorkflowScheduler::take_changed`].
    changed: BTreeSet<String>,
}

struct RunQueue {
    /// Issued calls waiting for a slot, oldest first.
    waiting: VecDeque<oneshot::Sender<Slot>>,
    running: usize,
    share: usize,
    /// The tick of the run's latest grant; kept while its queue is empty.
    last_grant: Option<u64>,
    /// The tick at which the run last became eligible.
    eligible_since: u64,
    eligible: bool,
    /// Paused, stopped or closing: no grant, and new waiters are refused.
    suspended: bool,
    /// The attempt ended; the entry goes once its last slot is released.
    removed: bool,
}

/// The queued and running agents of one run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSlots {
    /// Issued calls waiting for a slot.
    pub queued: usize,
    pub running: usize,
}

/// A granted agent slot. Dropping it releases the slot to the next eligible run.
pub struct Slot {
    shared: Option<Arc<Shared>>,
    run_id: String,
}

/// The run's queue was cleared (suspended or removed) before a slot was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// One issued call waiting for a slot; dropping it leaves the queue.
pub struct Waiter {
    receiver: oneshot::Receiver<Slot>,
    /// The scheduler and run to mark changed when the call leaves the queue unserved.
    queued: Option<(Weak<Shared>, String)>,
}

impl RunQueue {
    fn new(share: usize) -> Self {
        Self {
            waiting: VecDeque::new(),
            running: 0,
            share,
            last_grant: None,
            eligible_since: 0,
            eligible: false,
            suspended: false,
            removed: false,
        }
    }

    /// Forget waiters whose call was dropped.
    fn prune(&mut self) {
        self.waiting.retain(|waiter| !waiter.is_closed());
    }

    /// Recompute eligibility after a change, stamping the tick when it begins.
    fn refresh(&mut self, tick: &mut u64) {
        self.prune();
        let eligible = !self.suspended && !self.waiting.is_empty() && self.running < self.share;
        if eligible && !self.eligible {
            *tick += 1;
            self.eligible_since = *tick;
        }
        self.eligible = eligible;
    }
}

impl State {
    fn lock(shared: &Shared) -> MutexGuard<'_, State> {
        shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn touch(&mut self, run_id: &str) {
        if !self.changed.contains(run_id) {
            self.changed.insert(run_id.to_owned());
        }
    }

    fn share(&self, share: usize) -> usize {
        share.clamp(1, self.capacity)
    }

    /// The eligible run that least recently received a grant.
    fn next(&mut self) -> Option<String> {
        let tick = &mut self.tick;
        self.runs
            .iter_mut()
            .filter_map(|(id, queue)| {
                queue.refresh(tick);
                queue
                    .eligible
                    .then_some((queue.last_grant, queue.eligible_since, id))
            })
            .min()
            .map(|(_, _, id)| id.clone())
    }

    /// Grant free slots until none is left or no run is eligible.
    fn dispatch(&mut self, shared: &Arc<Shared>) {
        while self.total < self.capacity {
            let Some(run_id) = self.next() else {
                return;
            };
            let queue = self.runs.get_mut(&run_id).expect("the next run exists");
            let waiter = queue.waiting.pop_front().expect("an eligible run waits");
            let previous = queue.last_grant;
            self.tick += 1;
            queue.running += 1;
            queue.last_grant = Some(self.tick);
            self.total += 1;
            self.changed.insert(run_id.clone());
            let slot = Slot {
                shared: Some(Arc::clone(shared)),
                run_id: run_id.clone(),
            };
            if let Err(mut slot) = waiter.send(slot) {
                // The call was dropped after the last prune: undo the grant.
                slot.shared = None;
                queue.running -= 1;
                queue.last_grant = previous;
                self.total -= 1;
            }
            queue.refresh(&mut self.tick);
        }
    }

    fn release(&mut self, run_id: &str) {
        self.total = self.total.saturating_sub(1);
        self.touch(run_id);
        let Some(queue) = self.runs.get_mut(run_id) else {
            return;
        };
        queue.running = queue.running.saturating_sub(1);
        if queue.removed && queue.running == 0 {
            self.runs.remove(run_id);
        } else {
            queue.refresh(&mut self.tick);
        }
    }
}

impl WorkflowScheduler {
    /// A scheduler of `capacity` slots (at least 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    capacity: capacity.max(1),
                    total: 0,
                    tick: 0,
                    runs: BTreeMap::new(),
                    changed: BTreeSet::new(),
                }),
                changed: Notify::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        State::lock(&self.shared)
    }

    /// Mark `run_id` changed and wake [`WorkflowScheduler::changed`].
    fn touched(&self, state: &mut State, run_id: &str) {
        state.touch(run_id);
        self.shared.changed.notify_one();
    }

    /// Runs whose queued or running count may have changed since the last call.
    pub fn take_changed(&self) -> Vec<String> {
        std::mem::take(&mut self.state().changed)
            .into_iter()
            .collect()
    }

    /// Completes once some run is marked changed (at once when one was marked since the
    /// last wait).
    pub async fn changed(&self) {
        self.shared.changed.notified().await;
    }

    /// The slots shared by every run.
    pub fn capacity(&self) -> usize {
        self.state().capacity
    }

    /// Queue one call of `run_id`, whose share is `share` (capped at the capacity). The
    /// waiter of a suspended run is closed at once.
    pub fn enqueue(&self, run_id: &str, share: usize) -> Waiter {
        let (sender, receiver) = oneshot::channel();
        let mut state = self.state();
        let share = state.share(share);
        let state = &mut *state;
        let queue = state
            .runs
            .entry(run_id.to_owned())
            .or_insert_with(|| RunQueue::new(share));
        // A new attempt of a run whose previous attempt still releases slots.
        queue.removed = false;
        queue.share = share;
        if queue.suspended {
            return Waiter {
                receiver,
                queued: None,
            };
        }
        queue.refresh(&mut state.tick);
        queue.waiting.push_back(sender);
        queue.refresh(&mut state.tick);
        state.dispatch(&self.shared);
        self.touched(state, run_id);
        Waiter {
            receiver,
            queued: Some((Arc::downgrade(&self.shared), run_id.to_owned())),
        }
    }

    /// Pause, stop or shutdown: drop the run's waiters at once and grant it nothing more
    /// until [`WorkflowScheduler::unsuspend`].
    pub fn suspend(&self, run_id: &str) {
        let mut state = self.state();
        let share = state.capacity;
        let state = &mut *state;
        let queue = state
            .runs
            .entry(run_id.to_owned())
            .or_insert_with(|| RunQueue::new(share));
        queue.suspended = true;
        queue.waiting.clear();
        queue.refresh(&mut state.tick);
        self.touched(state, run_id);
    }

    /// A new attempt of the run may be granted slots again.
    pub fn unsuspend(&self, run_id: &str) {
        let mut state = self.state();
        let state = &mut *state;
        let Some(queue) = state.runs.get_mut(run_id) else {
            return;
        };
        queue.suspended = false;
        queue.refresh(&mut state.tick);
        state.dispatch(&self.shared);
        self.touched(state, run_id);
    }

    /// The attempt ended: drop its waiters, and the run's entry once its last slot is
    /// released.
    pub fn remove(&self, run_id: &str) {
        let mut state = self.state();
        let state = &mut *state;
        let Some(queue) = state.runs.get_mut(run_id) else {
            return;
        };
        queue.waiting.clear();
        if queue.running == 0 {
            state.runs.remove(run_id);
        } else {
            queue.removed = true;
            queue.refresh(&mut state.tick);
        }
        self.touched(state, run_id);
    }

    /// Change the run's share (capped at the capacity).
    pub fn set_share(&self, run_id: &str, share: usize) {
        let mut state = self.state();
        let share = state.share(share);
        let state = &mut *state;
        let Some(queue) = state.runs.get_mut(run_id) else {
            return;
        };
        queue.share = share;
        queue.refresh(&mut state.tick);
        state.dispatch(&self.shared);
        self.touched(state, run_id);
    }

    /// The run's queued calls and running agents now.
    pub fn snapshot(&self, run_id: &str) -> RunSlots {
        let mut state = self.state();
        state
            .runs
            .get_mut(run_id)
            .map(|queue| {
                queue.prune();
                RunSlots {
                    queued: queue.waiting.len(),
                    running: queue.running,
                }
            })
            .unwrap_or_default()
    }
}

impl Slot {
    /// The run this slot was granted to.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else {
            return;
        };
        let mut state = State::lock(&shared);
        state.release(&self.run_id);
        state.dispatch(&shared);
        drop(state);
        shared.changed.notify_one();
    }
}

impl Waiter {
    /// The slot, when granted already; without waiting.
    pub fn try_take(&mut self) -> Result<Option<Slot>, Closed> {
        match self.receiver.try_recv() {
            Ok(slot) => {
                self.queued = None;
                Ok(Some(slot))
            }
            Err(oneshot::error::TryRecvError::Empty) => Ok(None),
            Err(oneshot::error::TryRecvError::Closed) => {
                self.queued = None;
                Err(Closed)
            }
        }
    }
}

impl Drop for Waiter {
    /// A call dropped while queued leaves its run's queue: its queued count changed.
    fn drop(&mut self) {
        let Some((shared, run_id)) = self.queued.take() else {
            return;
        };
        let Some(shared) = shared.upgrade() else {
            return;
        };
        State::lock(&shared).touch(&run_id);
        shared.changed.notify_one();
    }
}

impl Future for Waiter {
    type Output = Result<Slot, Closed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let polled = Pin::new(&mut self.receiver).poll(cx);
        if polled.is_ready() {
            self.queued = None;
        }
        polled.map(|result| result.map_err(|_| Closed))
    }
}

#[cfg(test)]
#[path = "workflow_scheduler_tests.rs"]
mod tests;
