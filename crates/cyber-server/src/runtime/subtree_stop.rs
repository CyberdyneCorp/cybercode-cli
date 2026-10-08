//! Owned local actor sweep. Acknowledgement never implies reopening admission.
use super::{CallStatus, DelegationStatus, Inner, JobStatus, Runtime, RuntimeError};
use futures::FutureExt;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) const STARTED: &str = "session.subtree.stopping.1";
pub(super) const SETTLED: &str = "session.subtree.stopped.1";
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubtreeStopStatus {
    Acknowledged,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubtreeStopReport {
    pub session_id: String,
    pub scope_id: String,
    pub status: SubtreeStopStatus,
    pub problems: Vec<String>,
    pub persisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep_id: Option<String>,
}
pub(super) struct Control {
    done: CancellationToken,
    report: std::sync::Mutex<Option<SubtreeStopReport>>,
}
impl Runtime {
    /// Close admission and stop known local subtree actors; admission stays closed.
    /// Unknown native/cross-process outcomes require recovery, never implicit reopening.
    pub async fn stop_subtree(&self, id: &str) -> Result<SubtreeStopReport, RuntimeError> {
        let coordinator = self.inner.subtree_stop_lock.lock().await;
        drop(self.inner.open().await?);
        let existing = self
            .inner
            .subtree_stops
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .filter(|control| !control.done.is_cancelled())
            .cloned();
        let control = if let Some(control) = existing {
            control
        } else {
            let root = id.to_owned();
            self.inner.handle(id).await?;
            self.inner.descendants(id)?;
            let lookup = root.clone();
            let scope = self
                .inner
                .store
                .read(move |db| super::subtree_reopen::active_scope(db, &lookup))?;
            let scope = match scope {
                Some(scope) => scope,
                None => self.close_subtree_admissions(id).await?,
            };
            let ids = self.inner.descendants(id)?;
            let sweep = cyber_core::ids::new_id("op");
            let handle = self.inner.handle(id).await?;
            self.inner
                .commit(
                    &handle,
                    vec![super::event(
                        STARTED,
                        &serde_json::json!({"scope_id":scope,"sweep_id":sweep}),
                    )],
                )
                .await?;
            let control = Arc::new(Control {
                done: CancellationToken::new(),
                report: Default::default(),
            });
            self.inner
                .subtree_stops
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(root.clone(), control.clone());
            let weak = self.downgrade();
            let owned = control.clone();
            let task = tokio::spawn(async move {
                let _done = owned.done.clone().drop_guard();
                if let Some(runtime) = weak.upgrade() {
                    let result = std::panic::AssertUnwindSafe(runtime.sweep_subtree(&ids, &sweep))
                        .catch_unwind()
                        .await;
                    let mut problems = match result {
                        Ok(Ok(problems)) => problems,
                        Ok(Err(error)) => vec![error.to_string()],
                        Err(_) => vec!["Subtree stop worker panicked; recovery is required".into()],
                    };
                    problems.sort();
                    problems.dedup();
                    let mut report = SubtreeStopReport {
                        session_id: root.clone(),
                        scope_id: scope,
                        status: if problems.is_empty() {
                            SubtreeStopStatus::Acknowledged
                        } else {
                            SubtreeStopStatus::Unknown
                        },
                        problems,
                        persisted: true,
                        receipt_id: None,
                        sweep_id: Some(sweep),
                    };
                    match runtime.record_subtree_stop(&report).await {
                        Ok(receipt) => report.receipt_id = Some(receipt),
                        Err(error) => {
                            report.persisted = false;
                            report.status = SubtreeStopStatus::Unknown;
                            report
                                .problems
                                .push(format!("Stop receipt could not be recorded: {error}"));
                        }
                    }
                    *owned.report.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
                }
            });
            let mut tasks = self
                .inner
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
            control
        };
        drop(coordinator);
        control.done.cancelled().await;
        control
            .report
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                RuntimeError::Invalid(
                    "Subtree stop owner was lost; admission remains closed".into(),
                )
            })
    }
    async fn record_subtree_stop(
        &self,
        report: &SubtreeStopReport,
    ) -> Result<String, RuntimeError> {
        let handle = self.inner.handle(&report.session_id).await?;
        let stored = self
            .inner
            .commit(&handle, vec![super::event(SETTLED, report)])
            .await?;
        Ok(stored[0].id.clone())
    }
    async fn sweep_subtree(
        &self,
        ids: &[String],
        sweep: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        let ids: HashSet<String> = ids.iter().cloned().collect();
        self.inner.signal_subtree(&ids, &self.jobs(None)?);
        let attempt = async {
            for id in &ids {
                self.pause_requested_child_input(id).await.0?;
                self.inner.abandon_requests(id).await?;
            }
            for (source, operation) in self.scoped_delegations(&ids)? {
                self.cancel_delegation(&source, &operation).await?;
            }
            while self.inner.subtree_actors_live(&ids) || self.scoped_jobs_live(&ids)? {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            self.audit_subtree_stop(&ids, sweep).await
        };
        match tokio::time::timeout(Duration::from_secs(2), attempt).await {
            Ok(result) => result,
            Err(_) => Ok(vec![
                "Subtree actors did not acknowledge within two seconds; admission remains closed"
                    .into(),
            ]),
        }
    }
    pub(super) fn scoped_jobs_live(&self, ids: &HashSet<String>) -> Result<bool, RuntimeError> {
        let jobs = self.jobs(None)?;
        let controls = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(jobs
            .iter()
            .filter(|job| ids.contains(&job.session_id) || ids.contains(&job.child_id))
            .any(|job| {
                controls
                    .get(&job.id)
                    .is_some_and(|control| !control.done.is_cancelled())
            }))
    }
    fn scoped_delegations(
        &self,
        ids: &HashSet<String>,
    ) -> Result<Vec<(String, String)>, RuntimeError> {
        let rows=self.inner.store.read(|db|{
            let mut statement=db.prepare("SELECT aggregate_id,json_extract(data,'$.data.session_id'),(SELECT data FROM event origin WHERE origin.aggregate_id=e.aggregate_id ORDER BY seq LIMIT 1) FROM event e WHERE type='delegation.changed.1' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)")?;
            Ok(statement.query_map([],|row|Ok((row.get::<_,String>(1)?,row.get::<_,String>(0)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?)
        })?;
        let mut scoped = Vec::new();
        for (source, operation, origin) in rows {
            let origin =
                serde_json::from_str(&origin).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
            if ids.contains(&source)
                || super::admission_authority::origin_scoped(&origin, ids)
                    .map_err(RuntimeError::Corrupt)?
            {
                scoped.push((source, operation));
            }
        }
        Ok(scoped)
    }

    async fn audit_subtree_stop(
        &self,
        ids: &HashSet<String>,
        sweep: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        let lookup_ids = ids.clone();
        let lookup_sweep = sweep.to_owned();
        let mut problems = self.inner.store.read(move |db| {
            pending_sweeps(db, &lookup_ids, Some(&lookup_sweep)).map_err(|reason| {
                cyber_store::StoreError::CorruptEvent {
                    id: lookup_sweep,
                    reason,
                }
            })
        })?;
        for job in self.jobs(None)? {
            if (ids.contains(&job.session_id) || ids.contains(&job.child_id))
                && job.status == JobStatus::Running
            {
                problems.push(format!("Running Job {} has no settled owner", job.id));
            }
        }
        for (source, operation) in self.scoped_delegations(ids)? {
            if self.delegation(&source, &operation)?.is_some_and(|data| {
                matches!(
                    data.status,
                    DelegationStatus::Pending
                        | DelegationStatus::Cancelling
                        | DelegationStatus::Unknown
                )
            }) {
                problems.push(format!("Delegation {operation} requires recovery"));
            }
        }
        self.audit_idle_receipts(ids, &mut problems)?;
        for (operation, owner) in super::child_ownership::held(&self.inner.store)? {
            if ids.contains(&owner.parent_id)
                || ids.contains(&owner.child_id)
                || owner.admission_bindings.as_ref().is_some_and(|bindings| {
                    !super::admission_authority::binding_sessions(bindings).is_disjoint(ids)
                })
            {
                problems.push(format!(
                    "Child result owner {operation} requires acknowledgement or recovery"
                ));
            }
        }
        let mut leases = Vec::new();
        for id in ids {
            let handle = self.inner.handle(id).await?;
            let state = handle.state.lock().await.clone();
            if handle
                .location_uncertain
                .load(std::sync::atomic::Ordering::SeqCst)
                || state.child_continuation_unknown
                || state.child_worktree_setup_pending
                || state.open_step.is_some()
                || state.calls.values().any(|call| {
                    matches!(
                        call.status,
                        CallStatus::Dispatched | CallStatus::OutcomeUnknown
                    )
                })
            {
                problems.push(format!(
                    "Session {id} retains unknown execution or preparation"
                ));
            }
            let cancel = CancellationToken::new();
            match self.inner.claim_location(&state.info, false, cancel).await {
                Ok(lease) => leases.push(lease),
                Err(error) => {
                    problems.push(format!("Session {id} Location acknowledgement: {error}"))
                }
            }
        }
        for lease in leases {
            if let Err(error) = lease.settle() {
                problems.push(format!("Location acknowledgement: {error}"));
            }
        }
        Ok(problems)
    }
    fn audit_idle_receipts(
        &self,
        ids: &HashSet<String>,
        problems: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let records=self.inner.store.read(|db|{
            let mut statement=db.prepare("SELECT aggregate_id,data,(SELECT data FROM event origin WHERE origin.aggregate_id=e.aggregate_id ORDER BY seq LIMIT 1) FROM event e WHERE type='native.activity.changed.1' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)")?;
            Ok(statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?)
        })?;
        for (operation, data, origin) in records {
            let origin =
                serde_json::from_str(&origin).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
            let scoped = super::admission_authority::origin_scoped(&origin, ids)
                .map_err(RuntimeError::Corrupt)?;
            let data: super::activity::Record = serde_json::from_str(&data)
                .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
            if (ids.contains(&data.session_id) || scoped) && data.status != "settled" {
                problems.push(format!(
                    "Idle operation {operation} retains {} evidence",
                    data.status
                ));
            }
        }
        Ok(())
    }
}
impl Inner {
    fn signal_subtree(&self, ids: &HashSet<String>, jobs: &[super::Job]) {
        {
            let mut drains = self.drains.lock().unwrap_or_else(PoisonError::into_inner);
            for (id, entry) in drains.iter_mut().filter(|(id, _)| ids.contains(*id)) {
                let _ = id;
                entry.follow_up = false;
                entry.cancel.cancel();
            }
        }
        for (id, control) in self
            .child_preparations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if ids.contains(id) {
                control.cancel.cancel();
            }
        }
        for control in self
            .delegations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            if ids.contains(&control.source) || !control.admission_sessions.is_disjoint(ids) {
                control.stop.cancel();
            }
        }
        for control in self
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            if ids.contains(&control.source) || !control.admission_sessions.is_disjoint(ids) {
                control.stop.cancel();
            }
        }
        let controls = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        for job in jobs
            .iter()
            .filter(|job| ids.contains(&job.session_id) || ids.contains(&job.child_id))
        {
            if let Some(control) = controls.get(&job.id) {
                if ids.contains(&job.session_id) {
                    control
                        .notify
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                }
                control.stop.cancel();
            }
        }
    }
    pub(super) fn subtree_actors_live(&self, ids: &HashSet<String>) -> bool {
        if self
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .any(|id| ids.contains(id))
        {
            return true;
        }
        if self
            .child_preparations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|(id, c)| ids.contains(id) && !c.done.is_cancelled())
        {
            return true;
        }
        if self
            .delegations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|c| {
                (ids.contains(&c.source) || !c.admission_sessions.is_disjoint(ids))
                    && !c.done.is_cancelled()
            })
        {
            return true;
        }
        if self
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|c| {
                (ids.contains(&c.source) || !c.admission_sessions.is_disjoint(ids))
                    && !c.done.is_cancelled()
            })
        {
            return true;
        }
        self.child_executions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|(id, owner)| {
                ids.contains(id)
                    && owner
                        .upgrade()
                        .is_some_and(|mutex| mutex.try_lock().is_err())
            })
    }
}

pub(super) fn project(
    tx: &rusqlite::Transaction<'_>,
    event: &cyber_store::StoredEvent,
) -> Result<(), String> {
    let report: SubtreeStopReport =
        serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
    if report.session_id != event.aggregate_id
        || !report.persisted
        || (report.status == SubtreeStopStatus::Acknowledged) != report.problems.is_empty()
    {
        return Err("Invalid subtree stop receipt".into());
    }
    let active =
        super::subtree_reopen::active_scope(tx, &event.aggregate_id).map_err(|e| e.to_string())?;
    if active.as_deref() != Some(&report.scope_id) {
        return Err("Subtree stop receipt has a different active close owner".into());
    }
    let sweep = report
        .sweep_id
        .as_deref()
        .ok_or("Missing subtree sweep ownership")?;
    let valid: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2
         AND json_extract(data,'$.scope_id')=?3 AND json_extract(data,'$.sweep_id')=?4)",
            rusqlite::params![event.aggregate_id, STARTED, report.scope_id, sweep],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !valid {
        return Err("Subtree stop receipt has no owned sweep".into());
    }
    let terminal: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2 AND seq<?3
         AND json_extract(data,'$.sweep_id')=?4)",
            rusqlite::params![event.aggregate_id, SETTLED, event.seq, sweep],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if terminal {
        return Err("Subtree sweep already has a terminal receipt".into());
    }
    let scope:Option<String>=tx.query_row(
        "SELECT json_extract(data,'$.scope_id') FROM event WHERE aggregate_id=?1 AND type='session.admission.fenced.1' AND json_extract(data,'$.closed')=1 ORDER BY seq DESC LIMIT 1",
        [&event.aggregate_id],|row|row.get(0),
    ).optional().map_err(|error|error.to_string())?;
    if scope.as_deref() != Some(&report.scope_id) {
        return Err("Subtree stop receipt has a different close owner".into());
    }
    Ok(())
}

pub(super) fn project_started(
    tx: &rusqlite::Transaction<'_>,
    event: &cyber_store::StoredEvent,
) -> Result<(), String> {
    let scope = event.data["scope_id"]
        .as_str()
        .ok_or("Missing sweep scope")?;
    let sweep = event.data["sweep_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .ok_or("Missing sweep identity")?;
    if super::subtree_reopen::active_scope(tx, &event.aggregate_id)
        .map_err(|e| e.to_string())?
        .as_deref()
        != Some(scope)
    {
        return Err("Sweep has a stale close scope".into());
    }
    let duplicate: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM event WHERE type=?1 AND id!=?2 AND json_extract(data,'$.sweep_id')=?3)",
        rusqlite::params![STARTED,event.id,sweep], |r|r.get(0)).map_err(|e|e.to_string())?;
    if duplicate {
        return Err("Sweep identity is already owned".into());
    }
    Ok(())
}

pub(super) fn pending_sweeps(
    conn: &rusqlite::Connection,
    ids: &HashSet<String>,
    exclude: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut query = conn.prepare("SELECT aggregate_id,json_extract(data,'$.sweep_id') FROM event s WHERE type=?1
        AND NOT EXISTS(SELECT 1 FROM event e WHERE e.aggregate_id=s.aggregate_id AND e.type=?2 AND e.seq>s.seq
            AND json_extract(e.data,'$.sweep_id')=json_extract(s.data,'$.sweep_id'))").map_err(|e|e.to_string())?;
    let rows = query
        .query_map(rusqlite::params![STARTED, SETTLED], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|(source, sweep)| ids.contains(source) && Some(sweep.as_str()) != exclude)
        .map(|(_, sweep)| format!("Subtree sweep {sweep} requires acknowledgement or recovery"))
        .collect())
}
