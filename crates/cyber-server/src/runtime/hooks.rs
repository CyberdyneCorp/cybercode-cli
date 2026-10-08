//! Durable hook execution ownership and privacy-preserving receipts.
//! Recording does not authorize a handler or apply its decision; dispatch does both.

use std::sync::Arc;
use std::time::Instant;

use cyber_core::hooks::{
    HookAction, HookDecision, HookDefinition, HookEvent, HookOutcome, HookScope,
};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{AdmissionAuthority, Runtime, RuntimeError};

pub(super) const STARTED: &str = "hook.started.1";
const EXECUTED: &str = "hook.executed.1";
const IO_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookExecutionStatus {
    Running,
    Completed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookExecutionIo {
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookExecutionRecord {
    pub id: String,
    pub session_id: String,
    pub hook_id: String,
    pub digest: String,
    pub event: String,
    pub scope: HookScope,
    pub directory: String,
    pub agent: String,
    pub mode: String,
    pub started_ms: i64,
    pub log_io: bool,
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

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl Runtime {
    /// The caller supplies captured identity and an inspected definition, then still
    /// verifies authority, current config/trust and sandbox policy before launching.
    pub async fn start_hook_execution(
        &self,
        event: &HookEvent,
        definition: &HookDefinition,
        log_io: bool,
    ) -> Result<HookExecution, RuntimeError> {
        let _open = self.inner.open().await?;
        if definition.event != event.event()
            || definition
                .handler
                .digest()
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?
                != definition.digest
        {
            return Err(RuntimeError::Invalid(
                "Hook event or effective digest changed".into(),
            ));
        }
        let handle = self.inner.handle(&event.identity().session_id).await?;
        let activity = super::activity::Scope::reserve(&self.inner, &handle).await?;
        let authority = activity.authority();
        authority.verify(self, &event.identity().session_id)?;
        let identity = event.identity();
        let record = HookExecutionRecord {
            id: cyber_core::ids::new_id("hke"),
            session_id: identity.session_id.clone(),
            hook_id: definition
                .handler
                .id
                .clone()
                .unwrap_or_else(|| definition.digest.clone()),
            digest: definition.digest.clone(),
            event: event.event().into(),
            scope: definition.scope,
            directory: identity.location.directory.display().to_string(),
            agent: identity.agent.clone(),
            mode: identity.mode.clone(),
            started_ms: chrono::Utc::now().timestamp_millis(),
            log_io,
            status: HookExecutionStatus::Running,
            duration_ms: None,
            outcome: None,
            decision: None,
            acknowledged: None,
            must_stop: false,
            io: None,
        };
        let mut data = serde_json::to_value(&record).expect("receipt serializes");
        data["admission_bindings"] =
            serde_json::to_value(&authority.bindings).expect("authority serializes");
        self.inner.store.append(
            &record.id,
            Expected::Seq(-1),
            vec![NewEvent::new(STARTED, data)],
        )?;
        Ok(HookExecution {
            store: Arc::clone(&self.inner.store),
            authority,
            activity,
            record,
            started: Instant::now(),
            settled: false,
        })
    }

    pub fn hook_executions(
        &self,
        session: &str,
        limit: u32,
    ) -> Result<Vec<HookExecutionRecord>, RuntimeError> {
        if limit == 0 || limit > cyber_store::MAX_PAGE_LIMIT {
            return Err(RuntimeError::Invalid(
                "Hook receipt limit must be between 1 and 500".into(),
            ));
        }
        let session = session.to_owned();
        Ok(self.inner.store.read(move |conn| {
            let mut statement = conn.prepare("SELECT data FROM hook_execution WHERE session_id=?1 ORDER BY started_ms DESC,id DESC LIMIT ?2")?;
            let rows = statement.query_map(params![session,limit],|row| row.get::<_,String>(0))?;
            rows.map(|row| {
                serde_json::from_str(&row?).map_err(|error| StoreError::CorruptEvent { id: "hook receipt projection".into(), reason: error.to_string() })
            }).collect()
        })?)
    }
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
