//! Captured source/ancestor authority, checked again by the single database writer.
use std::collections::HashSet;
use std::future::Future;
use tokio_util::sync::CancellationToken;

tokio::task_local! { static INHERITED: AdmissionAuthority; }
use std::sync::{Arc, Weak};

use cyber_store::{StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{Inner, Runtime, RuntimeError};

pub(super) const STALE: &str = "Child admission was fenced by cancellation or changed ancestry";

pub(super) const CLOSED: &str =
    "Subtree admission is closed pending cancellation acknowledgement or recovery";

pub(super) const FENCED: &str = "session.admission.fenced.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Binding {
    pub(super) id: String,
    created_id: String,
    parent: Option<String>,
    subtree_seq: i64,
    /// Ordinary interruption invalidates only launches sourced by this Session.
    local_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    guards: Vec<Boundary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Boundary {
    id: String,
    created_id: String,
    parent: Option<String>,
    subtree_seq: i64,
    local_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    operations: Vec<String>,
}

#[derive(Clone)]
pub struct AdmissionAuthority {
    runtime: Weak<Inner>,
    cancellations: Vec<CancellationToken>,
    pub(super) bindings: Vec<Binding>,
}

impl std::fmt::Debug for AdmissionAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmissionAuthority")
            .field("bindings", &self.bindings)
            .finish()
    }
}

impl AdmissionAuthority {
    pub(super) fn with_target(mut self, target: Self) -> Self {
        let guards = &mut self.bindings.first_mut().expect("captured source").guards;
        for binding in target.bindings {
            guards.push(Boundary {
                id: binding.id,
                created_id: binding.created_id,
                parent: binding.parent,
                subtree_seq: binding.subtree_seq,
                local_seq: binding.local_seq,
                operations: binding.operations,
            });
            guards.extend(binding.guards);
        }
        self.cancellations.extend(target.cancellations);
        self
    }
    pub(super) fn with_operation(mut self, id: String) -> Self {
        self.bindings
            .first_mut()
            .expect("captured source")
            .operations
            .push(id);
        self
    }

    pub(super) fn with_cancellation(mut self, cancel: CancellationToken) -> Self {
        self.cancellations.push(cancel);
        self
    }

    /// Retain launch authority across a host callback without inheriting it into spawned Drains.
    pub fn run<'a, F>(
        self,
        cancel: CancellationToken,
        future: F,
    ) -> futures::future::BoxFuture<'a, F::Output>
    where
        F: Future + Send + 'a,
        F::Output: Send + 'a,
    {
        Box::pin(self.scope(cancel, future))
    }

    pub(super) fn scope<F: Future>(
        mut self,
        cancel: CancellationToken,
        future: F,
    ) -> impl Future<Output = F::Output> {
        self.cancellations.push(cancel);
        INHERITED.scope(self, Box::pin(future))
    }

    pub fn verify(&self, runtime: &Runtime, source: &str) -> Result<(), RuntimeError> {
        if !Weak::ptr_eq(&self.runtime, &Arc::downgrade(&runtime.inner))
            || self.bindings.first().map(|b| b.id.as_str()) != Some(source)
        {
            return Err(RuntimeError::Conflict(
                "Child admission authority has a different owner".into(),
            ));
        }
        if self
            .cancellations
            .iter()
            .any(CancellationToken::is_cancelled)
        {
            return Err(RuntimeError::Conflict(STALE.into()));
        }
        runtime.verify_admission_bindings(&self.bindings)
    }
}

impl Runtime {
    pub(super) fn verify_admission_bindings(
        &self,
        bindings: &[Binding],
    ) -> Result<(), RuntimeError> {
        let bindings = bindings.to_vec();
        let valid = self.inner.store.read(move |conn| {
            let snapshot = conn.unchecked_transaction()?;
            let valid = bindings_current(&snapshot, &bindings)?;
            snapshot.commit()?;
            Ok(valid)
        })?;
        if !valid {
            return Err(RuntimeError::Conflict(STALE.into()));
        }
        Ok(())
    }

    /// Capture before approval, concurrency waits or native child preparation.
    pub fn capture_child_admission(
        &self,
        source: &str,
    ) -> Result<AdmissionAuthority, RuntimeError> {
        if let Some(authority) = self.inherited_child_admission(source)? {
            return Ok(authority);
        }
        self.snapshot_child_admission(source)
    }

    pub(super) fn inherited_child_admission(
        &self,
        source: &str,
    ) -> Result<Option<AdmissionAuthority>, RuntimeError> {
        let Some(inherited) = INHERITED.try_with(Clone::clone).ok() else {
            return Ok(None);
        };
        let inherited_source = inherited
            .bindings
            .first()
            .expect("captured source")
            .id
            .as_str();
        if inherited_source == source {
            inherited.verify(self, source)?;
            return Ok(Some(inherited));
        }
        let mut current = self.snapshot_child_admission(source)?;
        let Some(index) = current
            .bindings
            .iter()
            .position(|binding| binding.id == inherited_source)
        else {
            if inherited
                .bindings
                .iter()
                .any(|binding| binding.id == source)
            {
                inherited.verify(self, inherited_source)?;
                return Ok(Some(current.with_target(inherited)));
            }
            return Ok(None);
        };
        inherited.verify(self, inherited_source)?;
        // Until handoff, nested callback work still belongs to the original launch source.
        current
            .bindings
            .splice(index.., inherited.bindings.iter().cloned());
        current.cancellations = inherited.cancellations;
        Ok(Some(current))
    }

    pub(super) fn callback_admission_bindings(
        &self,
        session_id: &str,
    ) -> Result<Option<Vec<Binding>>, RuntimeError> {
        Ok(self
            .inherited_child_admission(session_id)?
            .map(|authority| authority.bindings))
    }

    fn snapshot_child_admission(&self, source: &str) -> Result<AdmissionAuthority, RuntimeError> {
        Ok(AdmissionAuthority {
            runtime: Arc::downgrade(&self.inner),
            cancellations: Vec::new(),
            bindings: self.inner.open_admission_chain(source)?,
        })
    }

    /// Close fresh admission for an owned subtree sweep; this does not stop actors.
    /// The scope remains closed until acknowledged settlement or reviewed recovery is implemented.
    pub async fn close_subtree_admissions(&self, id: &str) -> Result<String, RuntimeError> {
        let _open = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        let mut state = handle.state.lock().await;
        self.inner.ensure_admission_open(id)?;
        self.inner.descendants(id)?;
        let scope = cyber_core::ids::new_id("op");
        self.inner.commit_locked(
            &mut state,
            vec![super::event(
                FENCED,
                &serde_json::json!({"closed":true,"scope_id":scope}),
            )],
        )?;
        Ok(scope)
    }

    /// Durable launch boundary for a subsequent owned subtree cancellation sweep.
    /// This invalidates captured launch authority; it does not stop running work.
    pub async fn fence_subtree_admissions(&self, id: &str) -> Result<(), RuntimeError> {
        let _open = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        let mut state = handle.state.lock().await;
        self.inner.commit_locked(
            &mut state,
            vec![super::event(FENCED, &serde_json::json!({}))],
        )?;
        Ok(())
    }
}

impl Inner {
    pub(super) fn ensure_admission_open(&self, source: &str) -> Result<(), RuntimeError> {
        self.open_admission_chain(source).map(|_| ())
    }

    fn open_admission_chain(&self, source: &str) -> Result<Vec<Binding>, RuntimeError> {
        let source_id = source.to_owned();
        let source = source_id.clone();
        let (exists, bindings, open) = self.store.read(move |conn| {
            let snapshot = conn.unchecked_transaction()?;
            let exists = snapshot.query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1)",
                [&source],
                |row| row.get::<_, bool>(0),
            )?;
            let bindings = snapshot_chain(&snapshot, &source)?;
            let open = match &bindings {
                Some(bindings) => chain_is_open(&snapshot, bindings)?,
                None => false,
            };
            snapshot.commit()?;
            Ok((exists, bindings, open))
        })?;
        if !exists {
            return Err(RuntimeError::SessionNotFound(source_id));
        }
        let bindings = bindings.ok_or_else(|| {
            RuntimeError::Corrupt(
                "Child admission ancestry has a cycle or a missing ancestor".into(),
            )
        })?;
        if !open {
            return Err(RuntimeError::Conflict(CLOSED.into()));
        }
        Ok(bindings)
    }
}

fn chain_is_open(conn: &Connection, bindings: &[Binding]) -> Result<bool, StoreError> {
    for binding in bindings {
        let closed = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2
             AND json_extract(data,'$.closed')=1)",
            params![binding.id, FENCED],
            |row| row.get::<_, bool>(0),
        )?;
        if closed {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn existing_target_open(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1)",
        [id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(true);
    }
    let Some(chain) = snapshot_chain(conn, id)? else {
        return Ok(false);
    };
    chain_is_open(conn, &chain)
}

fn snapshot_chain(conn: &Connection, source: &str) -> Result<Option<Vec<Binding>>, StoreError> {
    let mut next = Some(source.to_string());
    let mut seen = HashSet::new();
    let mut bindings = Vec::new();
    while let Some(id) = next {
        if !seen.insert(id.clone()) {
            return Ok(None);
        }
        let parent: Option<Option<String>> = conn
            .query_row("SELECT parent_id FROM session WHERE id=?1", [&id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(parent) = parent else {
            return Ok(None);
        };
        let created_id: Option<String> = conn
            .query_row(
                "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
                params![id, super::events::CREATED],
                |row| row.get(0),
            )
            .optional()?;
        let Some(created_id) = created_id else {
            return Ok(None);
        };
        let subtree_seq = sequence(conn, &id, FENCED)?;
        let local_seq = if bindings.is_empty() {
            Some(sequence(conn, &id, super::events::CHILD_INPUT_PAUSED)?)
        } else {
            None
        };
        next = parent.clone();
        bindings.push(Binding {
            id,
            created_id,
            parent,
            subtree_seq,
            local_seq,
            operations: Vec::new(),
            guards: Vec::new(),
        });
    }
    Ok(Some(bindings))
}

fn sequence(conn: &Connection, id: &str, kind: &str) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq),-1) FROM event WHERE aggregate_id=?1 AND type=?2",
        params![id, kind],
        |r| r.get(0),
    )?)
}

pub(super) fn bindings_current(
    conn: &Connection,
    captured: &[Binding],
) -> Result<bool, StoreError> {
    let Some(source) = captured.first() else {
        return Ok(false);
    };
    if source.local_seq.is_none() {
        return Ok(false);
    }
    let Some(mut current) = snapshot_chain(conn, &source.id)? else {
        return Ok(false);
    };
    if current.len() != captured.len() || !chain_is_open(conn, &current)? {
        return Ok(false);
    }
    for (current, captured) in current.iter_mut().zip(captured) {
        if !operations_current(conn, captured)? || !guards_current(conn, &captured.guards)? {
            return Ok(false);
        }
        current.operations = captured.operations.clone();
        current.guards = captured.guards.clone();
        if captured.local_seq.is_some() && current.local_seq.is_none() {
            current.local_seq = Some(sequence(
                conn,
                &current.id,
                super::events::CHILD_INPUT_PAUSED,
            )?);
        }
    }
    Ok(current == captured)
}

fn guards_current(conn: &Connection, guards: &[Boundary]) -> Result<bool, StoreError> {
    for guard in guards {
        let Some(chain) = snapshot_chain(conn, &guard.id)? else {
            return Ok(false);
        };
        if !chain_is_open(conn, &chain)? {
            return Ok(false);
        }
        let mut current = chain[0].clone();
        current.operations = guard.operations.clone();
        if !operations_current(conn, &current)? {
            return Ok(false);
        }
        if current.created_id != guard.created_id
            || current.parent != guard.parent
            || current.subtree_seq != guard.subtree_seq
            || guard
                .local_seq
                .is_some_and(|seq| current.local_seq != Some(seq))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn operations_current(conn: &Connection, binding: &Binding) -> Result<bool, StoreError> {
    for operation in &binding.operations {
        let record = conn.query_row(
            "SELECT CASE type WHEN 'child.execution.changed.1' THEN json_extract(data,'$.source_id') ELSE json_extract(data,'$.data.session_id') END,
             CASE type WHEN 'child.execution.changed.1' THEN CASE json_extract(data,'$.status') WHEN 'held' THEN 'pending' ELSE 'released' END ELSE json_extract(data,'$.data.status') END
             FROM event WHERE aggregate_id=?1 AND type IN ('delegation.changed.1','child.execution.changed.1') ORDER BY seq DESC LIMIT 1",
            [operation], |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        ).optional()?;
        if !record.is_some_and(|(source, status)| {
            source.as_deref() == Some(binding.id.as_str()) && status.as_deref() == Some("pending")
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if event.kind == super::child_ownership::CHANGED {
        return super::child_ownership::project(tx, event);
    }
    if event.kind == super::subtree_stop::SETTLED {
        return super::subtree_stop::project(tx, event);
    }
    check_closed_admission(tx, event)?;
    let source = match event.kind.as_str() {
        super::events::CREATED => event.data["info"]["parent_id"].as_str().map(str::to_string),
        super::events::ADMITTED | super::events::RESUMED => tx
            .query_row(
                "SELECT parent_id FROM session WHERE id=?1",
                [&event.aggregate_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten(),
        "job.started.1" => Some(event.aggregate_id.clone()),
        super::activity::CHANGED if event.data["status"] == "pending" => {
            event.data["session_id"].as_str().map(str::to_string)
        }
        "delegation.changed.1" if event.data["data"]["status"] == "pending" => {
            event.data["data"]["session_id"]
                .as_str()
                .map(str::to_string)
        }
        _ => return Ok(()),
    };
    let Some(value) = event.data.get("admission_bindings") else {
        return Ok(());
    };
    let bindings: Vec<Binding> =
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let Some(captured) = bindings.first() else {
        return Err("Empty child admission authority".into());
    };
    let self_source = matches!(
        event.kind.as_str(),
        super::events::ADMITTED | super::events::RESUMED
    ) && captured.id == event.aggregate_id;
    if !self_source && source.as_deref() != Some(captured.id.as_str()) {
        return Err("Child admission source differs from its declared parent".into());
    }
    if !bindings_current(tx, &bindings).map_err(|e| e.to_string())? {
        return Err(STALE.into());
    }
    Ok(())
}

fn check_closed_admission(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    use super::events::{
        ADMITTED, COMPACTION_STARTED, CREATED, EPOCH_STARTED, INBOX_UPDATED, PERMISSION_ASKED,
        PROMOTED, QUESTION_ASKED, RESUMED, STEP_STARTED, TOOL_DISPATCHED,
    };
    if event.kind == FENCED
        && event.data.get("closed").is_some()
        && (event.data["closed"] != true
            || !event.data["scope_id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()))
    {
        return Err("Invalid closed subtree admission boundary".into());
    }
    if shell_receipt(tx, event)? {
        return Ok(());
    }
    let targets: Vec<&str> = match event.kind.as_str() {
        CREATED => event.data["info"]["parent_id"]
            .as_str()
            .into_iter()
            .collect(),
        ADMITTED | RESUMED | PROMOTED | EPOCH_STARTED | STEP_STARTED | TOOL_DISPATCHED
        | COMPACTION_STARTED | PERMISSION_ASKED | QUESTION_ASKED => vec![&event.aggregate_id],
        INBOX_UPDATED if event.data["action"] == "released" => vec![&event.aggregate_id],
        "job.started.1" => vec![
            &event.aggregate_id,
            event.data["child_id"]
                .as_str()
                .ok_or("Missing Job admission target")?,
        ],
        super::activity::CHANGED if event.data["status"] == "pending" => vec![
            event.data["session_id"]
                .as_str()
                .ok_or("Missing native activity source")?,
        ],
        "delegation.changed.1" if event.data["data"]["status"] == "pending" => vec![
            event.data["data"]["session_id"]
                .as_str()
                .ok_or("Missing delegation admission source")?,
        ],
        _ => return Ok(()),
    };
    for target in targets {
        let chain = snapshot_chain(tx, target)
            .map_err(|e| e.to_string())?
            .ok_or("Admission ancestry has a cycle or a missing ancestor")?;
        if !chain_is_open(tx, &chain).map_err(|e| e.to_string())? {
            return Err(CLOSED.into());
        }
    }
    Ok(())
}

fn shell_receipt(tx: &Transaction<'_>, event: &StoredEvent) -> Result<bool, String> {
    use super::events::{ADMITTED, PROMOTED};
    if event.data["shell_receipt"] != true {
        return Ok(false);
    }
    match event.kind.as_str() {
        ADMITTED
            if event.data["source"] == "shell"
                && event.data.get("wake").is_none_or(|wake| wake == false) =>
        {
            Ok(true)
        }
        PROMOTED => tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type=?2
             AND json_extract(data,'$.message_id')=?3 AND json_extract(data,'$.shell_receipt')=1
             AND json_extract(data,'$.source')='shell')",
                params![
                    event.aggregate_id,
                    ADMITTED,
                    event.data["message_id"].as_str()
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())
            .and_then(|valid| {
                if valid {
                    Ok(true)
                } else {
                    Err("Missing paired shell settlement receipt".into())
                }
            }),
        _ => Err("Invalid shell settlement receipt".into()),
    }
}
