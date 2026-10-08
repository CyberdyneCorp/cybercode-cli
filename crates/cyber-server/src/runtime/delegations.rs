//! Durable request identity precedes queue admission and native child effects.
use super::{Runtime, RuntimeError, TurnContext, UserSubtask};
use cyber_store::{EventRegistry, Expected, NewEvent, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, PoisonError};
use tokio_util::sync::CancellationToken;

const CHANGED: &str = "delegation.changed.1";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    Pending,
    Cancelling,
    Admitted,
    Cancelled,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegationPhase {
    Reserved,
    Launching,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Delegation {
    pub id: String,
    pub session_id: String,
    pub status: DelegationStatus,
    pub phase: DelegationPhase,
    pub job_id: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_bindings: Option<Vec<super::admission_authority::Binding>>,
    data: Delegation,
    hash: Option<String>,
}
pub(super) struct Control {
    stop: CancellationToken,
    done: CancellationToken,
}
pub(super) fn register(registry: &mut EventRegistry) {
    registry.register(CHANGED).expect("valid delegation event");
}
fn valid(id: &str) -> Result<(), RuntimeError> {
    if !(4..=128).contains(&id.len())
        || !cyber_core::ids::has_prefix(id, "op")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(RuntimeError::Invalid(
            "Delegation identity must be an op_ identifier".into(),
        ));
    }
    Ok(())
}
fn active(status: DelegationStatus) -> bool {
    matches!(
        status,
        DelegationStatus::Pending | DelegationStatus::Cancelling
    )
}
impl Runtime {
    fn delegation_record(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<Option<(Record, i64)>, RuntimeError> {
        valid(id)?;
        let Some(seq) = self.inner.store.aggregate_seq(id)? else {
            return Ok(None);
        };
        let page = self.inner.store.read_events(id, seq - 1, 1)?;
        let event = page
            .events
            .first()
            .ok_or_else(|| RuntimeError::Corrupt("Missing delegation event".into()))?;
        if event.kind != CHANGED {
            return Err(RuntimeError::Invalid(
                "Operation identity already belongs to another operation".into(),
            ));
        }
        let record: Record = serde_json::from_value(event.data.clone())
            .map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
        if record.data.session_id != parent {
            return Err(RuntimeError::Invalid(
                "Delegation does not belong to this Session".into(),
            ));
        }
        Ok(Some((record, event.seq)))
    }
    fn write_delegation(&self, record: &Record, seq: i64) -> Result<(), RuntimeError> {
        self.inner.store.append(
            &record.data.id,
            Expected::Seq(seq),
            vec![NewEvent::new(
                CHANGED,
                serde_json::to_value(record).expect("serializable delegation"),
            )],
        )?;
        Ok(())
    }
    /// Missing local ownership after restart is uncertainty, never redispatch authority.
    pub fn delegation(&self, parent: &str, id: &str) -> Result<Option<Delegation>, RuntimeError> {
        let Some((record, _)) = self.delegation_record(parent, id)? else {
            return Ok(None);
        };
        let mut data = record.data;
        if active(data.status)
            && !self
                .inner
                .delegations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(id)
                .is_some_and(|control| !control.done.is_cancelled())
        {
            data.status = DelegationStatus::Unknown;
            data.error =
                Some("Admission owner is unavailable; reconcile before any redispatch".into());
        }
        Ok(Some(data))
    }
    pub async fn start_delegation(
        &self,
        parent: &str,
        id: &str,
        mut request: UserSubtask,
    ) -> Result<Delegation, RuntimeError> {
        valid(id)?;
        if request.prompt.trim().is_empty()
            || request.max_steps == Some(0)
            || request.admission_id.is_some()
        {
            return Err(RuntimeError::Invalid(
                "Invalid delegation prompt, ceiling or internal identity".into(),
            ));
        }
        let authority = self.capture_child_admission(parent)?;
        let _open = self.inner.open().await?;
        let state = self.state(parent).await?;
        if !self.inner.tools.durable_user_delegation() {
            return Err(RuntimeError::Invalid(
                "Host does not support durable user delegation".into(),
            ));
        }
        let hash = format!("{:x}",Sha256::digest(serde_json::to_vec(&serde_json::json!({"prompt":request.prompt,"agent":request.agent,"attachments":request.attachments,"max_steps":request.max_steps})).expect("serializable request")));
        if let Some(existing) = self.existing_delegation(parent, id, &hash)? {
            return Ok(existing);
        }
        let record = Record {
            admission_bindings: Some(authority.bindings.clone()),
            data: Delegation {
                id: id.into(),
                session_id: parent.into(),
                status: DelegationStatus::Pending,
                phase: DelegationPhase::Reserved,
                job_id: None,
                error: None,
            },
            hash: Some(hash.clone()),
        };
        let control = Arc::new(Control {
            stop: self.inner.closed.child_token(),
            done: CancellationToken::new(),
        });
        // The synchronous append and local ownership publication have no await gap.
        match self.write_delegation(&record, -1) {
            Ok(()) => {}
            Err(RuntimeError::Store(StoreError::Concurrency { .. })) => {
                return self
                    .existing_delegation(parent, id, &hash)?
                    .ok_or_else(|| RuntimeError::Corrupt("Missing competing admission".into()));
            }
            Err(error) => return Err(error),
        }
        self.inner
            .delegations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.into(), control.clone());
        request.admission_id = Some(id.into());
        let running = self.is_running(parent);
        let turn = TurnContext {
            session_id: parent.into(),
            directory: state.info.directory.clone(),
            agent: state.effective_agent(running).into(),
            mode: state.effective_mode(running).into(),
            prefers_apply_patch: false,
            rules: state.info.rules,
        };
        let host = self.inner.tools.clone();
        let weak = self.downgrade();
        let identity = id.to_owned();
        let source = parent.to_owned();
        let worker = tokio::spawn(async move {
            let _on_exit = control.done.clone().drop_guard();
            let result = authority
                .clone()
                .with_operation(identity.clone())
                .with_cancellation(control.done.clone())
                .run(
                    control.stop.clone(),
                    host.subtask_request(turn, request, control.stop.clone()),
                )
                .await;
            if let Some(runtime) = weak.upgrade() {
                let cancelled = control.stop.is_cancelled()
                    || (result.is_err() && authority.verify(&runtime, &source).is_err());
                if let Err(error) = runtime
                    .finish_delegation(&source, &identity, result, cancelled)
                    .await
                {
                    cyber_core::log::error(
                        "delegation",
                        &error.to_string(),
                        serde_json::json!({"request_id":identity,"session_id":source}),
                    );
                }
                runtime
                    .inner
                    .delegations
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&identity);
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(worker);
        Ok(record.data)
    }
    fn existing_delegation(
        &self,
        parent: &str,
        id: &str,
        hash: &str,
    ) -> Result<Option<Delegation>, RuntimeError> {
        loop {
            let Some((mut record, seq)) = self.delegation_record(parent, id)? else {
                return Ok(None);
            };
            if record.hash.as_deref().is_some_and(|prior| prior != hash) {
                return Err(RuntimeError::Conflict(
                    "Delegation identity was used with different input".into(),
                ));
            }
            if record.hash.is_none() {
                record.hash = Some(hash.into());
                match self.write_delegation(&record, seq) {
                    Err(RuntimeError::Store(StoreError::Concurrency { .. })) => continue,
                    result => result?,
                }
            }
            return self.delegation(parent, id);
        }
    }
    /// Legacy user dispatch has no ledger. Durable hosts call this before effects.
    pub async fn mark_delegation_launching(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<(), RuntimeError> {
        self.capture_child_admission(parent)?;
        self.state(parent).await?;
        loop {
            let Some((mut record, seq)) = self.delegation_record(parent, id)? else {
                return Ok(());
            };
            if record.data.status != DelegationStatus::Pending
                || record.data.phase != DelegationPhase::Reserved
            {
                return Err(RuntimeError::Invalid(
                    "Delegation no longer permits child creation".into(),
                ));
            }
            let owned = self
                .inner
                .delegations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(id)
                .is_some_and(|control| {
                    !control.done.is_cancelled() && !control.stop.is_cancelled()
                });
            if !owned {
                return Err(RuntimeError::Invalid(
                    "Delegation owner is unavailable; reconcile before launch".into(),
                ));
            }
            record.data.phase = DelegationPhase::Launching;
            match self.write_delegation(&record, seq) {
                Err(RuntimeError::Store(StoreError::Concurrency { .. })) => continue,
                result => return result,
            }
        }
    }
    pub async fn cancel_delegation(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<Delegation, RuntimeError> {
        let _open = self.inner.open().await?;
        self.state(parent).await?;
        loop {
            let prior = self.delegation_record(parent, id)?;
            let (mut record, seq) = prior.unwrap_or_else(|| {
                (
                    Record {
                        admission_bindings: None,
                        data: Delegation {
                            id: id.into(),
                            session_id: parent.into(),
                            status: DelegationStatus::Cancelled,
                            phase: DelegationPhase::Reserved,
                            job_id: None,
                            error: None,
                        },
                        hash: None,
                    },
                    -1,
                )
            });
            if let Some(job) = &record.data.job_id {
                let recorded = self.job(job)?;
                if recorded.session_id != parent {
                    return Err(RuntimeError::Corrupt(
                        "Delegation Job ownership changed".into(),
                    ));
                }
                let settled = self.cancel_job(job).await?;
                if settled.status == super::JobStatus::Running {
                    record.data.status = DelegationStatus::Unknown;
                    record.data.error =
                        Some(format!("Cancellation was not acknowledged; inspect {job}"));
                } else {
                    return Ok(record.data);
                }
            } else if seq >= 0 && !active(record.data.status) {
                return Ok(record.data);
            } else if seq >= 0 {
                let control = self
                    .inner
                    .delegations
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(id)
                    .cloned();
                if let Some(control) = control.filter(|control| !control.done.is_cancelled()) {
                    control.stop.cancel();
                    record.data.status = DelegationStatus::Cancelling;
                } else if record.data.phase == DelegationPhase::Reserved {
                    // The expected-sequence write fences every later launch marker.
                    record.data.status = DelegationStatus::Cancelled;
                    record.data.error = None;
                } else {
                    return Ok(self.delegation(parent, id)?.expect("existing admission"));
                }
            }
            match self.write_delegation(&record, seq) {
                Err(RuntimeError::Store(StoreError::Concurrency { .. })) => continue,
                Err(error) => return Err(error),
                Ok(()) => return Ok(record.data),
            }
        }
    }
    async fn finish_delegation(
        &self,
        parent: &str,
        id: &str,
        result: Result<super::Job, String>,
        cancelled: bool,
    ) -> Result<(), RuntimeError> {
        let outcome = match result {
            Ok(job) => {
                let recorded = self.job(&job.id)?;
                if recorded.session_id != parent || recorded.child_id != job.child_id {
                    return Err(RuntimeError::Corrupt(
                        "Delegation Job ownership changed".into(),
                    ));
                }
                let error = if cancelled
                    && self.cancel_job(&job.id).await?.status == super::JobStatus::Running
                {
                    Some(format!(
                        "Cancellation was not acknowledged; inspect {}",
                        job.id
                    ))
                } else {
                    None
                };
                Ok((job.id, error))
            }
            Err(error) => Err(error),
        };
        loop {
            let (mut record, seq) = self
                .delegation_record(parent, id)?
                .ok_or_else(|| RuntimeError::Corrupt("Missing delegation reservation".into()))?;
            if record.data.status == DelegationStatus::Cancelled
                && record.data.phase == DelegationPhase::Reserved
                && outcome.is_err()
            {
                return Ok(());
            }
            match &outcome {
                Ok((job, error)) => {
                    record.data.job_id = Some(job.clone());
                    record.data.status = if error.is_some() {
                        DelegationStatus::Unknown
                    } else {
                        DelegationStatus::Admitted
                    };
                    record.data.error = error.clone();
                }
                Err(error) => {
                    record.data.status = match (record.data.phase, cancelled) {
                        (DelegationPhase::Launching, _) => DelegationStatus::Unknown,
                        (_, true) => DelegationStatus::Cancelled,
                        (_, false) => DelegationStatus::Failed,
                    };
                    record.data.error = Some(error.clone());
                }
            }
            match self.write_delegation(&record, seq) {
                Err(RuntimeError::Store(StoreError::Concurrency { .. })) => continue,
                result => return result,
            }
        }
    }
}
