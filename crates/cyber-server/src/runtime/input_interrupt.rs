//! Retained input-scoped control; never native-effect acknowledgement.
use super::{InputStatus, Runtime, RuntimeError, events, requests::input_owners};
use cyber_store::{EventRegistry, StoreError};
use futures::FutureExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{sync::PoisonError, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) const CHANGED: &str = "session.input.interruption.1";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputInterrupt {
    pub request_id: String,
    pub input_message_ids: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputInterruptStatus {
    Requested,
    Acknowledged,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputInterruptProblem {
    MixedDrain,
    MixedJob,
    UnknownDrain,
    DrainTimeout,
    DescendantJob,
    MissingJobControl,
    JobTimeout,
    UnownedActivity,
    ReconciliationRequired,
    CleanupFailed,
}
/// Bounded control evidence. No tool payload, output, or native acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputInterruptReceipt {
    pub request_id: String,
    pub input_message_ids: Vec<String>,
    pub generation: String,
    pub status: InputInterruptStatus,
    /// Exact durable receipt revision, populated from the committed event on lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_seq: Option<i64>,
    pub drain_selected: bool,
    /// Original queued/held inputs refused at admission; excludes Job notices.
    pub refused: u64,
    pub jobs_cancelled: u64,
    pub problems: Vec<InputInterruptProblem>,
}
impl InputInterruptReceipt {
    fn problem(&mut self, problem: InputInterruptProblem) {
        if !self.problems.contains(&problem) {
            self.problems.push(problem);
        }
    }
    fn observed(mut self, generation: &str, retained: bool) -> Self {
        if self.status == InputInterruptStatus::Requested
            && (self.generation != generation || !retained)
        {
            self.status = InputInterruptStatus::Unknown;
            self.problem(InputInterruptProblem::ReconciliationRequired);
        }
        self
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256
}
fn valid_inputs(ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.len() <= 32
        && ids.iter().all(|id| valid_id(id))
        && ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len()
}
fn owns(selected: &[String], cohort: &[String]) -> bool {
    valid_inputs(cohort) && cohort.iter().all(|id| selected.contains(id))
}
pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CHANGED, |data| {
            let receipt: InputInterruptReceipt =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            if !valid_id(&receipt.request_id)
                || !valid_inputs(&receipt.input_message_ids)
                || !valid_id(&receipt.generation)
                || receipt.problems.len() > 10
            {
                return Err("Invalid input interruption receipt".into());
            }
            Ok(())
        })
        .expect("valid input interruption event");
}
fn recorded(
    db: &Connection,
    session: &str,
    request: &str,
) -> Result<Option<InputInterruptReceipt>, StoreError> {
    let raw: Option<(String, i64)> = db
        .query_row(
            "SELECT data,seq FROM event WHERE aggregate_id=?1 AND type=?2
         AND json_extract(data,'$.request_id')=?3 ORDER BY seq DESC LIMIT 1",
            params![session, CHANGED, request],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    raw.map(|(raw, seq)| {
        let mut receipt: InputInterruptReceipt = serde_json::from_str(&raw)
            .map_err(|_| StoreError::Unavailable("Malformed input interruption receipt".into()))?;
        receipt.session_seq = Some(seq);
        Ok(receipt)
    })
    .transpose()
}

struct CapturedDrain {
    cancel: CancellationToken,
    completed: CancellationToken,
}
struct Registration {
    inner: std::sync::Weak<super::Inner>,
    key: (String, String),
}
impl Registration {
    fn new(inner: &std::sync::Arc<super::Inner>, session: &str, request: &str) -> Self {
        let key = (session.to_owned(), request.to_owned());
        inner
            .input_interrupts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone());
        Self {
            inner: std::sync::Arc::downgrade(inner),
            key,
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .input_interrupts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.key);
        }
    }
}
impl Runtime {
    /// Lookup observes the original request only; it cannot target new execution.
    pub fn input_interrupt_receipt(
        &self,
        session: &str,
        request: &str,
    ) -> Result<Option<InputInterruptReceipt>, RuntimeError> {
        if !valid_id(request) {
            return Err(RuntimeError::Invalid(
                "Invalid interruption request identity".into(),
            ));
        }
        let key = (session.to_owned(), request.to_owned());
        let retained = self
            .inner
            .input_interrupts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&key);
        let (session, request) = key;
        Ok(self
            .inner
            .store
            .read(move |db| recorded(db, &session, &request))?
            .map(|receipt| receipt.observed(&self.inner.input_interrupt_generation, retained)))
    }

    /// Persist intent and queued refusals before signalling the exact captured Drain.
    /// Cleanup is runtime-owned even if the requester drops the returned response.
    pub async fn interrupt_inputs(
        &self,
        session: &str,
        mut request: InputInterrupt,
    ) -> Result<InputInterruptReceipt, RuntimeError> {
        if !valid_id(&request.request_id) || !valid_inputs(&request.input_message_ids) {
            return Err(RuntimeError::Invalid("Expected a request identity and 1–32 unique input identities, each at most 256 bytes".into()));
        }
        request.input_message_ids.sort();
        let lease = self.memory_mutation_lease().await?;
        let handle = self.inner.handle(session).await?;
        let mut state = handle.state.lock().await;
        if let Some(receipt) = self.input_interrupt_receipt(session, &request.request_id)? {
            if receipt.input_message_ids != request.input_message_ids {
                return Err(RuntimeError::Conflict(
                    "Interruption identity already selects different inputs".into(),
                ));
            }
            return Ok(receipt);
        }
        if state.info.parent_id.is_some() {
            return Err(RuntimeError::Invalid(
                "Scoped child interruption requires descendant ownership".into(),
            ));
        }
        for id in &request.input_message_ids {
            let row = state.input(id).ok_or_else(|| {
                RuntimeError::Invalid("Input is not admitted in this Session".into())
            })?;
            if row.forwarded_skill || row.source != "user" {
                return Err(RuntimeError::Invalid(
                    "Scoped interruption requires original user inputs".into(),
                ));
            }
        }
        let mut receipt = InputInterruptReceipt {
            request_id: request.request_id,
            input_message_ids: request.input_message_ids,
            generation: self.inner.input_interrupt_generation.clone(),
            status: InputInterruptStatus::Requested,
            session_seq: None,
            drain_selected: false,
            refused: 0,
            jobs_cancelled: 0,
            problems: Vec::new(),
        };
        let cohort = input_owners::capture(&state);
        let (id, before) = (session.to_owned(), state.last_seq + 1);
        let cohort = self
            .inner
            .store
            .read(move |db| input_owners::resolve(db, &id, cohort, before))?;
        let mut events = Vec::new();
        for row in &state.inbox {
            if receipt.input_message_ids.contains(&row.message_id)
                && matches!(row.status, InputStatus::Pending | InputStatus::Held)
            {
                events.push(refused(&row.message_id));
                receipt.refused += 1;
            }
        }
        let registration = Registration::new(&self.inner, session, &receipt.request_id);
        let captured = {
            let drains = self
                .inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            select_drain(drains.get(session), &cohort, &state, &mut receipt)
        };
        events.push(events::event(CHANGED, &receipt));
        // Promotion cannot cross the state lock. Completion/replacement may
        // proceed: signalling retains the old token and never selects anew.
        let stored = self.inner.commit_locked(&mut state, events)?;
        receipt.session_seq = stored
            .iter()
            .find(|event| event.kind == CHANGED)
            .map(|event| event.seq);
        if refuse_pending(&self.inner, &mut state).is_err() {
            receipt.problem(InputInterruptProblem::CleanupFailed);
        }
        if let Some(drain) = &captured {
            drain.cancel.cancel();
        }
        drop(state);
        let initial = receipt.clone();
        let runtime = self.clone();
        let session = session.to_owned();
        // No await between signalling and registering the retained cleanup owner.
        let task = tokio::spawn(async move {
            let _lease = lease;
            let _registration = registration;
            let outcome = std::panic::AssertUnwindSafe(runtime.settle_input_interrupt(
                &session,
                captured,
                &mut receipt,
            ))
            .catch_unwind()
            .await;
            if !matches!(outcome, Ok(Ok(()))) {
                receipt.problem(InputInterruptProblem::CleanupFailed);
            }
            receipt.status = if receipt.problems.is_empty() {
                InputInterruptStatus::Acknowledged
            } else {
                InputInterruptStatus::Unknown
            };
            if let Ok(handle) = runtime.inner.handle(&session).await {
                let _ = runtime
                    .inner
                    .commit(&handle, vec![events::event(CHANGED, &receipt)])
                    .await;
            }
        });
        let mut background = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        background.retain(|task| !task.is_finished());
        background.push(task);
        Ok(initial)
    }

    async fn settle_input_interrupt(
        &self,
        session: &str,
        captured: Option<CapturedDrain>,
        receipt: &mut InputInterruptReceipt,
    ) -> Result<(), RuntimeError> {
        if receipt.drain_selected {
            self.inner
                .abandon_input_requests(session, &receipt.input_message_ids)
                .await?;
        }
        if let Some(drain) = captured
            && tokio::time::timeout(CONTROL_TIMEOUT, drain.completed.cancelled())
                .await
                .is_err()
        {
            receipt.problem(InputInterruptProblem::DrainTimeout);
            return Ok(());
        }
        // ProcessReservation/JobAdmission retains this guard through registration.
        // Release it before waiting for supervisors, whose cleanup may re-enter it.
        if tokio::time::timeout(CONTROL_TIMEOUT, self.inner.job_admission.lock())
            .await
            .is_err()
        {
            receipt.problem(InputInterruptProblem::JobTimeout);
            return Ok(());
        }
        self.cancel_input_jobs(session, receipt).await?;
        let handle = self.inner.handle(session).await?;
        {
            let mut state = handle.state.lock().await;
            refuse_pending(&self.inner, &mut state)?;
        }
        if receipt.drain_selected
            && (handle.has_location_uncertainty()
                || self
                    .inner
                    .computer
                    .get()
                    .is_some_and(|computer| computer.require_stopped().is_err())
                || self
                    .inner
                    .activities
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .values()
                    .any(|control| control.source == session))
        {
            receipt.problem(InputInterruptProblem::UnownedActivity);
        }
        // A new, unrelated Drain is independent of the captured completion token.
        // Resume preserved queued input only after the selected Drain has released.
        if receipt.drain_selected
            && handle
                .state
                .lock()
                .await
                .inbox
                .iter()
                .any(|row| row.status == InputStatus::Pending)
            && !self.inner.closed.is_cancelled()
        {
            let state = handle.state.lock().await;
            state.ensure_worktree_ready()?;
            self.inner.ensure_snapshot_admission(&handle, session)?;
            drop(state);
            self.inner.start_drain(session, false);
        }
        Ok(())
    }
    async fn cancel_input_jobs(
        &self,
        session: &str,
        receipt: &mut InputInterruptReceipt,
    ) -> Result<(), RuntimeError> {
        let selected =
            serde_json::to_string(&receipt.input_message_ids).expect("input IDs serialize");
        let mut cursor = String::new();
        loop {
            let (id, ids, after) = (session.to_owned(), selected.clone(), cursor.clone());
            let page = self
                .inner
                .store
                .read(move |db| job_page(db, &id, &ids, &after))?;
            if page.is_empty() {
                break;
            }
            for (id, child, complete) in page {
                cursor = id.clone();
                if !complete {
                    receipt.problem(InputInterruptProblem::MixedJob);
                    continue;
                }
                if !child.is_empty() {
                    self.stop_input_subagent(session, &id, &child, receipt)
                        .await?;
                    continue;
                }
                let known = self
                    .inner
                    .jobs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(&id);
                if !known {
                    receipt.problem(InputInterruptProblem::MissingJobControl);
                    continue;
                }
                match tokio::time::timeout(CONTROL_TIMEOUT, self.cancel_job_quietly(&id)).await {
                    Ok(Ok(_)) => receipt.jobs_cancelled += 1,
                    Ok(Err(error)) => return Err(error),
                    Err(_) => receipt.problem(InputInterruptProblem::JobTimeout),
                }
            }
        }
        Ok(())
    }
    /// Stop a subagent Job's child subtree only when that subtree is wholly owned
    /// by the Job: a direct child of this Session that no other outside Job
    /// targets. Mixed, detached or unresolved descendant work reports unknown.
    async fn stop_input_subagent(
        &self,
        session: &str,
        job: &str,
        child: &str,
        receipt: &mut InputInterruptReceipt,
    ) -> Result<(), RuntimeError> {
        let (id, job_id, child_id) = (session.to_owned(), job.to_owned(), child.to_owned());
        let owned = self
            .inner
            .store
            .read(move |db| subagent_owned(db, &id, &job_id, &child_id))?;
        if !owned {
            receipt.problem(InputInterruptProblem::DescendantJob);
            return Ok(());
        }
        let control = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(job)
            .cloned();
        let Some(control) = control else {
            receipt.problem(InputInterruptProblem::MissingJobControl);
            return Ok(());
        };
        // An interrupted input must not be resumed by the subagent's handback.
        control
            .notify
            .store(false, std::sync::atomic::Ordering::SeqCst);
        match tokio::time::timeout(CONTROL_TIMEOUT, self.stop_subtree_leased(child)).await {
            Ok(Ok(report)) if report.status == super::SubtreeStopStatus::Acknowledged => {
                receipt.jobs_cancelled += 1;
            }
            Ok(_) => receipt.problem(InputInterruptProblem::DescendantJob),
            Err(_) => receipt.problem(InputInterruptProblem::JobTimeout),
        }
        Ok(())
    }
}
/// Bounded descendant sessions; a larger or deeper subtree is not proven owned.
const SUBTREE_LIMIT: i64 = 256;
fn subagent_owned(
    db: &Connection,
    session: &str,
    job: &str,
    child: &str,
) -> Result<bool, StoreError> {
    let parent: Option<Option<String>> = db
        .query_row(
            "SELECT parent_id FROM session WHERE id=?1",
            [child],
            |row| row.get(0),
        )
        .optional()?;
    if parent.flatten().as_deref() != Some(session) {
        return Ok(false);
    }
    let (count, tree): (i64, String) = db.query_row(
        "WITH RECURSIVE tree(id,depth) AS (SELECT ?1,0 UNION
            SELECT s.id,t.depth+1 FROM session s JOIN tree t ON s.parent_id=t.id WHERE t.depth<?2)
        SELECT count(*),json_group_array(id) FROM (SELECT DISTINCT id FROM tree LIMIT ?2+1)",
        params![child, SUBTREE_LIMIT],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if count > SUBTREE_LIMIT {
        return Ok(false);
    }
    let foreign: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM job j WHERE j.status='running' AND j.id<>?2
            AND j.child_id IN (SELECT value FROM json_each(?1))
            AND j.session_id NOT IN (SELECT value FROM json_each(?1)))",
        params![tree, job],
        |row| row.get(0),
    )?;
    Ok(!foreign)
}
fn select_drain(
    entry: Option<&super::DrainEntry>,
    cohort: &[String],
    state: &super::SessionState,
    receipt: &mut InputInterruptReceipt,
) -> Option<CapturedDrain> {
    let entry = entry?;
    if entry.deleting || entry.child_settling {
        receipt.problem(InputInterruptProblem::UnknownDrain);
        return None;
    }
    if owns(&receipt.input_message_ids, cohort) {
        receipt.drain_selected = true;
        return Some(CapturedDrain {
            cancel: entry.cancel.clone(),
            completed: entry.completed.clone(),
        });
    }
    if cohort
        .iter()
        .any(|id| receipt.input_message_ids.contains(id))
    {
        receipt.problem(InputInterruptProblem::MixedDrain);
    } else if cohort.is_empty()
        && receipt.input_message_ids.iter().any(|id| {
            state
                .input(id)
                .is_some_and(|row| row.status == InputStatus::Promoted)
        })
    {
        receipt.problem(InputInterruptProblem::UnknownDrain);
    }
    None
}
pub(super) fn refused(message: &str) -> cyber_store::NewEvent {
    events::event(
        events::INBOX_UPDATED,
        &events::InboxUpdated {
            message_id: message.into(),
            action: events::InboxAction::Refused,
            parts: None,
            delivery: None,
            hook_context: None,
            hook_stop: None,
            resources: Vec::new(),
            untrusted: None,
        },
    )
}

/// A notifier racing interruption commits an already refused handback. Validate
/// the actual immutable Job origin; an admission's name or source cannot opt in.
pub(super) fn notifier_refused(
    store: &cyber_store::Store,
    session: &str,
    message: &str,
    extra: &[cyber_store::NewEvent],
) -> Result<bool, RuntimeError> {
    let Some(notice) = extra.iter().find(|event| event.kind == "job.notified.1") else {
        return Ok(false);
    };
    let data = &notice.data;
    let Some(job) = data["id"].as_str() else {
        return Ok(false);
    };
    if message != super::jobs::handback_id(job)
        || data["session_id"] != session
        || data["notified"] != true
        || !origin_metadata_valid(&data["input_origin"])
    {
        return Ok(false);
    }
    let Some(ids) = data["input_origin"]["input_message_ids"]
        .as_array()
        .filter(|ids| ids.len() <= 32)
    else {
        return Ok(false);
    };
    let Some(cohort) = ids
        .iter()
        .map(|id| id.as_str().filter(|id| valid_id(id)).map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(false);
    };
    if !valid_inputs(&cohort) {
        return Ok(false);
    }
    let (session, job, origin) = (
        session.to_owned(),
        job.to_owned(),
        data["input_origin"].to_string(),
    );
    Ok(store.read(move |db| {
        let valid: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1
            AND type='job.started.1' AND json_extract(data,'$.id')=?2
            AND json_extract(data,'$.input_origin') IS ?3)",
            params![session, job, origin],
            |row| row.get(0),
        )?;
        Ok(valid && interrupted(db, &session, &cohort)?)
    })?)
}

fn origin_metadata_valid(origin: &serde_json::Value) -> bool {
    ["message_id", "call_id", "operation_key"]
        .iter()
        .all(|field| {
            origin[field]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        })
        && origin["attempt"]
            .as_u64()
            .is_some_and(|attempt| (1..=u32::MAX as u64).contains(&attempt))
}

/// Durable interruption intent also fences a late Job notifier. Names and
/// source labels do not establish handback ownership: resolve the actual
/// accepted notifier's immutable origin before matching an interruption.
pub(super) fn refuse_pending(
    inner: &super::Inner,
    state: &mut super::SessionState,
) -> Result<(), RuntimeError> {
    let session = state.info.id.clone();
    let present = inner.store.read(move |db| {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2)",
            params![session, CHANGED],
            |row| row.get::<_, bool>(0),
        )?)
    })?;
    if !present {
        return Ok(());
    }
    let mut cursor = -1;
    loop {
        // Inbox order is admission order. Bound each copied identity page and
        // refusal commit independently of the Session's retained history.
        let page: Vec<_> = state
            .inbox
            .iter()
            .filter(|row| {
                row.admitted_seq > cursor
                    && matches!(row.status, InputStatus::Pending | InputStatus::Held)
            })
            .take(64)
            .map(|row| (row.admitted_seq, row.message_id.clone()))
            .collect();
        if page.is_empty() {
            return Ok(());
        }
        cursor = page.last().expect("nonempty page").0;
        let (session, before) = (state.info.id.clone(), state.last_seq + 1);
        let refusals = inner.store.read(move |db| {
            let mut refusals = Vec::new();
            for (_, id) in page {
                let cohort = input_owners::resolve(db, &session, vec![id.clone()], before)?;
                if interrupted(db, &session, &cohort)? {
                    refusals.push(refused(&id));
                }
            }
            Ok(refusals)
        })?;
        if !refusals.is_empty() {
            inner.commit_locked(state, refusals)?;
        }
    }
}
fn interrupted(db: &Connection, session: &str, cohort: &[String]) -> Result<bool, StoreError> {
    if !valid_inputs(cohort) {
        return Ok(false);
    }
    let cohort = serde_json::to_string(cohort).expect("input IDs serialize");
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM event e WHERE e.aggregate_id=?1 AND e.type=?2
        AND NOT EXISTS(SELECT 1 FROM json_each(?3) owner
            WHERE NOT EXISTS(SELECT 1 FROM json_each(e.data,'$.input_message_ids') selected
                WHERE selected.value=owner.value)))",
        params![session, CHANGED, cohort],
        |row| row.get(0),
    )?)
}
/// Only identity/child metadata, in bounded pages; no Job payload/history retention.
fn job_page(
    db: &Connection,
    session: &str,
    ids: &str,
    after: &str,
) -> Result<Vec<(String, String, bool)>, StoreError> {
    let mut statement=db.prepare("SELECT j.id,coalesce(j.child_id,''),
        NOT EXISTS (SELECT 1 FROM json_each(j.data,'$.input_origin.input_message_ids') owner
            WHERE NOT EXISTS(SELECT 1 FROM json_each(?2) selected WHERE selected.value=owner.value))
        FROM job j WHERE j.session_id=?1
        AND j.status='running' AND j.id>?3
        AND json_type(j.data,'$.input_origin.input_message_ids')='array'
        AND json_array_length(j.data,'$.input_origin.input_message_ids') BETWEEN 1 AND 32
        AND json_type(j.data,'$.input_origin.message_id')='text' AND length(json_extract(j.data,'$.input_origin.message_id'))>0
        AND json_type(j.data,'$.input_origin.call_id')='text' AND length(json_extract(j.data,'$.input_origin.call_id'))>0
        AND json_type(j.data,'$.input_origin.operation_key')='text' AND length(json_extract(j.data,'$.input_origin.operation_key'))>0
        AND json_type(j.data,'$.input_origin.attempt')='integer' AND json_extract(j.data,'$.input_origin.attempt') BETWEEN 1 AND 4294967295
        AND NOT EXISTS (SELECT 1 FROM json_each(j.data,'$.input_origin.input_message_ids') owner
            WHERE owner.type<>'text' OR length(CAST(owner.value AS BLOB)) NOT BETWEEN 1 AND 256
)
        AND EXISTS (SELECT 1 FROM json_each(j.data,'$.input_origin.input_message_ids') owner
            JOIN json_each(?2) selected ON selected.value=owner.value)
        AND (SELECT count(DISTINCT owner.value) FROM json_each(j.data,'$.input_origin.input_message_ids') owner)
            =json_array_length(j.data,'$.input_origin.input_message_ids')
        ORDER BY j.id LIMIT 64")?;
    Ok(statement
        .query_map(params![session, ids, after], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn receipt() -> InputInterruptReceipt {
        InputInterruptReceipt {
            request_id: "stop".into(),
            input_message_ids: vec!["first".into()],
            generation: "generation".into(),
            status: InputInterruptStatus::Requested,
            session_seq: None,
            drain_selected: false,
            refused: 0,
            jobs_cancelled: 0,
            problems: vec![],
        }
    }
    #[test]
    fn pending_receipt_without_its_retained_worker_is_unknown_even_in_same_generation() {
        let observed = receipt().observed("generation", false);
        assert_eq!(observed.status, InputInterruptStatus::Unknown);
        assert_eq!(
            observed.problems,
            vec![InputInterruptProblem::ReconciliationRequired]
        );
        assert_eq!(
            receipt().observed("generation", true).status,
            InputInterruptStatus::Requested
        );
        assert_eq!(
            receipt().observed("replacement", true).status,
            InputInterruptStatus::Unknown
        );
        let mut acknowledged = receipt();
        acknowledged.status = InputInterruptStatus::Acknowledged;
        assert_eq!(
            acknowledged.observed("replacement", false).status,
            InputInterruptStatus::Acknowledged
        );
    }
    #[test]
    fn notifier_refusal_requires_valid_matching_immutable_job_provenance() {
        let mut registry = EventRegistry::default();
        registry.register("job.started.1").unwrap();
        register(&mut registry);
        let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            registry,
        ))
        .unwrap();
        store
            .append(
                "session",
                cyber_store::Expected::Any,
                vec![events::event(CHANGED, &receipt())],
            )
            .unwrap();
        let origin = json!({"message_id":"step","call_id":"call","operation_key":"key","attempt":1,"input_message_ids":["first"]});
        let notice = |id: &str, origin: serde_json::Value| {
            cyber_store::NewEvent::new(
                "job.notified.1",
                json!({"id":id,"session_id":"session","notified":true,"input_origin":origin}),
            )
        };
        let message = super::super::jobs::handback_id("job");
        let valid = notice("job", origin.clone());
        assert!(
            !notifier_refused(&store, "session", &message, std::slice::from_ref(&valid)).unwrap()
        );
        store
            .append(
                "session",
                cyber_store::Expected::Any,
                vec![cyber_store::NewEvent::new(
                    "job.started.1",
                    valid.data.clone(),
                )],
            )
            .unwrap();
        assert!(
            notifier_refused(&store, "session", &message, std::slice::from_ref(&valid)).unwrap()
        );
        assert!(
            !notifier_refused(
                &store,
                "session",
                "msg_forged",
                std::slice::from_ref(&valid)
            )
            .unwrap()
        );
        assert!(
            !notifier_refused(&store, "foreign", &message, std::slice::from_ref(&valid)).unwrap()
        );
        let different = notice("job", json!({"input_message_ids":["first"]}));
        assert!(!notifier_refused(&store, "session", &message, &[different]).unwrap());
        let mut malformed = origin;
        malformed["attempt"] = json!(0);
        let malformed = notice("malformed", malformed);
        store
            .append(
                "session",
                cyber_store::Expected::Any,
                vec![cyber_store::NewEvent::new(
                    "job.started.1",
                    malformed.data.clone(),
                )],
            )
            .unwrap();
        assert!(
            !notifier_refused(
                &store,
                "session",
                &super::super::jobs::handback_id("malformed"),
                &[malformed]
            )
            .unwrap()
        );
    }
    struct Retained {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl Drop for Retained {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
    impl super::super::ChildContinuation for Retained {
        fn settle(
            self: Box<Self>,
            _: bool,
            _: CancellationToken,
        ) -> futures::future::BoxFuture<'static, Result<Option<serde_json::Value>, String>>
        {
            Box::pin(async { Ok(None) })
        }
    }
    #[test]
    fn captured_completion_follows_actual_retained_owner_disposal() {
        let completed = CancellationToken::new();
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release) = std::sync::mpsc::channel();
        let entry = super::super::DrainEntry {
            _child_owner: None,
            child_settlement: Some(Box::new(Retained { entered, release })),
            child_settling: false,
            cancel: CancellationToken::new(),
            follow_up: false,
            deleting: false,
            completed: completed.clone(),
            _completion: completed.clone().drop_guard(),
        };
        let owner = std::thread::spawn(move || drop(entry));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !completed.is_cancelled(),
            "completion preceded retained-owner disposal"
        );
        release_tx.send(()).unwrap();
        owner.join().unwrap();
        assert!(completed.is_cancelled());
    }
    #[test]
    fn job_selection_pages_exact_session_and_reports_mixed_without_adopting_unknown_origins() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE job(id TEXT,session_id TEXT,child_id TEXT,status TEXT,data TEXT)",
        )
        .unwrap();
        let origin = |ids: serde_json::Value| json!({"message_id":"step","call_id":"call","operation_key":"key","attempt":1,"input_message_ids":ids});
        let insert = |id: &str, session: &str, origin: serde_json::Value| {
            db.execute(
                "INSERT INTO job VALUES(?1,?2,NULL,'running',?3)",
                params![id, session, json!({"input_origin":origin}).to_string()],
            )
            .unwrap();
        };
        for i in 0..130 {
            insert(
                &format!("owned_{i:03}"),
                "session",
                origin(json!(["first"])),
            );
        }
        insert("mixed", "session", origin(json!(["first", "second"])));
        insert("foreign", "other", origin(json!(["first"])));
        insert("unowned", "session", origin(json!(["second"])));
        insert("unknown", "session", serde_json::Value::Null);
        insert("duplicate", "session", origin(json!(["first", "first"])));
        insert("invalid", "session", origin(json!(["first", 1])));
        insert("oversize", "session", origin(json!(vec!["first"; 33])));
        let mut cursor = String::new();
        let mut seen = 0;
        let mut mixed = 0;
        loop {
            let page = job_page(&db, "session", "[\"first\"]", &cursor).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 64);
            for (id, child, complete) in page {
                cursor = id.clone();
                assert!(child.is_empty());
                if complete {
                    assert!(id.starts_with("owned_"));
                    seen += 1;
                } else {
                    assert_eq!(id, "mixed");
                    mixed += 1;
                }
            }
        }
        assert_eq!((seen, mixed), (130, 1));
    }
}
