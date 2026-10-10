//! Recover admission receipts from exact historical Job authority; never redispatch.
use super::{Delegation, DelegationPhase, DelegationStatus, Record, Runtime, RuntimeError};
use crate::runtime::{Job, admission_authority};
use cyber_store::StoreError;

impl Runtime {
    pub async fn reconcile_delegation(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<Delegation, RuntimeError> {
        let _open = self.inner.open().await?;
        self.state(parent).await?;
        loop {
            let (mut record, seq) = self
                .delegation_record(parent, id)?
                .ok_or_else(|| RuntimeError::Invalid("Unknown delegation request".into()))?;
            let current = self.delegation(parent, id)?.expect("existing record");
            if current.status != DelegationStatus::Unknown
                || record.data.phase != DelegationPhase::Launching
                || record.data.job_id.is_some()
            {
                return Ok(current);
            }
            let Some(job) = self.bound_admission_job(parent, id, &record)? else {
                return Ok(current);
            };
            if self.state(&job.child_id).await?.info.parent_id.as_deref() != Some(parent) {
                return Err(RuntimeError::Corrupt(
                    "Admission child ancestry changed".into(),
                ));
            }
            record.data.job_id = Some(job.id);
            record.data.status = DelegationStatus::Admitted;
            record.data.error = None;
            match self.write_delegation(&record, seq) {
                Err(RuntimeError::Store(StoreError::Concurrency { .. })) => continue,
                Err(error) => return Err(error),
                Ok(()) => return Ok(record.data),
            }
        }
    }

    fn bound_admission_job(
        &self,
        parent: &str,
        id: &str,
        record: &Record,
    ) -> Result<Option<Job>, RuntimeError> {
        let Some(captured) = record.admission_bindings.as_deref() else {
            return Ok(None);
        };
        let source = parent.to_owned();
        let operation = id.to_owned();
        let rows = self.inner.store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT data FROM event WHERE aggregate_id=?1 AND type='job.started.1'
                 AND EXISTS(SELECT 1 FROM json_each(data,'$.admission_bindings') binding
                   WHERE json_extract(binding.value,'$.id')=?1
                   AND EXISTS(SELECT 1 FROM json_each(binding.value,'$.operations') op WHERE op.value=?2))
                 LIMIT 2",
            )?;
            Ok(statement.query_map(rusqlite::params![source,operation], |row| row.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        if rows.len() != 1 {
            return Ok(None);
        }
        let data: serde_json::Value = serde_json::from_str(&rows[0])
            .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
        if !admission_authority::operation_origin_matches(&data, captured, parent, id)
            .map_err(RuntimeError::Corrupt)?
        {
            return Ok(None);
        }
        let started: Job = serde_json::from_value(data)
            .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
        let job = self.job(&started.id)?;
        if job.session_id != parent || job.child_id != started.child_id || job.kind != "subagent" {
            return Err(RuntimeError::Corrupt(
                "Admission Job identity changed".into(),
            ));
        }
        Ok(Some(job))
    }
}
