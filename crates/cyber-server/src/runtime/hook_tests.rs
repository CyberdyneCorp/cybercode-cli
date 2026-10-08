//! Synthetic execution receipts, independent of Session admission and projections.
//! Recording is not authority to execute: the caller still owns trust and native effects.
use std::sync::Arc;
use std::time::Instant;

use cyber_core::hooks::{HookDecision, HookDefinition, HookEvent, HookOutcome};
use cyber_store::{Expected, NewEvent, Store, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};

use super::hooks::{EXECUTED, STARTED, admission_record, elapsed, verify_settlement};
use super::{HookExecutionRecord, HookExecutionResult, HookExecutionStatus, RuntimeError};

pub struct SyntheticHookExecution {
    store: Arc<Store>,
    record: HookExecutionRecord,
    started: Instant,
    settled: bool,
}

pub fn start_synthetic_hook_execution(
    store: Arc<Store>,
    event: &HookEvent,
    definition: &HookDefinition,
    log_io: bool,
) -> Result<SyntheticHookExecution, RuntimeError> {
    if event.as_json()["synthetic"] != true
        || definition.event != event.event()
        || definition
            .handler
            .digest()
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?
            != definition.digest
    {
        return Err(RuntimeError::Invalid(
            "Synthetic hook event or effective digest is invalid".into(),
        ));
    }
    let mut record = admission_record(event, definition, log_io);
    record.synthetic = true;
    let mut data = serde_json::to_value(&record).expect("receipt serializes");
    data["admission_bindings"] = serde_json::json!([]);
    store.append(
        &record.id,
        Expected::Seq(-1),
        vec![NewEvent::new(STARTED, data)],
    )?;
    Ok(SyntheticHookExecution {
        store,
        record,
        started: Instant::now(),
        settled: false,
    })
}

impl SyntheticHookExecution {
    pub fn record(&self) -> &HookExecutionRecord {
        &self.record
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
        self.commit()?;
        self.settled = true;
        Ok(self.record.clone())
    }
    fn commit(&self) -> Result<(), RuntimeError> {
        self.store.append(
            &self.record.id,
            Expected::Seq(0),
            vec![NewEvent::new(
                EXECUTED,
                serde_json::to_value(&self.record).expect("receipt serializes"),
            )],
        )?;
        Ok(())
    }
}
impl Drop for SyntheticHookExecution {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.record.status = HookExecutionStatus::Unknown;
        self.record.duration_ms = Some(elapsed(self.started));
        self.record.outcome = Some(HookOutcome::Error);
        self.record.decision = Some(HookDecision::default());
        self.record.acknowledged = Some(false);
        self.record.must_stop = true;
        self.record.io = None;
        let _ = self.commit();
    }
}

pub(super) fn project(
    tx: &Transaction<'_>,
    event: &StoredEvent,
    record: &HookExecutionRecord,
) -> Result<(), String> {
    let data = serde_json::to_string(record).map_err(|error| error.to_string())?;
    if event.kind == STARTED {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1)",
                [&record.session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if exists || event.data["admission_bindings"] != serde_json::json!([]) {
            return Err("Synthetic hook identity cannot bind or borrow a Session".into());
        }
        tx.execute("INSERT INTO hook_test_execution(id,session_id,status,started_ms,data) VALUES (?1,?2,'running',?3,?4)",params![record.id,record.session_id,record.started_ms,data]).map_err(|error|error.to_string())?;
        return Ok(());
    }
    let previous: Option<String> = tx
        .query_row(
            "SELECT data FROM hook_test_execution WHERE id=?1 AND status='running'",
            [&record.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let previous =
        serde_json::from_str(&previous.ok_or("Synthetic execution is absent or already settled")?)
            .map_err(|error| error.to_string())?;
    verify_settlement(record, &previous)?;
    let status = if record.status == HookExecutionStatus::Unknown {
        "unknown"
    } else {
        "completed"
    };
    tx.execute(
        "UPDATE hook_test_execution SET status=?2,data=?3 WHERE id=?1",
        params![record.id, status, data],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}
