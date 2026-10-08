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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_bindings: Option<Vec<super::admission_authority::Binding>>,
    pub name: Option<String>,
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
        self.capture_child_admission(parent)?;
        let target = id.to_owned();
        let exists = self.inner.store.read(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1)",
                [target],
                |row| row.get::<_, bool>(0),
            )?)
        })?;
        if exists {
            self.inner.ensure_admission_open(id)?;
        }
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
        let authority = self.capture_child_admission(&owner.parent)?;
        let state = self.state(&owner.id).await?;
        if state.child_continuation_unknown {
            return Err(RuntimeError::Invalid(
                "Child continuation outcome is unknown; recovery is required".into(),
            ));
        }
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
                admission_bindings: Some(authority.bindings),
                name: Some(name),
                output_schema: schema.map(|schema| schema.schema().clone()),
            }),
        )
        .await
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Rebound {
    pub from: cyber_core::worktrees::Managed,
    pub to: cyber_core::worktrees::Managed,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SetupReady {
    pub worktree_id: String,
}

impl Runtime {
    /// Bind a verified new incarnation before source-authorized setup and prompt admission.
    pub async fn rebind_child_worktree(
        &self,
        owner: &ChildExecution,
        from: &cyber_core::worktrees::Managed,
        to: &cyber_core::worktrees::Managed,
    ) -> Result<(), RuntimeError> {
        use super::events::*;
        let _admission = self.inner.open().await?;
        if !std::sync::Weak::ptr_eq(&owner.runtime.inner, &self.downgrade().inner) {
            return Err(RuntimeError::Invalid(
                "Child owner belongs to another runtime".into(),
            ));
        }
        let handle = self.inner.handle(&owner.id).await?;
        if handle
            .location_uncertain
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(RuntimeError::Invalid(
                "Location outcome unknown; recovery is required".into(),
            ));
        }
        let old = handle.state.lock().await.clone();
        self.check_rebind(owner, &old, from, to)?;
        let mut info = old.info.clone();
        info.directory = to.path.display().to_string();
        info.worktree_id = Some(to.id.clone());
        let lease = self
            .inner
            .claim_location(&info, false, self.inner.closed.child_token())
            .await?;
        let result = {
            let mut state = handle.state.lock().await;
            self.check_rebind(owner, &state, from, to).and_then(|()| {
                self.inner
                    .commit_locked(
                        &mut state,
                        vec![event(
                            WORKTREE_REBOUND,
                            &Rebound {
                                from: from.clone(),
                                to: to.clone(),
                            },
                        )],
                    )
                    .map(|_| ())
            })
        };
        lease.settle().map_err(RuntimeError::Invalid)?;
        result
    }

    fn check_rebind(
        &self,
        owner: &ChildExecution,
        state: &super::SessionState,
        from: &cyber_core::worktrees::Managed,
        to: &cyber_core::worktrees::Managed,
    ) -> Result<(), RuntimeError> {
        state.ensure_worktree_ready()?;
        if state.info.parent_id.as_deref() != Some(&owner.parent)
            || state.child_worktree.as_ref() != Some(from)
            || state.info.worktree_id.as_deref() != Some(&from.id)
            || from.id == to.id
            || !to.ready
            || from.common_dir != to.common_dir
            || from.branch != to.branch
            || from.base != to.base
            || from.name != to.name
        {
            return Err(RuntimeError::Invalid(
                "Rebind differs from the owned child checkout".into(),
            ));
        }
        if self.is_running(&owner.id)
            || self
                .jobs(Some(&owner.parent))?
                .iter()
                .any(|job| job.child_id == owner.id && job.status == super::JobStatus::Running)
            || state.calls.values().any(|call| {
                matches!(
                    call.status,
                    super::CallStatus::Dispatched | super::CallStatus::OutcomeUnknown
                )
            })
        {
            return Err(RuntimeError::Invalid(
                "Subagent busy or execution outcome unknown".into(),
            ));
        }
        Ok(())
    }

    /// Trusted setup owner acknowledges completion for the exact bound incarnation.
    pub async fn complete_child_worktree_setup(
        &self,
        id: &str,
        managed: &cyber_core::worktrees::Managed,
    ) -> Result<(), RuntimeError> {
        use super::events::*;
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        let mut state = handle.state.lock().await;
        if state.child_worktree.as_ref() != Some(managed)
            || state.info.worktree_id.as_deref() != Some(&managed.id)
        {
            return Err(RuntimeError::Invalid(
                "Setup acknowledgement differs from the owned child checkout".into(),
            ));
        }
        self.inner.commit_locked(
            &mut state,
            vec![event(
                WORKTREE_SETUP_READY,
                &SetupReady {
                    worktree_id: managed.id.clone(),
                },
            )],
        )?;
        Ok(())
    }
}

impl Runtime {
    pub async fn validate_child_setup_recovery(
        &self,
        owner: &ChildExecution,
        managed: &cyber_core::worktrees::Managed,
    ) -> Result<(), RuntimeError> {
        if !std::sync::Weak::ptr_eq(&owner.runtime.inner, &self.downgrade().inner) {
            return Err(RuntimeError::Invalid(
                "Child owner belongs to another runtime".into(),
            ));
        }
        let handle = self.inner.handle(&owner.id).await?;
        if handle
            .location_uncertain
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(RuntimeError::Invalid(
                "Location outcome unknown; recovery is required".into(),
            ));
        }
        let state = handle.state.lock().await.clone();
        if state.info.parent_id.as_deref() != Some(&owner.parent)
            || state.child_worktree.as_ref() != Some(managed)
            || state.info.worktree_id.as_deref() != Some(&managed.id)
            || !state.child_worktree_setup_pending()
        {
            return Err(RuntimeError::Invalid(
                "Recovery requires the owned pending child binding".into(),
            ));
        }
        if self.is_running(&owner.id)
            || self
                .jobs(Some(&owner.parent))?
                .iter()
                .any(|job| job.child_id == owner.id && job.status == super::JobStatus::Running)
            || state.calls.values().any(|call| {
                matches!(
                    call.status,
                    super::CallStatus::Dispatched | super::CallStatus::OutcomeUnknown
                )
            })
        {
            return Err(RuntimeError::Invalid(
                "Subagent busy or execution outcome unknown".into(),
            ));
        }
        Ok(())
    }
}
