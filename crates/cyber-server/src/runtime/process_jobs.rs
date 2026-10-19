//! Process Jobs (`background-tasks` → Background bash, Job registry, Concurrency limits):
//! a command the tool layer started, owned by the runtime past its Turn, with a durable
//! record, a log file, live `job.output.1` and a completion notice.
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};

use futures::FutureExt;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::jobs::{CANCELLED, Control, ENDED, STARTED};
use super::{Bus, Job, JobStatus, LiveEvent, Runtime, RuntimeError, WeakRuntime};

/// Kinds with their own caps, not counted against `background.max_jobs`.
const OWN_CAPS: &[&str] = &["subagent", "workflow"];
/// Bytes of a log a completion notice reads to find its last lines.
const TAIL_BYTES: u64 = 64 * 1024;

/// What a process Job is, decided by the tool that starts it.
pub struct ProcessJob {
    pub session_id: String,
    /// `bash`, `powershell` or `monitor`.
    pub kind: String,
    pub command: String,
    pub description: String,
    /// `false` suppresses the completion notice (`notify: false`).
    pub notify: bool,
    /// The log directory, `<data>/jobs`.
    pub output_dir: PathBuf,
    /// `background.max_jobs` of the Session's Location.
    pub max_jobs: usize,
}

/// An admitted process Job, counted against the limit until it starts or is dropped.
/// Session deletion and other starts wait while it is held.
pub struct ProcessReservation {
    id: String,
    output_path: PathBuf,
    spec: ProcessJob,
    runtime: WeakRuntime,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl ProcessReservation {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `<data>/jobs/<job_id>.log`.
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

/// What a running process Job's owner gives its work.
pub struct JobIo {
    /// Cancelled when the Job is stopped, its Session deleted or the server shuts down;
    /// the work then stops its process tree and returns.
    pub stop: CancellationToken,
    bus: Bus,
    runtime: WeakRuntime,
    session_id: String,
    job_id: String,
}

impl JobIo {
    /// The Job's ID.
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    /// Admit `text` into the Job's Session as a queued prompt from `source` (a monitor's
    /// line uses `monitor`), so it never interrupts a running Turn. It carries the chain
    /// hops of the run that started the Job.
    pub async fn queue(&self, source: &str, text: String) -> Result<(), String> {
        let runtime = self.runtime.upgrade().ok_or("The server is stopping")?;
        let mut admission = super::Admission::text(text, super::Delivery::Queue);
        admission.source = source.into();
        let hops = runtime
            .job_message_hops(&self.session_id, &self.job_id)
            .await;
        runtime
            .admit_caused(&self.session_id, admission, hops)
            .await
            .map(drop)
            .map_err(|error| error.to_string())
    }

    /// Publish live output (`job.output.1`); it is not durable.
    pub fn output(&self, text: String) {
        self.bus.publish(LiveEvent::JobOutput {
            session_id: self.session_id.clone(),
            job_id: self.job_id.clone(),
            text,
        });
    }
}

/// The exit code of the process (`None` when a signal ended it), or why it failed.
pub type ProcessWork = BoxFuture<'static, Result<Option<i32>, String>>;

impl Runtime {
    /// Admit a process Job for `spec.session_id`: the Session must exist and not be
    /// deleting, and fewer than `max_jobs` of its process Jobs may be running.
    pub async fn reserve_process_job(
        &self,
        spec: ProcessJob,
    ) -> Result<ProcessReservation, RuntimeError> {
        let guard = self.inner.job_admission.clone().lock_owned().await;
        if self.is_shutting_down() {
            return Err(RuntimeError::ShuttingDown);
        }
        if self.is_deleting(&spec.session_id) {
            return Err(RuntimeError::Busy(spec.session_id.clone()));
        }
        self.inner.handle(&spec.session_id).await?;
        if self.running_process_jobs(&spec.session_id)? >= spec.max_jobs {
            return Err(RuntimeError::Invalid(format!(
                "Background job limit reached ({})",
                spec.max_jobs
            )));
        }
        let id = cyber_core::ids::new_id("job");
        Ok(ProcessReservation {
            output_path: spec.output_dir.join(format!("{id}.log")),
            id,
            spec,
            runtime: self.downgrade(),
            _guard: guard,
        })
    }

    fn running_process_jobs(&self, session: &str) -> Result<usize, RuntimeError> {
        let owned = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(self
            .jobs(Some(session))?
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .filter(|job| !OWN_CAPS.contains(&job.kind.as_str()))
            .filter(|job| owned.contains_key(&job.id))
            .count())
    }

    /// Record the Job and hand `work` to a runtime-owned task. The Job keeps running after
    /// the Turn that started it, and an interrupt never stops it.
    pub async fn start_process_job(
        &self,
        reservation: ProcessReservation,
        work: Box<dyn FnOnce(JobIo) -> ProcessWork + Send>,
    ) -> Result<Job, RuntimeError> {
        if !std::sync::Weak::ptr_eq(&reservation.runtime.inner, &self.downgrade().inner) {
            return Err(RuntimeError::Invalid(
                "Background admission does not belong to this runtime".into(),
            ));
        }
        let _open = self.inner.open().await?;
        let spec = &reservation.spec;
        let job = Job {
            id: reservation.id.clone(),
            session_id: spec.session_id.clone(),
            child_id: String::new(),
            name: reservation.id.clone(),
            kind: spec.kind.clone(),
            command: Some(spec.command.clone()),
            description: spec.description.clone(),
            status: JobStatus::Running,
            started_ms: chrono::Utc::now().timestamp_millis(),
            ended_ms: None,
            exit_code: None,
            output_path: Some(reservation.output_path.display().to_string()),
            result: None,
            error: None,
            cost: 0.0,
            unpriced: false,
            tokens: 0,
            // A suppressed notice is never delivered, not even after a restart.
            notified: !spec.notify,
        };
        let chain = self.job_chain(&job.session_id, &job.id);
        self.record_job_with(STARTED, &job, chain).await?;
        let control = Arc::new(Control {
            stop: CancellationToken::new(),
            done: CancellationToken::new(),
            notify: AtomicBool::new(true),
        });
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job.id.clone(), Arc::clone(&control));
        let io = JobIo {
            stop: CancellationToken::new(),
            bus: self.inner.bus.clone(),
            runtime: self.downgrade(),
            session_id: job.session_id.clone(),
            job_id: job.id.clone(),
        };
        let owner = tokio::spawn(own(self.downgrade(), job.clone(), control, work, io));
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(owner);
        Ok(job)
    }

    async fn finish_process_job(
        &self,
        mut job: Job,
        status: JobStatus,
        exit: Result<Option<i32>, String>,
        notify: bool,
    ) -> Result<Job, RuntimeError> {
        job.status = status;
        job.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        match exit {
            Ok(code) => job.exit_code = code,
            Err(error) => job.error = Some(error),
        }
        let deliver = notify && !job.notified;
        // A deleted Session's notice is suppressed for good.
        job.notified |= !notify;
        let kind = if status == JobStatus::Cancelled {
            CANCELLED
        } else {
            ENDED
        };
        self.record_job(kind, &job).await?;
        if deliver && !self.is_shutting_down() {
            self.notify_job(job.clone()).await?;
        }
        Ok(job)
    }
}

/// The owner task: run `work` until it exits or is stopped, then settle the record,
/// the notice and the observational hooks.
async fn own(
    weak: WeakRuntime,
    job: Job,
    control: Arc<Control>,
    work: Box<dyn FnOnce(JobIo) -> ProcessWork + Send>,
    io: JobIo,
) {
    let closed = match weak.upgrade() {
        Some(runtime) => runtime.inner.closed.clone(),
        None => CancellationToken::new(),
    };
    let (status, exit) = run(&control, &closed, work, io).await;
    let notify = control.notify.load(Ordering::SeqCst);
    let mut settled = None;
    if let Some(runtime) = weak.upgrade() {
        match runtime
            .finish_process_job(job.clone(), status, exit, notify)
            .await
        {
            Ok(job) => settled = Some(job),
            Err(error) => runtime.inner.bus.publish(LiveEvent::Error {
                session_id: job.session_id.clone(),
                kind: "background_settlement".into(),
                message: error.to_string(),
            }),
        }
        runtime
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&job.id);
    }
    control.done.cancel();
    // Observational hooks run after settlement, so waiters never block on them. A
    // deleted Session observes nothing.
    if notify
        && let Some(job) = settled
        && let Some(runtime) = weak.upgrade()
    {
        runtime.inner.task_completed(&job).await;
        runtime.inner.job_ended(&job).await;
    }
}

/// Run `work` to its exit, or stop it when the Job is cancelled or the server stops.
async fn run(
    control: &Control,
    closed: &CancellationToken,
    work: Box<dyn FnOnce(JobIo) -> ProcessWork + Send>,
    io: JobIo,
) -> (JobStatus, Result<Option<i32>, String>) {
    let halt = io.stop.clone();
    let mut work = std::panic::AssertUnwindSafe(work(io)).catch_unwind();
    tokio::select! {
        biased;
        result = &mut work => {
            let exit = result
                .unwrap_or_else(|_| Err("Background owner stopped on an internal error".into()));
            return (exited(&exit), exit);
        }
        _ = control.stop.cancelled() => {}
        _ = closed.cancelled() => {}
    }
    // The work stops its process tree (TERM, then KILL after 5 s) and returns.
    halt.cancel();
    let exit = work.await.unwrap_or(Ok(None));
    let status = if control.stop.is_cancelled() {
        JobStatus::Cancelled
    } else {
        JobStatus::Interrupted
    };
    (status, exit)
}

/// `completed` for exit code 0, `error` otherwise.
fn exited(exit: &Result<Option<i32>, String>) -> JobStatus {
    if matches!(exit, Ok(Some(0))) {
        JobStatus::Completed
    } else {
        JobStatus::Error
    }
}

/// The text near the end of a process Job's log, for its completion notice.
pub(super) fn log_tail(job: &Job) -> String {
    let Some(path) = job.output_path.as_deref() else {
        return String::new();
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map_or(0, |meta| meta.len());
    let mut bytes = Vec::new();
    let read = file
        .seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES)))
        .and_then(|_| file.read_to_end(&mut bytes));
    if read.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
