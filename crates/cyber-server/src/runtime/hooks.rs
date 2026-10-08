//! Durable hook execution ownership and privacy-preserving receipts.
//! Recording does not authorize a handler or apply its decision; dispatch does both.

use std::sync::Arc;
use std::time::Instant;

use cyber_core::hooks::{
    HookAction, HookDecision, HookDefinition, HookEvent, HookOutcome, HookScope,
};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AdmissionAuthority, Runtime, RuntimeError};

pub(super) const STARTED: &str = "hook.started.1";
pub(super) const EXECUTED: &str = "hook.executed.1";
const ONCE_ADMITTED: &str = "Hook once handler already admitted";
const IO_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookExecutionStatus {
    Running,
    Completed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookExecutionIo {
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookExecutionRecord {
    pub id: String,
    /// Synthetic identity is never a Session binding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    pub session_id: String,
    pub hook_id: String,
    pub digest: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub scope: HookScope,
    pub directory: String,
    pub agent: String,
    pub mode: String,
    pub started_ms: i64,
    pub log_io: bool,
    #[serde(default)]
    pub once: bool,
    pub status: HookExecutionStatus,
    pub duration_ms: Option<u64>,
    pub outcome: Option<HookOutcome>,
    pub decision: Option<HookDecision>,
    pub acknowledged: Option<bool>,
    pub must_stop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<HookExecutionIo>,
}

pub struct HookExecutionResult {
    pub outcome: HookOutcome,
    pub decision: HookDecision,
    pub acknowledged: bool,
    pub must_stop: bool,
    pub io: Option<HookExecutionIo>,
}

/// Exclusive, non-cloneable execution owner. Its committed start survives process death.
/// Call verify before effects; finish records observations even after admission closes.
pub struct HookExecution {
    store: Arc<Store>,
    authority: AdmissionAuthority,
    activity: super::activity::Scope,
    record: HookExecutionRecord,
    started: Instant,
    settled: bool,
}

impl HookExecution {
    pub fn record(&self) -> &HookExecutionRecord {
        &self.record
    }

    pub fn verify(&self, runtime: &Runtime) -> Result<(), RuntimeError> {
        self.authority.verify(runtime, &self.record.session_id)
    }

    pub fn cancellation(&self) -> tokio_util::sync::CancellationToken {
        self.activity.control.stop.clone()
    }

    /// Durable native launch marker; disposal after this point retains Unknown activity.
    pub fn mark_launch(&mut self, runtime: &Runtime) -> Result<(), RuntimeError> {
        self.verify(runtime)?;
        self.activity.launch()
    }

    pub fn finish(
        mut self,
        result: HookExecutionResult,
    ) -> Result<HookExecutionRecord, RuntimeError> {
        self.record.status = if result.acknowledged {
            HookExecutionStatus::Completed
        } else {
            HookExecutionStatus::Unknown
        };
        self.record.duration_ms = Some(elapsed(self.started));
        self.record.outcome = Some(result.outcome);
        self.record.decision = Some(result.decision);
        self.record.acknowledged = Some(result.acknowledged);
        self.record.must_stop = result.must_stop || !result.acknowledged;
        self.record.io = if self.record.log_io { result.io } else { None };
        if result.acknowledged {
            self.activity.settle()?;
        }
        self.store.append(
            &self.record.id,
            Expected::Seq(0),
            vec![NewEvent::new(
                EXECUTED,
                serde_json::to_value(&self.record).expect("receipt serializes"),
            )],
        )?;
        self.settled = true;
        Ok(self.record.clone())
    }
}

impl Drop for HookExecution {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        // No raw buffers or inferred successful termination on disposal.
        self.record.status = HookExecutionStatus::Unknown;
        self.record.duration_ms = Some(elapsed(self.started));
        self.record.outcome = Some(HookOutcome::Error);
        self.record.decision = Some(HookDecision::default());
        self.record.acknowledged = Some(false);
        self.record.must_stop = true;
        self.record.io = None;
        let _ = self.store.append(
            &self.record.id,
            Expected::Seq(0),
            vec![NewEvent::new(
                EXECUTED,
                serde_json::to_value(&self.record).expect("receipt serializes"),
            )],
        );
    }
}

pub(super) fn admission_record(
    event: &HookEvent,
    definition: &HookDefinition,
    log_io: bool,
) -> HookExecutionRecord {
    let identity = event.identity();
    HookExecutionRecord {
        id: cyber_core::ids::new_id("hke"),
        synthetic: false,
        session_id: identity.session_id.clone(),
        hook_id: definition
            .handler
            .id
            .clone()
            .unwrap_or_else(|| definition.digest.clone()),
        digest: definition.digest.clone(),
        event: event.event().into(),
        call_id: event
            .as_json()
            .get("call_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        tool_name: event
            .as_json()
            .get("tool_name")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        scope: definition.scope,
        directory: identity.location.directory.display().to_string(),
        agent: identity.agent.clone(),
        mode: identity.mode.clone(),
        started_ms: chrono::Utc::now().timestamp_millis(),
        log_io,
        once: definition.handler.once,
        status: HookExecutionStatus::Running,
        duration_ms: None,
        outcome: None,
        decision: None,
        acknowledged: None,
        must_stop: false,
        io: None,
    }
}

pub(super) fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl Runtime {
    pub fn hook_notice(&self, session: &str, hook_id: &str, message: &str) {
        self.inner.bus.publish(super::LiveEvent::HookNotice {
            session_id: session.into(),
            hook_id: hook_id.into(),
            message: message.into(),
        });
    }

    /// The caller supplies captured identity and an inspected definition, then still
    /// verifies authority, current config/trust and sandbox policy before launching.
    pub async fn start_hook_execution(
        &self,
        event: &HookEvent,
        definition: &HookDefinition,
        log_io: bool,
    ) -> Result<HookExecution, RuntimeError> {
        self.try_start_hook_execution(event, definition, log_io)
            .await?
            .ok_or_else(|| RuntimeError::Invalid(ONCE_ADMITTED.into()))
    }

    /// Atomically claim a once handler at durable admission. An existing claim
    /// remains consumed across completion, disposal, crashes and runtime restart.
    pub async fn try_start_hook_execution(
        &self,
        event: &HookEvent,
        definition: &HookDefinition,
        log_io: bool,
    ) -> Result<Option<HookExecution>, RuntimeError> {
        let _open = self.inner.open().await?;
        if event.is_synthetic()
            || definition.event != event.event()
            || definition
                .handler
                .digest()
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?
                != definition.digest
        {
            return Err(RuntimeError::Invalid(
                "Hook event is synthetic or effective event/digest changed".into(),
            ));
        }
        let handle = self.inner.handle(&event.identity().session_id).await?;
        let activity = super::activity::Scope::reserve(&self.inner, &handle).await?;
        let authority = activity.authority();
        authority.verify(self, &event.identity().session_id)?;
        let record = admission_record(event, definition, log_io);
        let mut data = serde_json::to_value(&record).expect("receipt serializes");
        data["admission_bindings"] =
            serde_json::to_value(&authority.bindings).expect("authority serializes");
        match self.inner.store.append(
            &record.id,
            Expected::Seq(-1),
            vec![NewEvent::new(STARTED, data)],
        ) {
            Ok(_) => {}
            Err(StoreError::Projector { kind, reason })
                if kind == STARTED && reason == ONCE_ADMITTED =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Some(HookExecution {
            store: Arc::clone(&self.inner.store),
            authority,
            activity,
            record,
            started: Instant::now(),
            settled: false,
        }))
    }

    pub fn hook_executions(
        &self,
        session: &str,
        limit: u32,
    ) -> Result<Vec<HookExecutionRecord>, RuntimeError> {
        self.hook_executions_page(session, limit, None)
            .map(|(rows, _)| rows)
    }

    pub fn hook_executions_page(
        &self,
        session: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<(Vec<HookExecutionRecord>, Option<String>), RuntimeError> {
        hook_execution_page(&self.inner.store, session, limit, cursor)
    }
}

/// Read committed observations without constructing a runtime or reconciling owners.
pub fn hook_execution_page(
    store: &Store,
    session: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<(Vec<HookExecutionRecord>, Option<String>), RuntimeError> {
    if limit == 0 || limit > cyber_store::MAX_PAGE_LIMIT {
        return Err(RuntimeError::Invalid(
            "Hook receipt limit must be between 1 and 500".into(),
        ));
    }
    let (at, id) = hook_cursor(session, cursor)?;
    let session_id = session;
    let session = session.to_owned();
    let rows: Option<Vec<HookExecutionRecord>> = store.read(move |conn| {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM session WHERE id=?1)", [&session], |row| row.get(0))?;
            if !exists { return Ok(None); }
            let mut statement = conn.prepare("SELECT data FROM hook_execution WHERE session_id=?1 AND (started_ms<?2 OR (started_ms=?2 AND id<?3)) ORDER BY started_ms DESC,id DESC LIMIT ?4")?;
            let rows = statement.query_map(params![session,at,id,limit+1],|row| row.get::<_,String>(0))?;
            rows.map(|row| serde_json::from_str(&row?).map_err(|error| StoreError::CorruptEvent { id:"hook receipt projection".into(),reason:error.to_string() })).collect::<Result<Vec<_>,_>>().map(Some)
        })?;
    let mut rows = rows.ok_or_else(|| RuntimeError::SessionNotFound(session_id.into()))?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = more
        .then(|| {
            rows.last()
                .map(|row| format!("{}:{}:{}", row.started_ms, row.session_id, row.id))
        })
        .flatten();
    Ok((rows, next))
}

/// Read only summary metadata, matching the immutable execution Location rather
/// than a Session's potentially rebound current directory. No reconciliation.
pub fn hook_last_run(
    store: &Store,
    location: &std::path::Path,
    definition: &HookDefinition,
) -> Result<Option<cyber_core::hooks::HookLastRun>, RuntimeError> {
    let root = std::fs::canonicalize(cyber_core::config::project_root(location))
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    let digest = definition.digest.clone();
    let event = definition.event.clone();
    let scope = serde_json::to_value(definition.scope)
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    store.read(move |conn| {
        let mut statement = conn.prepare("SELECT json_remove(data,'$.io','$.decision') FROM hook_execution WHERE json_extract(data,'$.digest')=?1 AND json_extract(data,'$.event')=?2 AND json_extract(data,'$.scope')=?3 ORDER BY started_ms DESC,id DESC")?;
        let rows = statement.query_map(params![digest,event,scope.as_str()], |row| row.get::<_,String>(0))?;
        for row in rows {
            let observation: cyber_core::hooks::HookLastRun = serde_json::from_str(&row?).map_err(|error| StoreError::CorruptEvent { id:"hook last-run projection".into(),reason:error.to_string() })?;
            let recorded_root = cyber_core::config::project_root(std::path::Path::new(&observation.directory));
            if std::fs::canonicalize(recorded_root).is_ok_and(|path| path == root) { return Ok(Some(observation)); }
        }
        Ok(None)
    }).map_err(Into::into)
}

fn hook_cursor(session: &str, cursor: Option<&str>) -> Result<(i64, String), RuntimeError> {
    let Some(cursor) = cursor else {
        return Ok((i64::MAX, String::new()));
    };
    let invalid =
        || RuntimeError::Invalid("InvalidCursorError: malformed or foreign hook cursor".into());
    let mut fields = cursor.split(':');
    let at: i64 = fields
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let owner = fields.next().ok_or_else(invalid)?;
    let id = fields.next().ok_or_else(invalid)?;
    if at < 0
        || owner != session
        || !cyber_core::ids::has_prefix(id, "hke")
        || fields.next().is_some()
    {
        return Err(invalid());
    }
    Ok((at, id.into()))
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(STARTED, |data| validate(data, true))
        .expect("valid hook start");
    registry
        .register_with(EXECUTED, |data| validate(data, false))
        .expect("valid hook receipt");
}

fn decode(data: &serde_json::Value) -> Result<HookExecutionRecord, String> {
    let mut data = data.clone();
    if let Some(object) = data.as_object_mut() {
        object.remove("admission_bindings");
    }
    serde_json::from_value(data).map_err(|error| error.to_string())
}

fn validate(data: &serde_json::Value, started: bool) -> Result<(), String> {
    let record = decode(data)?;
    validate_identity(&record)?;
    if started {
        validate_start(&record, data)
    } else {
        validate_terminal(&record)
    }
}

fn validate_identity(record: &HookExecutionRecord) -> Result<(), String> {
    let valid_digest = record.digest.strip_prefix("sha256:").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if !cyber_core::ids::has_prefix(&record.id, "hke")
        || record.hook_id.trim().is_empty()
        || !valid_digest
    {
        return Err("Invalid hook receipt identity".into());
    }
    HookEvent::new(
        &record.event,
        cyber_core::hooks::HookIdentity {
            session_id: record.session_id.clone(),
            location: cyber_core::hooks::HookLocation {
                directory: record.directory.clone().into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: record.agent.clone(),
            mode: record.mode.clone(),
        },
        record.started_ms,
        serde_json::Map::new(),
    )
    .map(|_| ())
}

fn validate_start(record: &HookExecutionRecord, data: &serde_json::Value) -> Result<(), String> {
    if record.status != HookExecutionStatus::Running
        || record.duration_ms.is_some()
        || record.outcome.is_some()
        || record.decision.is_some()
        || record.acknowledged.is_some()
        || record.must_stop
        || record.io.is_some()
        || data.get("admission_bindings").is_none()
    {
        return Err("Invalid hook admission receipt".into());
    }
    Ok(())
}

fn validate_terminal(record: &HookExecutionRecord) -> Result<(), String> {
    let restrictive = record.decision.as_ref().is_some_and(|decision| {
        matches!(
            decision.decision,
            None | Some(HookAction::Deny | HookAction::Block)
        )
    });
    let unknown_valid = record.acknowledged != Some(false)
        || (record.must_stop
            && restrictive
            && matches!(
                record.outcome,
                Some(HookOutcome::Error | HookOutcome::Timeout)
            ));
    if record.status == HookExecutionStatus::Running
        || record.duration_ms.is_none()
        || record.outcome.is_none()
        || record.decision.is_none()
        || record.acknowledged.is_none()
        || (record.status == HookExecutionStatus::Completed) != (record.acknowledged == Some(true))
        || !unknown_valid
    {
        return Err("Invalid hook terminal receipt".into());
    }
    if let Some(io) = &record.io
        && (!record.log_io
            || [io.stdin.len(), io.stdout.len(), io.stderr.len()]
                .into_iter()
                .any(|length| length > IO_LIMIT))
    {
        return Err("Hook IO logging disabled or capture exceeds limit".into());
    }
    Ok(())
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if ![STARTED, EXECUTED].contains(&event.kind.as_str()) {
        return Ok(());
    }
    let record = decode(&event.data)?;
    if event.aggregate_id != record.id {
        return Err("Hook execution aggregate differs from receipt".into());
    }
    if record.synthetic {
        return super::hook_tests::project(tx, event, &record);
    }
    let data = serde_json::to_string(&record).map_err(|error| error.to_string())?;
    if event.kind == STARTED {
        let directory: String = tx
            .query_row(
                "SELECT directory FROM session WHERE id=?1",
                [&record.session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if directory != record.directory {
            return Err("Hook Location differs from Session".into());
        }
        if record.once {
            let admitted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM hook_execution WHERE session_id=?1 AND json_extract(data,'$.digest')=?2 AND json_extract(data,'$.once')=1)",
                params![record.session_id,record.digest], |row| row.get(0),
            ).map_err(|error| error.to_string())?;
            if admitted {
                return Err(ONCE_ADMITTED.into());
            }
        }
        tx.execute("INSERT INTO hook_execution(id,session_id,status,started_ms,data) VALUES (?1,?2,'running',?3,?4)",params![record.id,record.session_id,record.started_ms,data]).map_err(|error|error.to_string())?;
    } else {
        let previous: Option<String> = tx
            .query_row(
                "SELECT data FROM hook_execution WHERE id=?1 AND status='running'",
                [&record.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let previous: HookExecutionRecord =
            serde_json::from_str(&previous.ok_or("Hook execution is absent or already settled")?)
                .map_err(|error| error.to_string())?;
        verify_settlement(&record, &previous)?;
        let status = if record.status == HookExecutionStatus::Unknown {
            "unknown"
        } else {
            "completed"
        };
        tx.execute(
            "UPDATE hook_execution SET status=?2,data=?3 WHERE id=?1",
            params![record.id, status, data],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn verify_settlement(
    record: &HookExecutionRecord,
    previous: &HookExecutionRecord,
) -> Result<(), String> {
    let mut expected = record.clone();
    expected.status = previous.status;
    expected.duration_ms = None;
    expected.outcome = None;
    expected.decision = None;
    expected.acknowledged = None;
    expected.must_stop = false;
    expected.io = None;
    if serde_json::to_value(expected).map_err(|error| error.to_string())?
        != serde_json::to_value(previous).map_err(|error| error.to_string())?
    {
        return Err("Hook receipt changed its admitted identity or IO policy".into());
    }
    Ok(())
}
