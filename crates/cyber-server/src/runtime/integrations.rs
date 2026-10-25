//! Durable owner-selected batches; workflow runs own native checks, review,
//! budget scheduling and recovery. Changing inputs always makes a new attempt.
use cyber_store::{Expected, NewEvent, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Runtime, RuntimeError, WorkflowRunStatus};

pub const EVENT: &str = "workflow.integration.batch.1";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IntegrationBatch {
    pub id: String,
    pub session_id: String,
    pub directory: String,
    pub revision: u64,
    pub attempt: u32,
    pub status: String,
    pub manifest: cyber_workflow::integration::Manifest,
    pub args: Value,
    pub budget: Option<Value>,
    pub run_id: Option<String>,
    pub previous_runs: Vec<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub report: Option<Value>,
    pub publication: Option<Value>,
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let data = &event.data;
    tx.execute("INSERT INTO integration_batch(id,session_id,directory,status,revision,created_ms,data) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET status=excluded.status,revision=excluded.revision,data=excluded.data", params![event.aggregate_id,data["session_id"].as_str(),data["directory"].as_str(),data["status"].as_str(),data["revision"].as_i64(),data["created_ms"].as_i64(),data.to_string()])?;
    Ok(())
}

fn rejected(reason: impl Into<String>) -> StoreError {
    StoreError::InvalidEvent {
        kind: EVENT.into(),
        reason: reason.into(),
    }
}
fn parse(text: String) -> Result<IntegrationBatch, StoreError> {
    serde_json::from_str(&text).map_err(|error| StoreError::Unavailable(error.to_string()))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn read(tx: &rusqlite::Connection, id: &str) -> Result<IntegrationBatch, StoreError> {
    let text: Option<String> = tx
        .query_row(
            "SELECT data FROM integration_batch WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    parse(text.ok_or_else(|| rejected("integration batch not found"))?)
}

impl Runtime {
    pub async fn integration_submit(
        &self,
        session: &str,
        mut args: Value,
        budget: Option<Value>,
        request_key: Option<String>,
    ) -> Result<IntegrationBatch, RuntimeError> {
        let owner = self.state(session).await?.info;
        let manifest:cyber_workflow::integration::Manifest=serde_json::from_value(json!({"target_branch":args["target_branch"],"contributions":args["contributions"],"checks":args["checks"],"reviewer_model":args["reviewer_model"]})).map_err(|error|RuntimeError::Invalid(error.to_string()))?;
        manifest.validate().map_err(RuntimeError::Invalid)?;
        let id = request_key.map_or_else(
            || cyber_core::ids::new_id("intg"),
            |key| {
                use sha2::{Digest, Sha256};
                format!(
                    "intg_{:x}",
                    Sha256::digest(format!("integration-submit:cyber:{key}"))
                )
            },
        );
        args["integration_admission"]["batch_id"] = id.clone().into();
        let batch = IntegrationBatch {
            id: id.clone(),
            session_id: owner.id,
            directory: owner.directory,
            revision: 1,
            attempt: 0,
            status: "queued".into(),
            manifest,
            args,
            budget,
            run_id: None,
            previous_runs: vec![],
            created_ms: now(),
            updated_ms: now(),
            report: None,
            publication: None,
        };
        let record = serde_json::to_value(&batch)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let (_, batch) = self.inner.store.append_checked(&id,Expected::Any,move|tx| {
            let old: Option<String> = tx.query_row("SELECT data FROM integration_batch WHERE id=?1",[&batch.id],|row|row.get(0)).optional()?;
            if let Some(old) = old {
                let previous = parse(old)?;
                if previous.session_id != batch.session_id || previous.directory != batch.directory || previous.manifest != batch.manifest || previous.budget != batch.budget {
                    return Err(rejected("Integration submission key was reused for different input"));
                }
                return Ok((vec![],previous));
            }
            let pending:i64=tx.query_row("SELECT COUNT(*) FROM integration_batch b LEFT JOIN workflow_run r ON json_extract(b.data,'$.run_id')=r.id WHERE b.status IN ('queued','preparing') OR r.status='queued'",[],|row|row.get(0))?;
            if pending>=100 { return Err(rejected("Integration queue limit reached (100)")); }
            Ok((vec![NewEvent::new(EVENT,record)],batch))
        }).map_err(|error| match error {
            StoreError::InvalidEvent {kind,reason} if kind == EVENT => RuntimeError::Conflict(reason),
            error => RuntimeError::from(error),
        })?;
        Ok(batch)
    }

    pub fn integration_batch(&self, id: &str) -> Result<IntegrationBatch, RuntimeError> {
        let id = id.to_string();
        let mut batch = self.inner.store.read(move |db| read(db, &id))?;
        self.integration_effective(&mut batch)?;
        Ok(batch)
    }

    pub fn integration_batches(
        &self,
        directory: &str,
    ) -> Result<Vec<IntegrationBatch>, RuntimeError> {
        let directory = directory.to_string();
        let mut rows=self.inner.store.read(move|db| {
            let mut statement=db.prepare("SELECT data FROM integration_batch WHERE directory=?1 ORDER BY created_ms DESC LIMIT 100")?;
            statement.query_map([directory],|row|row.get::<_,String>(0))?.map(|row|parse(row?)).collect::<Result<Vec<_>,StoreError>>()
        })?;
        for batch in &mut rows {
            self.integration_effective(batch)?;
        }
        Ok(rows)
    }

    fn integration_effective(&self, batch: &mut IntegrationBatch) -> Result<(), RuntimeError> {
        let run_id = match &batch.run_id {
            Some(id) => Some(id.clone()),
            None if batch.status == "preparing" => self.integration_orphan_run(&batch.id)?,
            _ => None,
        };
        let Some(run_id) = run_id else {
            return Ok(());
        };
        if let Some(receipt) = self.integration_publication_receipts(&run_id)? {
            batch.publication = Some(receipt);
        }
        if batch.status == "cancelled" {
            return Ok(());
        }
        let Some(run) = self.workflow_run(&run_id)? else {
            batch.status = "unknown".into();
            return Ok(());
        };
        batch.run_id = Some(run_id);
        batch.status = match run.status {
            WorkflowRunStatus::Queued => "queued",
            WorkflowRunStatus::Running => match run.phase.as_deref() {
                Some("Test") => "checking",
                Some("Review") => "reviewing",
                _ => "preparing",
            },
            WorkflowRunStatus::Completed => run
                .result
                .as_ref()
                .and_then(|report| report["status"].as_str())
                .unwrap_or("unknown"),
            WorkflowRunStatus::Stopped => "cancelled",
            WorkflowRunStatus::Failed | WorkflowRunStatus::BudgetExceeded => "failed",
            WorkflowRunStatus::Paused | WorkflowRunStatus::WaitingInput => "unknown",
        }
        .into();
        batch.report = run.result;
        Ok(())
    }

    fn integration_publication_receipts(&self, run: &str) -> Result<Option<Value>, RuntimeError> {
        let aggregate = format!("integration_publication_{run}");
        let page = self.inner.store.read_events(&aggregate, -1, 16)?;
        let records = page
            .events
            .iter()
            .map(
                |event| json!({"seq":event.seq,"data":publication_preview(&event.data,&aggregate)}),
            )
            .collect::<Vec<_>>();
        let Some(first) = records.first() else {
            return Ok(None);
        };
        let last = &records.last().expect("nonempty receipts")["data"];
        let mut receipt = first["data"].clone();
        receipt["phase"] = if last["phase"] == "published" && !page.has_more {
            "published"
        } else {
            "unknown"
        }
        .into();
        receipt["settled"] = (receipt["phase"] == "published").into();
        receipt["last_phase"] = last["phase"].clone();
        if let Some(pr) = last.get("pull_request") {
            receipt["pull_request"] = pr.clone();
        }
        receipt["aggregate_id"] = aggregate.into();
        receipt["records_truncated"] = page.has_more.into();
        receipt["records"] = records.into();
        Ok(Some(receipt))
    }

    fn integration_orphan_run(&self, id: &str) -> Result<Option<String>, RuntimeError> {
        let id = id.to_string();
        self.inner.store.read(move|db|Ok(db.query_row("SELECT r.id FROM workflow_run r JOIN integration_batch b ON b.id=?1 WHERE r.name='integrate' AND r.session_id=b.session_id AND json_extract(r.data,'$.directory')=b.directory AND json_extract(r.data,'$.args.integration_admission.batch_id')=b.id AND json_extract(r.data,'$.args.integration_admission.attempt')=json_extract(b.data,'$.attempt') AND json_extract(r.data,'$.origin.kind')='bundled' ORDER BY r.started_ms DESC LIMIT 1",[id],|row|row.get(0)).optional()?)).map_err(Into::into)
    }

    pub fn integration_claim(
        &self,
        id: &str,
        revision: u64,
        mut args: Value,
    ) -> Result<IntegrationBatch, RuntimeError> {
        let id = id.to_string();
        let aggregate = id.clone();
        let (_, batch) = self
            .inner
            .store
            .append_checked(&aggregate, Expected::Any, move |tx| {
                let mut batch = read(tx, &id)?;
                let active = if let Some(run)=batch.run_id.as_deref() { tx.query_row("SELECT status IN ('running','queued','waiting_input','paused') FROM workflow_run WHERE id=?1",[run],|row|row.get::<_,bool>(0)).optional()?.unwrap_or(true) } else { batch.status=="preparing" };
                if batch.revision != revision || active {
                    return Err(rejected("Integration revision changed or attempt already claimed"));
                }
                if let Some(run) = batch.run_id.take() {
                    batch.previous_runs.push(run);
                }
                batch.attempt += 1;
                batch.revision += 1;
                batch.status = "preparing".into();
                batch.report = None;
                batch.publication = None;
                batch.updated_ms = now();
                args["integration_admission"]["batch_id"] = batch.id.clone().into();
                args["integration_admission"]["attempt"] = batch.attempt.into();
                batch.args = args;
                let value = serde_json::to_value(&batch)
                    .map_err(|error| StoreError::Unavailable(error.to_string()))?;
                Ok((vec![NewEvent::new(EVENT, value)], batch))
            })?;
        Ok(batch)
    }

    pub fn integration_start_failed(
        &self,
        id: &str,
        reason: String,
    ) -> Result<IntegrationBatch, RuntimeError> {
        if let Some(run) = self.integration_orphan_run(id)? {
            return self.integration_bind(id, &run);
        }
        self.integration_update(id, move |batch| {
            if batch.status != "cancelled" {
                batch.status = "unknown".into();
                batch.report =
                    Some(json!({"status":"unknown","eligible":false,"limitation":reason}));
            }
            Ok(())
        })
    }
    pub fn integration_bind(&self, id: &str, run: &str) -> Result<IntegrationBatch, RuntimeError> {
        let run = self
            .workflow_run(run)?
            .ok_or_else(|| RuntimeError::Invalid("integration run missing".into()))?;
        self.integration_update(id, move |batch| {
            let admission = &run.args["integration_admission"];
            if run.session_id != batch.session_id
                || run.directory != batch.directory
                || admission["batch_id"].as_str() != Some(batch.id.as_str())
                || admission["attempt"].as_u64() != Some(u64::from(batch.attempt))
                || run.origin != json!({"kind":"bundled","name":"integrate"})
            {
                return Err(
                    "Integration run does not match the current owner/attempt claim".into(),
                );
            }
            if batch.status != "cancelled" {
                batch.status = "running".into();
            }
            batch.run_id = Some(run.id);
            Ok(())
        })
    }

    pub async fn integration_cancel(
        &self,
        id: &str,
        revision: u64,
    ) -> Result<IntegrationBatch, RuntimeError> {
        let batch = self.integration_update(id, move |batch| {
            if batch.revision != revision {
                return Err("Integration revision changed".into());
            }
            batch.status = "cancelled".into();
            Ok(())
        })?;
        if let Some(run) = batch.run_id.clone().or(self.integration_orphan_run(id)?) {
            let _ = self.stop_workflow_run(&run).await;
        }
        Ok(batch)
    }

    pub fn integration_publication(
        &self,
        id: &str,
        revision: u64,
        run: &str,
        value: Value,
    ) -> Result<IntegrationBatch, RuntimeError> {
        let run = run.to_string();
        self.integration_update(id, move |batch| {
            if batch.revision != revision || batch.run_id.as_deref() != Some(run.as_str()) || batch.status == "cancelled" {
                return Err("Integration attempt changed before publication settled; inspect the original native receipt".into());
            }
            batch.publication = Some(value);
            Ok(())
        })
    }

    fn integration_update(
        &self,
        id: &str,
        update: impl FnOnce(&mut IntegrationBatch) -> Result<(), String> + Send + 'static,
    ) -> Result<IntegrationBatch, RuntimeError> {
        let id = id.to_string();
        let aggregate = id.clone();
        let (_, batch) = self
            .inner
            .store
            .append_checked(&aggregate, Expected::Any, move |tx| {
                let mut batch = read(tx, &id)?;
                update(&mut batch).map_err(rejected)?;
                batch.revision += 1;
                batch.updated_ms = now();
                let value = serde_json::to_value(&batch)
                    .map_err(|e| StoreError::Unavailable(e.to_string()))?;
                Ok((vec![NewEvent::new(EVENT, value)], batch))
            })?;
        Ok(batch)
    }
}

// Native records are retained in full; owner queue responses bound their previews.
fn publication_preview(record: &Value, aggregate: &str) -> Value {
    if record.to_string().len() <= 16_384 {
        return record.clone();
    }
    let mut preview = json!({"preview_truncated":true,"full_evidence_aggregate":aggregate});
    for key in [
        "phase",
        "operation_id",
        "definition_sha256",
        "repository",
        "remote",
        "destination",
        "target",
        "branch",
        "commit",
    ] {
        if let Some(text) = record[key].as_str() {
            let mut end = text.len().min(512);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            preview[key] = text[..end].into();
        }
    }
    if let Some(outcome) = record.get("outcome") {
        preview["outcome"] = json!({"outcome":outcome["outcome"],"settled":outcome["settled"],"exit_code":outcome["exit_code"],"log_truncated":true});
    }
    preview
}
