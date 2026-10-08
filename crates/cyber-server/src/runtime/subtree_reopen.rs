//! Reviewed reopening with native ownership retained through writer verification.
use super::{
    CallStatus, Runtime, RuntimeError, SessionInfo, SessionState, SubtreeStopReport,
    SubtreeStopStatus,
};
use cyber_store::{StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;

pub(super) const REOPENED: &str = "session.admission.reopened.1";

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct SubtreeReopenReport {
    pub session_id: String,
    pub scope_id: String,
    pub stop_receipt_id: String,
    pub reopen_receipt_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SessionProof {
    id: String,
    created: String,
    latest: String,
    seq: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Proof {
    sessions: Vec<SessionProof>,
    jobs: Vec<(String, String)>,
    operations: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Reopened {
    scope_id: String,
    stop_receipt_id: String,
    proof: Proof,
}
struct Snapshot {
    proof: Proof,
    locations: Vec<SessionInfo>,
}

impl Runtime {
    /// Reopen only the reviewed acknowledged scope; do not dispatch preserved input.
    pub async fn reopen_subtree(
        &self,
        id: &str,
        scope_id: &str,
        stop_receipt_id: &str,
    ) -> Result<SubtreeReopenReport, RuntimeError> {
        let _coordinator = self.inner.subtree_stop_lock.lock().await;
        let _open = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        let root = id.to_owned();
        let scope = scope_id.to_owned();
        let receipt = stop_receipt_id.to_owned();
        let snapshot = self
            .inner
            .store
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let result = snapshot(&tx, &root, &scope, &receipt, None)
                    .map_err(|reason| StoreError::CorruptEvent { id: root, reason })?;
                tx.commit()?;
                Ok(result)
            })
            .map_err(|error| match error {
                StoreError::CorruptEvent { reason, .. } => RuntimeError::Conflict(reason),
                error => RuntimeError::from(error),
            })?;
        let ids: HashSet<_> = snapshot
            .locations
            .iter()
            .map(|info| info.id.clone())
            .collect();
        if self.inner.subtree_actors_live(&ids) || self.scoped_jobs_live(&ids)? {
            return Err(RuntimeError::Conflict(
                "Subtree actors still retain ownership".into(),
            ));
        }
        let proofs = tokio::time::timeout(
            Duration::from_secs(2),
            self.reopen_native_proofs(&snapshot.locations),
        )
        .await
        .map_err(|_| {
            RuntimeError::Conflict(
                "Native reopening proof timed out; admission remains closed".into(),
            )
        })??;
        if self.inner.subtree_actors_live(&ids) || self.scoped_jobs_live(&ids)? {
            return Err(RuntimeError::Conflict(
                "Subtree actors changed during native verification".into(),
            ));
        }
        let data = Reopened {
            scope_id: scope_id.into(),
            stop_receipt_id: stop_receipt_id.into(),
            proof: snapshot.proof,
        };
        let stored = self
            .inner
            .commit(&handle, vec![super::event(REOPENED, &data)])
            .await
            .map_err(|error| match error {
                RuntimeError::Store(StoreError::Projector { kind, reason }) if kind == REOPENED => {
                    RuntimeError::Conflict(reason)
                }
                RuntimeError::Store(error @ StoreError::Concurrency { .. }) => {
                    RuntimeError::Conflict(error.to_string())
                }
                error => error,
            })?;
        // Native ownership outlives the writer's acknowledgement.
        drop(proofs);
        Ok(SubtreeReopenReport {
            session_id: id.into(),
            scope_id: scope_id.into(),
            stop_receipt_id: stop_receipt_id.into(),
            reopen_receipt_id: stored[0].id.clone(),
        })
    }

    async fn reopen_native_proofs(
        &self,
        locations: &[SessionInfo],
    ) -> Result<Vec<Box<dyn Send>>, RuntimeError> {
        let mut proofs = Vec::with_capacity(locations.len());
        for info in locations {
            let handle = self.inner.handle(&info.id).await?;
            if handle.location_uncertain.load(Ordering::SeqCst) {
                return Err(RuntimeError::Conflict(
                    "Native Location requires reviewed recovery".into(),
                ));
            }
            let lease = self
                .inner
                .claim_location(info, false, self.inner.closed.child_token())
                .await?;
            if lease.worktree_id != info.worktree_id {
                return Err(RuntimeError::Conflict(
                    "Native Location identity changed".into(),
                ));
            }
            proofs.push(lease.settle_retained().map_err(RuntimeError::Conflict)?);
        }
        Ok(proofs)
    }
}

pub(super) fn active_scope(conn: &Connection, id: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT json_extract(c.data,'$.scope_id') FROM event c
         WHERE c.aggregate_id=?1 AND c.type='session.admission.fenced.1'
         AND json_extract(c.data,'$.closed')=1
         AND NOT EXISTS(SELECT 1 FROM event r WHERE r.aggregate_id=c.aggregate_id
             AND r.type='session.admission.reopened.1' AND r.seq>c.seq
             AND json_extract(r.data,'$.scope_id')=json_extract(c.data,'$.scope_id'))
         ORDER BY c.seq DESC LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let data: Reopened = serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    let current = snapshot(
        tx,
        &event.aggregate_id,
        &data.scope_id,
        &data.stop_receipt_id,
        Some(&event.id),
    )?;
    if current.proof != data.proof {
        return Err("Subtree reopening evidence changed before writer commit".into());
    }
    Ok(())
}

fn snapshot(
    conn: &Connection,
    root: &str,
    scope: &str,
    receipt: &str,
    exclude: Option<&str>,
) -> Result<Snapshot, String> {
    verify_review(conn, root, scope, receipt, exclude)?;
    if super::admission_authority::snapshot_chain(conn, root)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Err("Reopening ancestry is missing or cyclic".into());
    }
    let ids = descendants(conn, root)?;
    let pending = super::subtree_stop::pending_sweeps(conn, &ids.iter().cloned().collect(), None)?;
    if !pending.is_empty() {
        return Err(pending.join("; "));
    }
    let mut sessions = Vec::new();
    let mut locations = Vec::new();
    for id in &ids {
        let events = session_events(conn, id, exclude)?;
        if events
            .iter()
            .enumerate()
            .any(|(index, event)| event.seq != index as i64)
        {
            return Err(format!("Session {id} history has a sequence gap"));
        }
        let state = SessionState::replay(&events)?;
        if state.info.id != *id || events[0].kind != super::events::CREATED {
            return Err(format!(
                "Session {id} creation identity differs from its aggregate"
            ));
        }
        verify_session(&state)?;
        verify_projection(conn, &state)?;
        sessions.push(SessionProof {
            id: id.clone(),
            created: events[0].id.clone(),
            latest: events.last().unwrap().id.clone(),
            seq: state.last_seq,
        });
        locations.push(state.info);
    }
    Ok(Snapshot {
        proof: Proof {
            sessions,
            jobs: settled_jobs(conn, &ids)?,
            operations: settled_operations(conn, &ids)?,
        },
        locations,
    })
}

fn verify_review(
    conn: &Connection,
    root: &str,
    scope: &str,
    receipt: &str,
    exclude: Option<&str>,
) -> Result<(), String> {
    // The event currently being projected must not satisfy its own close review.
    let close: Option<(String, i64)> = conn.query_row(
        "SELECT json_extract(data,'$.scope_id'),seq FROM event WHERE aggregate_id=?1
         AND type='session.admission.fenced.1' AND json_extract(data,'$.closed')=1 ORDER BY seq DESC LIMIT 1",
        [root], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|e| e.to_string())?;
    let Some((current, close_seq)) = close else {
        return Err("No closed subtree scope to reopen".into());
    };
    if current != scope {
        return Err("Reopening review has a stale scope".into());
    }
    let reopened: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2 AND seq>?3 AND id IS NOT ?4
         AND json_extract(data,'$.scope_id')=?5)", params![root,REOPENED,close_seq,exclude,scope], |r|r.get(0),
    ).map_err(|e|e.to_string())?;
    if reopened {
        return Err("Subtree scope is already reopened".into());
    }
    let stop: Option<(String,String)> = conn.query_row(
        "SELECT id,data FROM event WHERE aggregate_id=?1 AND type='session.subtree.stopped.1' AND seq>?2 ORDER BY seq DESC LIMIT 1",
        params![root,close_seq], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional().map_err(|e|e.to_string())?;
    let Some((latest, data)) = stop else {
        return Err("Subtree stop has no persisted acknowledgement".into());
    };
    let report: SubtreeStopReport = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    if latest != receipt
        || report.session_id != root
        || report.scope_id != scope
        || report.status != SubtreeStopStatus::Acknowledged
        || !report.persisted
        || !report.problems.is_empty()
    {
        return Err("Reopening requires the latest matching acknowledged stop receipt".into());
    }
    Ok(())
}

fn descendants(conn: &Connection, root: &str) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::from([root.to_owned()]);
    let mut pending = vec![root.to_owned()];
    while let Some(parent) = pending.pop() {
        let mut query = conn
            .prepare("SELECT id FROM session WHERE parent_id=?1")
            .map_err(|e| e.to_string())?;
        let children = query
            .query_map([&parent], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for child in children {
            if !ids.insert(child.clone()) {
                return Err("Reopening descendants contain a cycle".into());
            }
            pending.push(child);
        }
    }
    Ok(ids)
}

fn session_events(
    conn: &Connection,
    id: &str,
    exclude: Option<&str>,
) -> Result<Vec<StoredEvent>, String> {
    let mut query = conn
        .prepare(
            "SELECT id,aggregate_id,seq,type,data,time,causation_id FROM event
        WHERE aggregate_id=?1 AND id IS NOT ?2 ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![id, exclude], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    rows.into_iter()
        .map(
            |(id, aggregate_id, seq, kind, data, time_ms, causation_id)| {
                Ok(StoredEvent {
                    id,
                    aggregate_id,
                    seq,
                    kind,
                    data: serde_json::from_str(&data).map_err(|e| e.to_string())?,
                    time_ms,
                    causation_id,
                })
            },
        )
        .collect()
}

fn verify_session(state: &SessionState) -> Result<(), String> {
    if state.child_continuation_unknown
        || state.child_worktree_setup_pending
        || state.open_step.is_some()
        || state.calls.values().any(|c| {
            matches!(
                c.status,
                CallStatus::Dispatched | CallStatus::OutcomeUnknown
            )
        })
    {
        return Err(format!(
            "Session {} requires execution or preparation recovery",
            state.info.id
        ));
    }
    Ok(())
}

fn verify_projection(conn: &Connection, state: &SessionState) -> Result<(), String> {
    let row = conn
        .query_row(
            "SELECT directory,parent_id,last_seq,agent,model,mode FROM session WHERE id=?1",
            [&state.info.id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    let expected = (
        state.info.directory.clone(),
        state.info.parent_id.clone(),
        state.last_seq,
        state.info.agent.clone(),
        state.info.model.clone(),
        state.info.mode.clone(),
    );
    if row != expected {
        return Err(format!(
            "Session {} projection differs from durable history",
            state.info.id
        ));
    }
    Ok(())
}

fn settled_jobs(
    conn: &Connection,
    ids: &BTreeSet<String>,
) -> Result<Vec<(String, String)>, String> {
    let mut query = conn
        .prepare("SELECT id,session_id,child_id,status,data FROM job ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut stamps = Vec::new();
    for (id, parent, child, status, data) in rows {
        if !ids.contains(&parent) && !child.as_ref().is_some_and(|id| ids.contains(id)) {
            continue;
        }
        if !matches!(
            status.as_str(),
            "completed" | "error" | "cancelled" | "interrupted"
        ) {
            return Err(format!("Job {id} lacks settled ownership"));
        }
        let job: super::Job = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        if job.id != id
            || job.session_id != parent
            || child.as_deref() != Some(job.child_id.as_str())
            || serde_json::to_value(job.status).map_err(|e| e.to_string())? != status
        {
            return Err(format!("Job {id} projection differs from ownership data"));
        }
        stamps.push((id, format!("{:x}", Sha256::digest(data.as_bytes()))));
    }
    Ok(stamps)
}

fn settled_operations(conn: &Connection, ids: &BTreeSet<String>) -> Result<Vec<String>, String> {
    let mut query=conn.prepare("SELECT id,type,data,(SELECT data FROM event origin WHERE origin.aggregate_id=e.aggregate_id ORDER BY seq LIMIT 1) FROM event e WHERE type IN
        ('child.execution.changed.1','native.activity.changed.1','delegation.changed.1')
        AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) ORDER BY id")
        .map_err(|e|e.to_string())?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut stamps = Vec::new();
    let scope = ids.iter().cloned().collect();
    for (id, kind, data, origin) in rows {
        let origin = serde_json::from_str(&origin).map_err(|e| e.to_string())?;
        let origin_scoped = super::admission_authority::origin_scoped(&origin, &scope)?;
        let data: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let (scoped, settled) = operation_status(&kind, &data, ids)?;
        if !scoped && !origin_scoped {
            continue;
        }
        if !settled {
            return Err(format!("Operation {id} requires ownership recovery"));
        }
        stamps.push(id);
    }
    Ok(stamps)
}

fn operation_status(
    kind: &str,
    data: &Value,
    ids: &BTreeSet<String>,
) -> Result<(bool, bool), String> {
    match kind {
        "child.execution.changed.1" => {
            let record: super::child_ownership::Record =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            Ok((
                ids.contains(&record.parent_id) || ids.contains(&record.child_id),
                record.status == "released",
            ))
        }
        "native.activity.changed.1" => {
            let record: super::activity::Record =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            Ok((ids.contains(&record.session_id), record.status == "settled"))
        }
        "delegation.changed.1" => {
            let record: super::Delegation =
                serde_json::from_value(data["data"].clone()).map_err(|e| e.to_string())?;
            Ok((
                ids.contains(&record.session_id),
                matches!(
                    record.status,
                    super::DelegationStatus::Admitted
                        | super::DelegationStatus::Cancelled
                        | super::DelegationStatus::Failed
                ),
            ))
        }
        _ => Err("Unknown operation ledger".into()),
    }
}
