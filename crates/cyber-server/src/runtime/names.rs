//! Indexed, parent-scoped child identities; historical background names remain reserved.
use super::{Runtime, RuntimeError};

impl Runtime {
    pub fn subagent_names(&self, parent: &str) -> Result<Vec<String>, RuntimeError> {
        let parent = parent.to_owned();
        self.inner
            .store
            .read(move |db| {
                let mut query = db.prepare(
                "SELECT subagent_name FROM session WHERE parent_id=?1 AND subagent_name IS NOT NULL
                 UNION SELECT name FROM job WHERE session_id=?1 ORDER BY 1",
            )?;
                Ok(query
                    .query_map([parent], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .map_err(Into::into)
    }
}

/// Exclusive tool execution ownership, retained after Drain idleness through result settlement.
pub struct ChildExecution {
    runtime: super::WeakRuntime,
    parent: String,
    id: String,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Resumed {
    pub name: String,
    pub output_schema: Option<serde_json::Value>,
}

impl Runtime {
    pub fn claim_child_execution(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<ChildExecution, RuntimeError> {
        let mut owners = self
            .inner
            .child_executions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|_, owner| owner.strong_count() > 0);
        let mutex = owners
            .get(id)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                owners.insert(id.into(), std::sync::Arc::downgrade(&mutex));
                mutex
            });
        let guard = mutex
            .try_lock_owned()
            .map_err(|_| RuntimeError::Invalid("Subagent busy".into()))?;
        Ok(ChildExecution {
            runtime: self.downgrade(),
            parent: parent.into(),
            id: id.into(),
            _guard: guard,
        })
    }

    pub async fn resolve_subagent(
        &self,
        parent: &str,
        reference: &str,
    ) -> Result<super::SessionInfo, RuntimeError> {
        use rusqlite::OptionalExtension;
        let parent = parent.to_owned();
        let reference = reference.to_owned();
        let id: Option<String> = self.inner.store.read(move |db| {
            let exact: Option<String> = db
                .query_row(
                    "SELECT id FROM session WHERE parent_id=?1 AND id=?2",
                    rusqlite::params![parent, reference],
                    |row| row.get(0),
                )
                .optional()?;
            if exact.is_some() {
                return Ok(exact);
            }
            Ok(db
                .query_row(
                    "SELECT id FROM session WHERE parent_id=?1 AND subagent_name=?2
                 UNION SELECT s.id FROM job j JOIN session s ON s.id=j.child_id
                 WHERE j.session_id=?1 AND j.name=?2 AND s.parent_id=?1 LIMIT 1",
                    rusqlite::params![parent, reference],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        let id = id.ok_or_else(|| RuntimeError::Invalid("Subagent not found".into()))?;
        Ok(self.state(&id).await?.info)
    }

    pub async fn resume_child(
        &self,
        owner: &ChildExecution,
        admission: super::Admission,
        name: String,
        schema: Option<super::StructuredSchema>,
    ) -> Result<super::Receipt, RuntimeError> {
        if !std::sync::Weak::ptr_eq(&owner.runtime.inner, &self.downgrade().inner) {
            return Err(RuntimeError::Invalid(
                "Child owner belongs to another runtime".into(),
            ));
        }
        let state = self.state(&owner.id).await?;
        if state.info.parent_id.as_deref() != Some(&owner.parent) {
            return Err(RuntimeError::Invalid("Subagent not found".into()));
        }
        if self.is_running(&owner.id)
            || self
                .jobs(Some(&owner.parent))?
                .iter()
                .any(|job| job.child_id == owner.id && job.status == super::JobStatus::Running)
        {
            return Err(RuntimeError::Invalid("Subagent busy".into()));
        }
        if name.trim().is_empty()
            || name.len() > 128
            || state
                .info
                .subagent_name
                .as_ref()
                .is_some_and(|old| old != &name)
        {
            return Err(RuntimeError::Invalid(
                "Resume cannot change the child name".into(),
            ));
        }
        self.admit_attempt(
            &owner.id,
            admission,
            Some(Resumed {
                name,
                output_schema: schema.map(|schema| schema.schema().clone()),
            }),
        )
        .await
    }
}
