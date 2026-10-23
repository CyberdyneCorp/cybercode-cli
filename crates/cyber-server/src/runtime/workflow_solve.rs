//! Bind verification to a settled child owned by this run, then hold its native lease.
use super::{Runtime, WorkflowToolRequest};
use cyber_store::{Expected, NewEvent};
tokio::task_local! { static VERIFY_SESSION: String; }
pub(super) fn owns_verification(session: &str) -> bool {
    VERIFY_SESSION
        .try_with(|owner| owner == session)
        .unwrap_or(false)
}

use cyber_workflow::Diagnostic;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

impl Runtime {
    pub(super) async fn verify_solve_candidate(
        &self,
        mut request: WorkflowToolRequest,
        cancel: CancellationToken,
    ) -> Result<Value, Diagnostic> {
        let run = self
            .existing_workflow_run(&request.run_id)
            .map_err(failure)?;
        if run.origin != json!({"kind":"bundled","name":"solve"})
            || !cyber_workflow::bundled::module("solve")
                .is_some_and(|module| module.digest == run.digest)
        {
            return Err(failure("untrusted solve module"));
        }
        let label = request.input["label"]
            .as_str()
            .ok_or_else(|| failure("missing candidate label"))?;
        if !run.args["solve_admission"]["candidates"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["label"] == label))
        {
            return Err(failure("unknown solve candidate"));
        }
        let agents = self.workflow_agents(&run.id).map_err(failure)?;
        let matches: Vec<_> = agents
            .iter()
            .filter(|agent| agent.label.as_deref() == Some(label))
            .collect();
        let [agent] = matches.as_slice() else {
            return Err(failure("candidate must identify exactly one owned agent"));
        };
        if agent.status != "completed" || agent.branch.is_none() {
            return Err(failure("candidate is not settled on an isolated branch"));
        }
        let session = agent
            .session_id
            .as_deref()
            .ok_or_else(|| failure("candidate has no Session"))?;
        let (spend, blocked) =
            super::budget::check_run(&self.inner.store, &run.id, 0, u64::MAX).map_err(failure)?;
        if let Some(limit) = blocked {
            return Err(Diagnostic::new(
                cyber_workflow::DiagnosticKind::BudgetExceeded,
                limit,
            ));
        }
        let deadline = run
            .budget
            .as_ref()
            .and_then(|budget| budget.budget.max_wall_seconds)
            .map(|cap| {
                chrono::Utc::now()
                    .timestamp_millis()
                    .saturating_add(((cap - spend.elapsed_seconds).max(0.0) * 1000.0) as i64)
            });
        self.inner.ensure_idle(session).map_err(failure)?;
        let handle = self.inner.handle(session).await.map_err(failure)?;
        self.inner.check_budget(&handle).await.map_err(failure)?;
        let state = handle.state.lock().await;
        if state.info.parent_id.as_deref() != Some(&run.session_id)
            || state.info.workflow_run_id.as_deref() != Some(&run.id)
        {
            return Err(failure("candidate Session authority mismatch"));
        }
        request.owner = super::drain::turn_context_for(&state, false);
        drop(state);
        // Freeze future solver/subagent admission durably before checking active actors.
        // Only this narrowly scoped host callback may launch checks through the fence.
        let fence = format!("solve_fence_{session}");
        if self
            .inner
            .store
            .read_events(&fence, -1, 1)
            .map_err(failure)?
            .events
            .is_empty()
        {
            self.inner
                .store
                .append(
                    &fence,
                    Expected::Seq(-1),
                    vec![NewEvent::new(
                        "workflow.solve.submitted.1",
                        json!({"run_id":run.id,"candidate_id":agent.id,"session_id":session}),
                    )],
                )
                .map_err(failure)?;
        }
        let ids: std::collections::HashSet<_> = self
            .inner
            .descendants(session)
            .map_err(failure)?
            .into_iter()
            .collect();
        if self.inner.subtree_actors_live(&ids)
            || self.jobs(None).map_err(failure)?.iter().any(|job| {
                ids.contains(&job.session_id)
                    && matches!(
                        job.status,
                        super::JobStatus::Running | super::JobStatus::Interrupted
                    )
            })
        {
            return Err(failure("candidate subtree still owns unsettled work"));
        }
        request.input = json!({"base":run.args["solve_admission"]["base"], "checks":run.args["checks"],
            "revision":run.revision,"candidate":agent.id,"label":label,"branch":agent.branch,
            "model":agent.model,"cost":agent.cost,"tokens":agent.tokens,"deadline_ms":deadline});
        VERIFY_SESSION
            .scope(
                session.to_owned(),
                self.inner.with_idle_location(&handle, async {
                    let outcome = self.inner.tools.workflow_tool(request, cancel).await;
                    let unresolved = self
                        .solve_evidence(&run.id)
                        .map(|evidence| {
                            evidence.is_some_and(|evidence| {
                                evidence
                                    .candidates
                                    .iter()
                                    .filter(|row| row.candidate_id == agent.id)
                                    .flat_map(|row| &row.checks)
                                    .any(|check| check["outcome"] == "unknown")
                            })
                        })
                        .unwrap_or(true);
                    if unresolved {
                        handle
                            .location_uncertain
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    outcome.map_err(|error| super::RuntimeError::Invalid(error.to_string()))
                }),
            )
            .await
            .map_err(failure)
    }
}
fn failure(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::execution(error.to_string())
}

/// Server-derived evidence. Module output cannot populate this authority field.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct SolveEvidence {
    pub run_id: String,
    pub revision: u32,
    pub base: Value,
    pub candidates: Vec<SolveCandidateEvidence>,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct SolveCandidateEvidence {
    pub candidate_id: String,
    pub label: Option<String>,
    pub session_id: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub source_sha256: Option<String>,
    pub patch_sha256: Option<String>,
    pub checks: Vec<Value>,
    pub host_verified: bool,
    pub cost_usd: f64,
    pub tokens: u64,
}

impl Runtime {
    /// Evidence comes from host records even while a run is interrupted or has no report.
    pub fn solve_evidence(
        &self,
        run_id: &str,
    ) -> Result<Option<SolveEvidence>, super::RuntimeError> {
        let run = self.existing_workflow_run(run_id)?;
        if run.origin != json!({"kind":"bundled","name":"solve"})
            || !cyber_workflow::bundled::module("solve")
                .is_some_and(|module| module.digest == run.digest)
        {
            return Ok(None);
        }
        let checks = run.args["checks"].as_array().cloned().unwrap_or_default();
        let mut candidates = Vec::new();
        for agent in self
            .workflow_agents(run_id)?
            .into_iter()
            .filter(|agent| agent.branch.is_some())
        {
            let identity = format!("solve_{run_id}_{}_{}", run.revision, agent.id);
            let records = self.inner.store.read_events(&identity, -1, 1)?;
            let snapshot = records.events.first().map(|event| &event.data);
            let source = snapshot
                .and_then(|row| row["source_sha256"].as_str())
                .map(str::to_owned);
            let patch = snapshot
                .and_then(|row| row["patch_sha256"].as_str())
                .map(str::to_owned);
            let mut rows = Vec::new();
            let mut identities_match = true;
            for (ordinal, check) in checks.iter().enumerate() {
                let id = format!("{identity}_check_{ordinal}");
                let events = self.inner.store.read_events(&id, -1, 3)?;
                let row = events.events.get(1).or_else(|| events.events.first()).map(|event| event.data.clone())
                    .unwrap_or_else(|| json!({"id":id,"ordinal":ordinal,"argv":check["argv"],"outcome":"not_run","settled":false}));
                let definition =
                    serde_json::from_value::<cyber_workflow::solve::Check>(check.clone())
                        .ok()
                        .and_then(|check| serde_json::to_vec(&check).ok())
                        .map(|bytes| {
                            use sha2::{Digest, Sha256};
                            format!("{:x}", Sha256::digest(bytes))
                        });
                identities_match &= row["id"] == id
                    && row["run_id"] == run_id
                    && row["candidate"] == agent.id
                    && row["revision"] == run.revision
                    && row["ordinal"] == ordinal
                    && row["definition_sha256"].as_str() == definition.as_deref();
                rows.push(row);
            }
            let verified = identities_match
                && source.is_some()
                && rows.len() == checks.len()
                && !rows.is_empty()
                && rows.iter().all(|row| {
                    row["outcome"] == "passed"
                        && row["settled"] == true
                        && row["source_unchanged"] == true
                        && row["source_sha256"].as_str() == source.as_deref()
                });
            candidates.push(SolveCandidateEvidence {
                candidate_id: agent.id,
                label: agent.label,
                session_id: agent.session_id,
                branch: agent.branch,
                model: agent.model,
                source_sha256: source,
                patch_sha256: patch,
                checks: rows,
                host_verified: verified,
                cost_usd: agent.cost,
                tokens: agent.tokens,
            });
        }
        Ok(Some(SolveEvidence {
            run_id: run.id,
            revision: run.revision,
            base: run.args["solve_admission"]["base"].clone(),
            candidates,
        }))
    }
}
