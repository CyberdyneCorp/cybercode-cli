//! Background work a run started (child Jobs, a forked command): exec does not
//! finish while it is unsettled, and a stopped run cancels only that work.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use cyber_client::{Client, Event};
use cyber_server::runtime::settlement::{HANDBACK_GRACE, JobSettlement, job_settlement};
use cyber_server::runtime::{DelegationStatus, Runtime};
use serde_json::Value;

use super::delegation::{admission::Admission, stop_job};
use super::{api, report::Run, setup::urlencode};
use crate::error::CliError;

/// How long an unread durable tail may stay unread before it is presumed lost
/// (a lagged live stream drops events rather than blocking the server).
const LAG_GRACE: Duration = Duration::from_secs(2);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Settle {
    Done,
    Wait,
}

/// How a forked command ended.
pub enum ForkOutcome {
    /// Its Job: status, `result` and `error`.
    Job(Value),
    /// Failed or cancelled before a Job existed (a hook or admission denied
    /// the spawn); carries the delegation's error.
    Ended(String),
}

pub(super) struct Owned {
    session: String,
    /// Jobs that belonged to the Session before this run: never awaited or cancelled.
    baseline: HashSet<String>,
    /// The forked command's delegation; `None` once known not to be forked.
    fork: Option<Admission>,
    /// Message IDs admitted to the parent since the run began.
    admitted: HashSet<String>,
    last_seq: i64,
    lagged: Option<(i64, Instant)>,
    /// Jobs and the fork a stop already tried to cancel.
    stopped: HashSet<String>,
    /// When ended work was first seen without its handback admitted.
    unnotified: HashMap<String, Instant>,
    /// The last `background_settlement` error: a handback admission failed.
    settlement: Option<String>,
}

impl Owned {
    /// Record the Session's existing Jobs; call before the run sends anything.
    pub async fn capture(client: &Client, session: &str) -> Result<Self, CliError> {
        let baseline = jobs(client, session)
            .await?
            .iter()
            .filter_map(|job| job["id"].as_str().map(str::to_string))
            .collect();
        Ok(Self {
            session: session.into(),
            baseline,
            fork: None,
            admitted: HashSet::new(),
            last_seq: -1,
            lagged: None,
            stopped: HashSet::new(),
            unnotified: HashMap::new(),
            settlement: None,
        })
    }

    /// A `--command` receipt: the server derives a fork's delegation ID from it.
    pub fn command(&mut self, message: &str) -> Result<(), CliError> {
        let id = Runtime::skill_command_delegation_id(&self.session, message);
        self.fork = Some(Admission::existing(&self.session, &id)?);
        Ok(())
    }

    pub fn observe(&mut self, event: &Event) {
        if let Some(seq) = event.seq {
            self.last_seq = self.last_seq.max(seq);
        }
        if event.kind.starts_with("session.prompt.admitted")
            && let Some(id) = event.data["message_id"].as_str()
        {
            self.admitted.insert(id.into());
        }
        if event.kind == "session.error" && event.data["kind"] == "background_settlement" {
            self.settlement = event.data["message"].as_str().map(str::to_string);
        }
    }

    /// Whether the run may finish now that the parent went (or stayed) idle.
    /// While stopping, handbacks are not awaited: they are cancelled work.
    pub async fn settle(&mut self, client: &Client, stopping: bool) -> Result<Settle, CliError> {
        if self.fork_state(client, stopping).await? == Settle::Wait {
            return Ok(Settle::Wait);
        }
        let owned = self.owned_jobs(client).await?;
        let mut unsettled = false;
        for job in &owned {
            unsettled |= self.job_unsettled(job, stopping)?;
        }
        if unsettled {
            return Ok(Settle::Wait);
        }
        if self.fork.is_none() && owned.is_empty() {
            return Ok(Settle::Done);
        }
        self.quiescent(client).await
    }

    async fn fork_state(&mut self, client: &Client, stopping: bool) -> Result<Settle, CliError> {
        let Some(fork) = &self.fork else {
            return Ok(Settle::Done);
        };
        let Some(record) = fork.find(client).await? else {
            self.fork = None;
            return Ok(Settle::Done);
        };
        let handback = format!("msg_skill_handback_{}", fork.id);
        match record.status {
            DelegationStatus::Pending | DelegationStatus::Cancelling => Ok(Settle::Wait),
            DelegationStatus::Unknown => Err(CliError::runtime(format!(
                "Forked skill admission needs reconciliation; inspect {}",
                fork.id
            ))),
            DelegationStatus::Failed | DelegationStatus::Cancelled
                if !stopping && !self.admitted.contains(&handback) =>
            {
                let id = fork.id.clone();
                self.handback_pending(&id).map(|()| Settle::Wait)
            }
            _ => Ok(Settle::Done),
        }
    }

    /// A running Job, or (unless stopping) an ended one whose handback is not admitted yet.
    fn job_unsettled(&mut self, job: &Value, stopping: bool) -> Result<bool, CliError> {
        match job_settlement(job, stopping) {
            JobSettlement::Running => Ok(true),
            JobSettlement::Settled => Ok(false),
            JobSettlement::AwaitingHandback => {
                self.handback_pending(job["id"].as_str().unwrap_or_default())?;
                Ok(true)
            }
        }
    }

    /// Ended work whose handback admission failed (or never arrived within the
    /// grace period) would otherwise be awaited forever.
    fn handback_pending(&mut self, id: &str) -> Result<(), CliError> {
        let since = *self
            .unnotified
            .entry(id.into())
            .or_insert_with(Instant::now);
        if self.settlement.is_none() && since.elapsed() < HANDBACK_GRACE {
            return Ok(());
        }
        let detail = self
            .settlement
            .as_deref()
            .unwrap_or("no handback was admitted");
        Err(CliError::runtime(format!(
            "Background handback was not admitted; inspect {id}: {detail}"
        )))
    }

    /// The forked command's delegation ID; `None` once known not to be forked.
    pub fn fork_id(&self) -> Option<&str> {
        self.fork.as_ref().map(|fork| fork.id.as_str())
    }

    /// The forked command's Job (its status, `result` and `error`), from the
    /// Jobs this run owns; `None` for an inline command.
    pub async fn fork_job(&self, client: &Client) -> Result<Option<ForkOutcome>, CliError> {
        let Some(fork) = &self.fork else {
            return Ok(None);
        };
        let Some(record) = fork.find(client).await? else {
            return Ok(None);
        };
        let Some(job) = record.job_id else {
            return Ok(match record.status {
                DelegationStatus::Failed | DelegationStatus::Cancelled => {
                    Some(ForkOutcome::Ended(record.error.unwrap_or_else(|| {
                        format!(
                            "The forked command ended without a Job; inspect {}",
                            fork.id
                        )
                    })))
                }
                _ => None,
            });
        };
        let owned = self.owned_jobs(client).await?;
        Ok(owned
            .into_iter()
            .find(|candidate| candidate["id"] == job.as_str())
            .map(ForkOutcome::Job))
    }

    async fn owned_jobs(&self, client: &Client) -> Result<Vec<Value>, CliError> {
        let mut owned = jobs(client, &self.session).await?;
        owned.retain(|job| {
            job["id"]
                .as_str()
                .is_some_and(|id| !self.baseline.contains(id))
        });
        Ok(owned)
    }

    /// The parent is idle and every durable event it committed has been read,
    /// so the final text belongs to the Turn that handled the last handback.
    async fn quiescent(&mut self, client: &Client) -> Result<Settle, CliError> {
        let snapshot = client
            .get(&format!("/sessions/{}", self.session))
            .await
            .map_err(api)?;
        let session = &snapshot["data"];
        if session["status"] != "idle" {
            self.lagged = None;
            return Ok(Settle::Wait);
        }
        let seq = session["seq"].as_i64().unwrap_or(-1);
        if self.last_seq >= seq {
            return Ok(Settle::Done);
        }
        match self.lagged {
            Some((at, since)) if at == seq && since.elapsed() >= LAG_GRACE => Ok(Settle::Done),
            Some((at, _)) if at == seq => Ok(Settle::Wait),
            _ => {
                self.lagged = Some((seq, Instant::now()));
                Ok(Settle::Wait)
            }
        }
    }

    /// Cancel the pending fork and the running Jobs this run owns. Idempotent:
    /// a stopping run calls it again so work started while it was stopping
    /// (before the Drain acknowledged the interrupt) is cancelled too.
    pub async fn stop(&mut self, client: &Client, run: &mut Run) {
        if let Err(error) = self.stop_fork(client).await {
            append_error(run, error.message);
        }
        let owned = match self.owned_jobs(client).await {
            Ok(owned) => owned,
            Err(error) => {
                append_error(run, error.message);
                return;
            }
        };
        for job in owned.iter().filter(|job| job["status"] == "running") {
            let id = job["id"].as_str().unwrap_or_default();
            if !self.stopped.insert(id.into()) {
                continue;
            }
            if !matches!(
                tokio::time::timeout(STOP_TIMEOUT, stop_job(client, id)).await,
                Ok(Ok(_))
            ) {
                append_error(
                    run,
                    format!("Background Job cancellation was not acknowledged; inspect {id}"),
                );
            }
        }
    }

    async fn stop_fork(&mut self, client: &Client) -> Result<(), CliError> {
        let Some(fork) = &self.fork else {
            return Ok(());
        };
        if self.stopped.contains(&fork.id) {
            return Ok(());
        }
        // An unrecorded request is never stopped: that would tombstone an inline command.
        let pending = fork.find(client).await?.is_some_and(|record| {
            matches!(
                record.status,
                DelegationStatus::Pending | DelegationStatus::Cancelling
            )
        });
        if !pending {
            return Ok(());
        }
        self.stopped.insert(fork.id.clone());
        let detail = match tokio::time::timeout(STOP_TIMEOUT, fork.stop(client, &mut None)).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => error.message,
            Err(_) => "Cancellation acknowledgement timed out".into(),
        };
        Err(CliError::runtime(format!(
            "Forked skill cancellation was not acknowledged; inspect {}: {detail}",
            fork.id
        )))
    }
}

/// Every Job of the Session, across pages.
async fn jobs(client: &Client, session: &str) -> Result<Vec<Value>, CliError> {
    let mut all = Vec::new();
    let mut cursor = String::new();
    loop {
        let page = client
            .get(&format!("/jobs?session_id={session}&limit=200{cursor}"))
            .await
            .map_err(api)?;
        let data = page["data"]
            .as_array()
            .ok_or_else(|| CliError::runtime("Invalid Job list"))?;
        all.extend(data.iter().cloned());
        match page["cursor"]["next"].as_str() {
            Some(next) if !data.is_empty() => cursor = format!("&cursor={}", urlencode(next)),
            _ => return Ok(all),
        }
    }
}

fn append_error(run: &mut Run, message: String) {
    run.error = Some(match run.error.take() {
        Some(prior) => format!("{prior}; {message}"),
        None => message,
    });
}
