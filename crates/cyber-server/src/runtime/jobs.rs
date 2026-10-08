//! Process-local background ownership, durable registry and idempotent queue notices.
use std::sync::{Arc, PoisonError};

use cyber_store::{EventRegistry, StoredEvent};
use futures::{FutureExt, future::BoxFuture};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{Admission, Delivery, LiveEvent, Runtime, RuntimeError, WeakRuntime};

const STARTED: &str = "job.started.1";
const ENDED: &str = "job.ended.1";
const CANCELLED: &str = "job.cancelled.1";
const NOTIFIED: &str = "job.notified.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Completed,
    Error,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Job {
    pub id: String,
    pub session_id: String,
    pub child_id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub status: JobStatus,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub output_path: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Known cost lower bound; unpriced marks an unknown total.
    pub cost: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unpriced: bool,
    pub tokens: u64,
    pub notified: bool,
}

/// Usage accumulated before an attempt, kept independently of its public Job snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobUsage {
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    unpriced_steps: u32,
    #[serde(default)]
    children: Option<super::ChildrenUsage>,
}
impl Default for JobUsage {
    fn default() -> Self {
        Self {
            cost: 0.0,
            tokens: 0,
            unpriced_steps: 0,
            children: Some(super::ChildrenUsage {
                children_usage_complete: true,
                children_token_classes_complete: true,
                ..Default::default()
            }),
        }
    }
}
impl JobUsage {
    /// Own usage only. Use Runtime::job_usage for a complete subtree baseline.
    pub fn of(state: &super::SessionState) -> Self {
        Self {
            cost: state.totals.cost,
            tokens: state.totals.usage.context_tokens()
                + state.totals.usage.output
                + state.totals.usage.reasoning,
            unpriced_steps: state.totals.unpriced_steps,
            children: None,
        }
    }
}
pub struct JobAttempt {
    pub name: String,
    pub description: String,
    pub usage: JobUsage,
}

/// Fences child creation/registration against Session deletion.
pub struct JobAdmission {
    runtime: WeakRuntime,
    parent: String,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

pub(super) struct Control {
    stop: CancellationToken,
    done: CancellationToken,
    notify: std::sync::atomic::AtomicBool,
}

pub(super) fn register(registry: &mut EventRegistry) {
    for kind in [STARTED, ENDED, CANCELLED, NOTIFIED] {
        registry.register(kind).expect("valid job event");
    }
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    if ![STARTED, ENDED, CANCELLED, NOTIFIED].contains(&event.kind.as_str()) {
        return Ok(());
    }
    let data = &event.data;
    if event.kind == STARTED {
        tx.execute(
            "INSERT INTO job (id,session_id,child_id,name,status,data,usage_baseline) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                data["id"].as_str(),
                event.aggregate_id,
                data["child_id"].as_str(),
                data["name"].as_str(),
                data["status"].as_str(),
                data.to_string(),
                data.get("usage_baseline").cloned().unwrap_or_else(|| serde_json::json!({})).to_string()
            ],
        )?;
    } else {
        tx.execute(
            "UPDATE job SET status=?2,data=?3 WHERE id=?1 AND session_id=?4",
            params![
                data["id"].as_str(),
                data["status"].as_str(),
                data.to_string(),
                event.aggregate_id
            ],
        )?;
    }
    Ok(())
}

impl WeakRuntime {
    /// Subscribe before checking running state; do not retain a runtime owner while waiting.
    pub async fn wait_idle(&self, id: &str) -> Result<(), RuntimeError> {
        let mut events = self
            .upgrade()
            .ok_or(RuntimeError::ShuttingDown)?
            .subscribe();
        loop {
            let runtime = self.upgrade().ok_or(RuntimeError::ShuttingDown)?;
            if !runtime.is_running(id) {
                return Ok(());
            }
            drop(runtime);
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err(RuntimeError::ShuttingDown);
                }
            }
        }
    }
}

impl Runtime {
    /// Read own and descendant counters from one durable database snapshot.
    pub fn job_usage(&self, id: &str) -> Result<JobUsage, RuntimeError> {
        let id = id.to_owned();
        self.inner.store.read(move |db| {
            db.query_row(
                "SELECT cost,input_tokens+output_tokens+reasoning_tokens+cache_read_tokens+cache_write_tokens,unpriced_steps,children_cost,children_tokens,children_unpriced_steps,children_usage_complete,children_token_classes,children_token_classes_complete FROM session WHERE id=?1",
                [id],
                |row| Ok(JobUsage {
                    cost: row.get(0)?,
                    tokens: row.get::<_, i64>(1)? as u64,
                    unpriced_steps: row.get(2)?,
                    children: Some(super::child_usage::read(row, 3)?),
                }),
            ).map_err(Into::into)
        }).map_err(Into::into)
    }
    pub fn jobs(&self, session_id: Option<&str>) -> Result<Vec<Job>, RuntimeError> {
        let session = session_id.map(str::to_string);
        let rows = self.inner.store.read(move |conn| {
            let mut statement = conn
                .prepare("SELECT data FROM job WHERE (?1 IS NULL OR session_id=?1) ORDER BY id")?;
            let rows = statement.query_map([session], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        rows.into_iter()
            .map(|data| {
                serde_json::from_str(&data).map_err(|e| RuntimeError::Corrupt(e.to_string()))
            })
            .collect()
    }

    pub fn find_job(&self, id: &str) -> Result<Option<Job>, RuntimeError> {
        let id = id.to_string();
        let data = self.inner.store.read(move |conn| {
            Ok(conn
                .query_row("SELECT data FROM job WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?)
        })?;
        data.map(|data| {
            serde_json::from_str(&data).map_err(|e| RuntimeError::Corrupt(e.to_string()))
        })
        .transpose()
    }

    pub fn job(&self, id: &str) -> Result<Job, RuntimeError> {
        self.find_job(id)?
            .ok_or_else(|| RuntimeError::Invalid(format!("Unknown job: {id}")))
    }

    pub fn jobs_page(
        &self,
        session: Option<&str>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<(Vec<Job>, Option<String>), RuntimeError> {
        if !(1..=200).contains(&limit) {
            return Err(RuntimeError::Invalid("limit must be 1–200".into()));
        }
        let session = session.map(str::to_string);
        let cursor = cursor.map(str::to_string);
        let rows=self.inner.store.read(move |conn| {
            let mut statement=conn.prepare("SELECT data FROM job WHERE (?1 IS NULL OR session_id=?1) AND (?2 IS NULL OR id<?2) ORDER BY id DESC LIMIT ?3")?;
            let rows=statement.query_map(params![session,cursor,limit+1],|row| row.get::<_,String>(0))?;
            Ok(rows.collect::<Result<Vec<_>,_>>()?)
        })?;
        let mut jobs: Vec<Job> = rows
            .into_iter()
            .map(|data| {
                serde_json::from_str(&data).map_err(|e| RuntimeError::Corrupt(e.to_string()))
            })
            .collect::<Result<_, _>>()?;
        let more = jobs.len() > limit as usize;
        jobs.truncate(limit as usize);
        let next = if more {
            jobs.last().map(|job| job.id.clone())
        } else {
            None
        };
        Ok((jobs, next))
    }

    pub async fn reserve_child_job(&self, parent: &str) -> Result<JobAdmission, RuntimeError> {
        let guard = self.inner.job_admission.clone().lock_owned().await;
        if self.is_shutting_down() {
            return Err(RuntimeError::ShuttingDown);
        }
        self.state(parent).await?;
        Ok(JobAdmission {
            runtime: self.downgrade(),
            parent: parent.into(),
            _guard: guard,
        })
    }

    /// Transfers an already-created child to a process-local owner after durable registration.
    pub async fn start_child_job(
        &self,
        parent: &str,
        child: &str,
        name: String,
        description: String,
        reservation: Option<JobAdmission>,
        work: BoxFuture<'static, Result<Value, String>>,
    ) -> Result<Job, RuntimeError> {
        self.start_child_job_attempt(
            parent,
            child,
            JobAttempt {
                name,
                description,
                usage: JobUsage::default(),
            },
            reservation,
            work,
        )
        .await
    }

    pub async fn start_child_job_attempt(
        &self,
        parent: &str,
        child: &str,
        attempt: JobAttempt,
        reservation: Option<JobAdmission>,
        work: BoxFuture<'static, Result<Value, String>>,
    ) -> Result<Job, RuntimeError> {
        let JobAttempt {
            name,
            description,
            usage,
        } = attempt;
        let _reservation = match reservation {
            Some(reservation) => {
                if reservation.parent != parent
                    || !std::sync::Weak::ptr_eq(&reservation.runtime.inner, &self.downgrade().inner)
                {
                    return Err(RuntimeError::Invalid(
                        "Background admission does not belong to this parent/runtime".into(),
                    ));
                }
                reservation
            }
            None => self.reserve_child_job(parent).await?,
        };
        let _open = self.inner.open().await?;
        let child_info = self.state(child).await?.info;
        if child_info.parent_id.as_deref() != Some(parent) {
            return Err(RuntimeError::Invalid(
                "Background child does not belong to parent".into(),
            ));
        }
        let job = Job {
            id: cyber_core::ids::new_id("job"),
            session_id: parent.into(),
            child_id: child.into(),
            name,
            kind: "subagent".into(),
            description,
            status: JobStatus::Running,
            started_ms: chrono::Utc::now().timestamp_millis(),
            ended_ms: None,
            exit_code: None,
            output_path: None,
            result: None,
            error: None,
            cost: 0.0,
            unpriced: false,
            tokens: 0,
            notified: false,
        };
        let mut started = serde_json::to_value(&job).expect("job serializes");
        started["usage_baseline"] = serde_json::to_value(usage).expect("usage serializes");
        let handle = self.inner.handle(parent).await?;
        self.inner
            .commit(&handle, vec![super::events::event(STARTED, &started)])
            .await?;
        let control = Arc::new(Control {
            stop: CancellationToken::new(),
            done: CancellationToken::new(),
            notify: std::sync::atomic::AtomicBool::new(true),
        });
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job.id.clone(), control.clone());
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let running = job.clone();
        let owner = tokio::spawn(async move {
            let work = std::panic::AssertUnwindSafe(work).catch_unwind();
            let (status, result) = tokio::select! {
                biased;
                _ = closed.cancelled() => (JobStatus::Interrupted, Err("Server stopped before background completion".into())),
                _ = control.stop.cancelled() => (JobStatus::Cancelled, Err("Background task cancelled".into())),
                result = work => { let result = result.unwrap_or_else(|_| Err("Background owner stopped on an internal error".into())); (if result.is_ok() {JobStatus::Completed} else {JobStatus::Error}, result) },
            };
            if let Some(runtime) = weak.upgrade() {
                if status != JobStatus::Completed {
                    let _ = runtime.interrupt(&running.child_id).await;
                }
                if let Err(error) = runtime
                    .finish_job(
                        running.clone(),
                        status,
                        result,
                        control.notify.load(std::sync::atomic::Ordering::SeqCst),
                    )
                    .await
                {
                    runtime.inner.bus.publish(LiveEvent::Error {
                        session_id: running.session_id.clone(),
                        kind: "background_settlement".into(),
                        message: error.to_string(),
                    });
                }
                runtime
                    .inner
                    .jobs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&running.id);
            }
            control.done.cancel();
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(owner);
        Ok(job)
    }

    async fn record_job(&self, kind: &str, job: &Job) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(&job.session_id).await?;
        self.inner
            .commit(&handle, vec![super::events::event(kind, job)])
            .await?;
        Ok(())
    }

    async fn finish_job(
        &self,
        mut job: Job,
        status: JobStatus,
        result: Result<Value, String>,
        notify: bool,
    ) -> Result<(), RuntimeError> {
        if status != JobStatus::Completed {
            self.inner.abandon_requests(&job.child_id).await?;
        }
        job.status = status;
        job.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        match result {
            Ok(value) => {
                job.output_path = value["output_file"].as_str().map(str::to_string);
                job.result = Some(value);
            }
            Err(error) => job.error = Some(error),
        }
        self.refresh_job_usage(&mut job).await;
        // Suppressed notices (Session deletion) must not be resurrected by recovery.
        job.notified = !notify;
        self.record_job(
            if status == JobStatus::Cancelled {
                CANCELLED
            } else {
                ENDED
            },
            &job,
        )
        .await?;
        if notify && !self.is_shutting_down() {
            self.notify_job(job).await?;
        }
        Ok(())
    }

    async fn refresh_job_usage(&self, job: &mut Job) {
        let id = job.id.clone();
        let baseline = self
            .inner
            .store
            .read(move |db| {
                Ok(
                    db.query_row("SELECT usage_baseline FROM job WHERE id=?1", [id], |row| {
                        row.get::<_, String>(0)
                    })?,
                )
            })
            .ok()
            .and_then(|data| serde_json::from_str::<JobUsage>(&data).ok());
        match (self.job_usage(&job.child_id), baseline) {
            (Ok(current), Some(baseline)) => {
                job.cost = (current.cost - baseline.cost).max(0.0);
                job.tokens = current.tokens.saturating_sub(baseline.tokens);
                job.unpriced = current.unpriced_steps > baseline.unpriced_steps
                    || current.cost < baseline.cost
                    || current.tokens < baseline.tokens;
                match (current.children, baseline.children) {
                    (Some(current), Some(baseline)) => {
                        job.cost += (current.children_cost - baseline.children_cost).max(0.0);
                        job.tokens = job.tokens.saturating_add(
                            current
                                .children_tokens
                                .saturating_sub(baseline.children_tokens),
                        );
                        job.unpriced |= !current.children_usage_complete
                            || !baseline.children_usage_complete
                            || current.children_unpriced_steps > baseline.children_unpriced_steps
                            || current.children_cost < baseline.children_cost
                            || current.children_tokens < baseline.children_tokens;
                    }
                    _ => job.unpriced = true,
                }
            }
            _ => job.unpriced = true,
        }
    }

    async fn notify_job(&self, mut job: Job) -> Result<(), RuntimeError> {
        let mut admission = Admission::text(
            format!(
                "Background subagent handback: {}",
                serde_json::json!({
                    "job":job,
                    "duration_ms":job.ended_ms.unwrap_or(job.started_ms).saturating_sub(job.started_ms).max(0),
                    "output_tail":job.result.as_ref().and_then(|result| result["text"].as_str()).unwrap_or("").lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
                })
            ),
            Delivery::Queue,
        );
        admission.message_id = Some(format!("msg_handback_{}", job.id));
        admission.source = "session".into();
        self.admit(&job.session_id, admission).await?;
        job.notified = true;
        self.record_job(NOTIFIED, &job).await
    }

    pub async fn cancel_job(&self, id: &str) -> Result<Job, RuntimeError> {
        let control = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(control) = control {
            control.stop.cancel();
            control.done.cancelled().await;
        }
        self.job(id)
    }

    pub(super) async fn cancel_session_jobs(&self, ids: &[String]) -> Result<(), RuntimeError> {
        for job in self
            .jobs(None)?
            .into_iter()
            .filter(|job| ids.contains(&job.session_id) && job.status == JobStatus::Running)
        {
            if let Some(control) = self
                .inner
                .jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&job.id)
                .cloned()
            {
                control
                    .notify
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            self.cancel_job(&job.id).await?;
        }
        Ok(())
    }

    /// Must run once during application construction, before external admission.
    pub async fn recover_jobs(&self) -> Result<(), RuntimeError> {
        for mut job in self.jobs(None)? {
            if self
                .inner
                .jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&job.id)
            {
                continue;
            }
            if job.status == JobStatus::Running {
                job.status = JobStatus::Interrupted;
                job.ended_ms = Some(chrono::Utc::now().timestamp_millis());
                job.error = Some("Server restarted; background task was not redispatched".into());
                self.refresh_job_usage(&mut job).await;
                self.record_job(ENDED, &job).await?;
            }
            if !job.notified {
                self.notify_job(job).await?;
            }
        }
        Ok(())
    }
}
