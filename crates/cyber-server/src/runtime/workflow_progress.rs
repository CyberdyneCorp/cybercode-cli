//! The `workflow.run.progress` live event (`workflows` → Run monitor).
//!
//! A run is marked changed by any durable `workflow.*` event of it and by the scheduler
//! whenever its queued or running count moves. A changed run's progress is read again at
//! most every [`PROGRESS_INTERVAL_MS`], and published only when it differs from the last
//! one published, on the instance stream of the run's Location. After a `Lagged` gap every
//! run that may still move is read again, so the stream catches up with the list. The
//! event is non-durable: a client that reconnects resynchronizes with
//! `GET /workflows/runs`.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};

use super::workflow_summary::WorkflowRunProgress;
use super::{LiveEvent, Runtime};

/// The shortest time between two progress events of one run.
pub const PROGRESS_INTERVAL_MS: i64 = 250;

/// What the driver reads and publishes; the runtime in production, a fake in tests.
pub(super) trait ProgressSource {
    /// Runs that may still move: queued, running, waiting for input, or paused.
    fn unsettled(&self) -> Vec<String>;
    /// The run's progress now and its Location's directory; `None` when it is gone.
    fn progress(&self, run_id: &str) -> Option<(String, WorkflowRunProgress)>;
    fn publish(&self, directory: String, progress: WorkflowRunProgress);
}

#[derive(Default)]
struct Track {
    sent: Option<WorkflowRunProgress>,
    sent_at: Option<i64>,
    changed: bool,
}

/// Rate-bounded, change-only progress per run, over an explicit clock.
#[derive(Default)]
pub(super) struct Driver {
    runs: HashMap<String, Track>,
}

impl Driver {
    pub(super) fn mark(&mut self, run_id: &str) {
        self.runs.entry(run_id.to_owned()).or_default().changed = true;
    }

    /// Follow one bus message; false once the bus closed.
    pub(super) fn observe(
        &mut self,
        received: Result<LiveEvent, RecvError>,
        source: &dyn ProgressSource,
    ) -> bool {
        match received {
            Ok(LiveEvent::Durable {
                session_id, kind, ..
            }) if kind.starts_with("workflow.") => self.mark(&session_id),
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => self.resync(source),
            Err(RecvError::Closed) => return false,
        }
        true
    }

    /// Events were missed: read every run again that may have moved meanwhile.
    fn resync(&mut self, source: &dyn ProgressSource) {
        let tracked: Vec<String> = self.runs.keys().cloned().collect();
        for id in source.unsettled().into_iter().chain(tracked) {
            self.mark(&id);
        }
    }

    fn due(track: &Track, now: i64) -> bool {
        track.changed
            && track
                .sent_at
                .is_none_or(|at| now.saturating_sub(at) >= PROGRESS_INTERVAL_MS)
    }

    /// Publish the progress of every changed run whose interval has passed, when it
    /// differs from the last published.
    pub(super) fn flush(&mut self, now: i64, source: &dyn ProgressSource) {
        let due: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, track)| Self::due(track, now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some((directory, progress)) = source.progress(&id) else {
                self.runs.remove(&id);
                continue;
            };
            let track = self.runs.get_mut(&id).expect("a due run is tracked");
            track.changed = false;
            if track.sent.as_ref() != Some(&progress) {
                track.sent = Some(progress.clone());
                track.sent_at = Some(now);
                source.publish(directory, progress);
            }
        }
    }

    /// When the next changed run becomes due, after a flush at `now`.
    pub(super) fn next_due(&self) -> Option<i64> {
        self.runs
            .values()
            .filter(|track| track.changed)
            .map(|track| {
                track
                    .sent_at
                    .map_or(i64::MIN, |at| at + PROGRESS_INTERVAL_MS)
            })
            .min()
    }
}

impl ProgressSource for Runtime {
    fn unsettled(&self) -> Vec<String> {
        self.unsettled_workflow_runs().unwrap_or_default()
    }

    fn progress(&self, run_id: &str) -> Option<(String, WorkflowRunProgress)> {
        self.workflow_run_progress(run_id).ok().flatten()
    }

    fn publish(&self, directory: String, progress: WorkflowRunProgress) {
        self.inner.bus.publish(LiveEvent::WorkflowProgress {
            directory,
            progress,
        });
    }
}

impl Runtime {
    /// Start publishing `workflow.run.progress` once, from the first run this runtime
    /// starts, resumes or recovers. The task ends with the runtime.
    pub(super) fn ensure_workflow_progress(&self) {
        if self.inner.workflow_progress.set(()).is_err() {
            return;
        }
        let (runtime, live) = (self.downgrade(), self.subscribe());
        let (scheduler, closed) = (self.workflow_scheduler(), self.inner.closed.clone());
        tokio::spawn(async move {
            tokio::select! {
                () = closed.cancelled() => {}
                () = follow(runtime, live, scheduler) => {}
            }
        });
    }
}

async fn follow(
    runtime: super::WeakRuntime,
    mut live: broadcast::Receiver<LiveEvent>,
    scheduler: super::WorkflowScheduler,
) {
    let mut driver = Driver::default();
    loop {
        let wait = driver.next_due().map(|at| {
            let ms = at.saturating_sub(now_ms()).clamp(0, PROGRESS_INTERVAL_MS);
            Duration::from_millis(ms as u64)
        });
        tokio::select! {
            received = live.recv() => {
                let Some(runtime) = runtime.upgrade() else { return };
                if !driver.observe(received, &runtime) {
                    return;
                }
            }
            () = scheduler.changed() => {
                for id in scheduler.take_changed() {
                    driver.mark(&id);
                }
            }
            () = tokio::time::sleep(wait.unwrap_or_default()), if wait.is_some() => {}
        }
        let Some(runtime) = runtime.upgrade() else {
            return;
        };
        driver.flush(now_ms(), &runtime);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
#[path = "workflow_progress_tests.rs"]
mod tests;
