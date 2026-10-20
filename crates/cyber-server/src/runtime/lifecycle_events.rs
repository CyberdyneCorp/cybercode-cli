//! Runtime-owned lifecycle hooks with an existing producer (`hooks` → Supported
//! events): Interrupt, PreModelSwitch/PostModelSwitch, PreCompact/PostCompact,
//! PostToolBatch, SessionEnd, TaskCreated/TaskCompleted (for background Jobs and team
//! tasks) and InstructionsLoaded, plus the Session-derived envelope identity every hook
//! event carries.
//!
//! Each hook is built under the Session lock, the lock is dropped, then the port
//! is awaited; none of these runs while the runtime lifecycle guard is held.

use std::path::Path;
use std::sync::PoisonError;

use cyber_core::hooks::HookAction;
use cyber_llm::ToolCall;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::events::{CompactionTrigger, MODEL_SWITCHED, Switched, event};
use super::host::SessionEnvelope;
use super::jobs::Job;
use super::lifecycle::{lifecycle, stop_text};
use super::teams::{IDLE_HOOK, TASK_SUBJECT, TaskUpdated};
use super::{Handle, Inner, Runtime, RuntimeError};

impl Inner {
    /// The Session's project identity, resolved off the async runtime once per
    /// Location and cached on its handle; a rebound Location resolves again.
    pub(crate) async fn envelope(&self, handle: &Handle) -> SessionEnvelope {
        let (directory, parent_session_id, workflow_run_id) = {
            let state = handle.state.lock().await;
            (
                state.info.directory.clone(),
                state.info.parent_id.clone(),
                state.info.workflow_run_id.clone(),
            )
        };
        if let Some((cached, envelope)) = handle
            .envelope
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && *cached == directory
        {
            return envelope.clone();
        }
        let location = directory.clone();
        let identified = tokio::task::spawn_blocking(move || {
            cyber_core::project::identify(Path::new(&location))
        })
        .await;
        let Ok(project) = identified else {
            // Not cached: the next event resolves again. Admission compares against
            // the same resolution, so an event cannot borrow another identity.
            return SessionEnvelope {
                project_id: "global".into(),
                workspace: None,
                parent_session_id,
                workflow_run_id,
            };
        };
        let envelope = SessionEnvelope {
            workspace: (project.id != "global").then(|| project.worktree.display().to_string()),
            project_id: project.id,
            parent_session_id,
            workflow_run_id,
        };
        *handle
            .envelope
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((directory, envelope.clone()));
        envelope
    }

    /// Observational Interrupt once the interrupted Drain and idle operations settled.
    pub(crate) async fn interrupted(&self, handle: &Handle, running: bool) {
        let hook = self
            .lifecycle_hook(
                handle,
                "Interrupt",
                String::new(),
                json!({"running": running}),
            )
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// A user model switch: PreModelSwitch may refuse it with its reason, then the
    /// switch commits and PostModelSwitch observes it.
    pub(crate) async fn switch_model_hooked(
        &self,
        handle: &Handle,
        model: &str,
    ) -> Result<(), RuntimeError> {
        let pre = {
            let state = handle.state.lock().await;
            if !state.model_selection.agent_default && state.model_selection.reference == model {
                return Ok(());
            }
            let fields = json!({"from": state.info.model, "to": model, "automatic": false});
            lifecycle(&state, "PreModelSwitch", model.into(), fields)
        };
        let verdict = self
            .tools
            .lifecycle_hooks(pre, self.closed.child_token())
            .await
            .map_err(RuntimeError::Hook)?;
        if let Some(HookAction::Deny | HookAction::Block) = verdict.decision.decision {
            return Err(RuntimeError::Invalid(format!(
                "blocked by hook {}",
                verdict.decision.reason.as_deref().unwrap_or("policy")
            )));
        }
        let post = {
            let mut state = handle.state.lock().await;
            if !state.model_selection.agent_default && state.model_selection.reference == model {
                return Ok(());
            }
            let payload = Switched {
                from: state.info.model.clone(),
                to: model.into(),
                automatic: false,
            };
            self.commit_locked(&mut state, vec![event(MODEL_SWITCHED, &payload)])?;
            lifecycle(&state, "PostModelSwitch", model.into(), switched(&payload))
        };
        self.observe_lifecycle(post).await;
        Ok(())
    }

    /// Apply the profile-derived model at a Safe Boundary. Both hooks only observe
    /// an automatic switch: the Session follows its profile and is never refused.
    pub(crate) async fn automatic_model_switch(&self, handle: &Handle) -> Result<(), RuntimeError> {
        let pre = {
            let state = handle.state.lock().await;
            let selected = self.resolve_selection(&state.info, &state.model_selection)?;
            if state.info.model == selected.reference {
                return Ok(());
            }
            let fields =
                json!({"from": state.info.model, "to": selected.reference, "automatic": true});
            lifecycle(&state, "PreModelSwitch", selected.reference, fields)
        };
        self.observe_lifecycle(pre).await;
        let post = {
            let mut state = handle.state.lock().await;
            let selected = self.resolve_selection(&state.info, &state.model_selection)?;
            if state.info.model == selected.reference {
                return Ok(());
            }
            let payload = Switched {
                from: state.info.model.clone(),
                to: selected.reference.clone(),
                automatic: true,
            };
            self.commit_locked(&mut state, vec![event(MODEL_SWITCHED, &payload)])?;
            lifecycle(
                &state,
                "PostModelSwitch",
                selected.reference,
                switched(&payload),
            )
        };
        self.observe_lifecycle(post).await;
        Ok(())
    }

    /// PreCompact before a compaction starts. Its tagged context is appended to the
    /// summary instructions, never admitted as a message; an unavailable
    /// `fail_closed` handler refuses the compaction.
    pub(crate) async fn pre_compact(
        &self,
        handle: &Handle,
        trigger: CompactionTrigger,
        instructions: Option<String>,
    ) -> Result<Option<String>, RuntimeError> {
        let fields = json!({"trigger": trigger, "instructions": instructions});
        let hook = self
            .lifecycle_hook(handle, "PreCompact", trigger_name(trigger), fields)
            .await;
        let verdict = self
            .owned_lifecycle_hooks(hook)
            .await
            .map_err(RuntimeError::Hook)?;
        Ok(match (instructions, verdict.decision.additional_context) {
            (Some(instructions), Some(context)) => Some(format!("{instructions}\n\n{context}")),
            (instructions, context) => instructions.or(context),
        })
    }

    /// Observational PostCompact after the summary committed.
    pub(crate) async fn post_compact(
        &self,
        handle: &Handle,
        trigger: CompactionTrigger,
        tokens: (u64, u64),
    ) {
        let fields =
            json!({"trigger": trigger, "tokens_before": tokens.0, "tokens_after": tokens.1});
        let hook = self
            .lifecycle_hook(handle, "PostCompact", trigger_name(trigger), fields)
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// PostToolBatch once every call of a Turn settled, interrupted ones included.
    /// Its context commits like tool-event context and `continue: false` ends the
    /// Drain after this Turn. Returns whether a hook asked to stop.
    ///
    /// It runs Drain-owned (`owned_lifecycle_hooks`), so an interrupt cancels its
    /// handlers cooperatively and waits for them, and a Drain interrupted while it
    /// runs commits nothing from it. After an interrupt (`cancel`) it only observes,
    /// in a runtime-owned task, so the interrupt never waits for it.
    pub(crate) async fn post_tool_batch(
        &self,
        handle: &Handle,
        message_id: &str,
        calls: &[ToolCall],
        cancel: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        if calls.is_empty() || self.closed.is_cancelled() {
            return Ok(false);
        }
        let hook = {
            let state = handle.state.lock().await;
            let settled: Vec<Value> = calls
                .iter()
                .map(|call| {
                    let status = state.calls.get(&call.id).map(|settled| settled.status);
                    json!({"call_id": call.id, "tool_name": call.name, "status": status})
                })
                .collect();
            let fields = json!({"message_id": message_id, "calls": settled});
            lifecycle(&state, "PostToolBatch", String::new(), fields)
        };
        if cancel.is_cancelled() {
            self.spawn_observed(hook);
            return Ok(false);
        }
        let session_id = hook.session_id.clone();
        let verdict = self.owned_lifecycle_hooks(hook).await;
        if cancel.is_cancelled() || self.closed.is_cancelled() {
            return Ok(false);
        }
        let decision = verdict.map_err(RuntimeError::Hook)?.decision;
        let mut events: Vec<_> = decision
            .additional_context
            .map(super::drain::hook_context)
            .into_iter()
            .collect();
        let stop = (decision.continuation == Some(false)).then(|| stop_text(decision.stop_reason));
        events.extend(stop.clone().map(super::drain::hook_stop));
        if !events.is_empty() {
            self.commit(handle, events).await?;
        }
        if let Some(reason) = &stop {
            self.publish_hook_stop(&session_id, reason);
        }
        Ok(stop.is_some())
    }

    /// Observe a lifecycle event in a runtime-owned task that shutdown joins. Its
    /// handlers stop with shutdown or, once a Drain owns the Session, with it.
    fn spawn_observed(&self, hook: super::host::LifecycleHook) {
        let Some(inner) = self.me.upgrade() else {
            return;
        };
        let mut tasks = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.closed.is_cancelled() {
            return;
        }
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(async move {
            inner.observe_lifecycle(hook).await;
        }));
    }

    /// Observational SessionEnd. It never blocks archiving or deletion.
    pub(crate) async fn session_end(&self, handle: &Handle, reason: &str) {
        let hook = self
            .lifecycle_hook(
                handle,
                "SessionEnd",
                reason.into(),
                json!({"reason": reason}),
            )
            .await;
        self.observe_lifecycle(hook).await;
        let session_id = handle.state.lock().await.info.id.clone();
        self.tools.end_session(&session_id).await;
    }

    /// Observational TaskCreated once a background Job is durably registered. Its
    /// handlers stop with `cancel` and settle cooperatively.
    async fn task_created(&self, job: &Job, cancel: CancellationToken) {
        if self.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.handle(&job.session_id).await else {
            return;
        };
        let hook = self
            .lifecycle_hook(
                &handle,
                "TaskCreated",
                job.kind.clone(),
                job_fields(job, false),
            )
            .await;
        if let Err(error) = self.tools.lifecycle_hooks(hook, cancel).await {
            cyber_core::log::warn(
                "hooks",
                "lifecycle hook unavailable",
                json!({"event": "TaskCreated", "session_id": job.session_id, "error": error}),
            );
        }
    }

    /// Observational TaskCompleted after a Job's terminal record, before JobEnded.
    pub(crate) async fn task_completed(&self, job: &Job) {
        self.job_hook(job, "TaskCompleted", job_fields(job, true))
            .await;
    }

    /// Observational TaskCreated or TaskCompleted (`kind`) for a team task change, on
    /// the Session of the member that made it, matched by the subject `team_task`.
    pub(crate) async fn team_task_hook(&self, session_id: &str, kind: &str, change: &TaskUpdated) {
        if self.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.handle(session_id).await else {
            return;
        };
        let hook = self
            .lifecycle_hook(&handle, kind, TASK_SUBJECT.into(), team_task_fields(change))
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// `TeammateIdle` after a teammate's idle notice is recorded, on the teammate's
    /// Session (the envelope's `session_id`), matched by its member name, with
    /// `{ team_id, member, tasks_in_progress, last_status }`. Its decisions are only
    /// logged.
    pub(crate) async fn teammate_idle_hook(&self, session_id: &str, member: &str, fields: Value) {
        if self.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.handle(session_id).await else {
            return;
        };
        let hook = self
            .lifecycle_hook(&handle, IDLE_HOOK, member.into(), fields)
            .await;
        match self.owned_lifecycle_hooks(hook).await {
            Ok(verdict) if verdict.configured => cyber_core::log::info(
                "hooks",
                "TeammateIdle decision ignored",
                json!({"session_id": session_id, "member": member, "decision": verdict.decision}),
            ),
            Ok(_) => {}
            Err(error) => cyber_core::log::warn(
                "hooks",
                "lifecycle hook unavailable",
                json!({"event": IDLE_HOOK, "session_id": session_id, "error": error}),
            ),
        }
    }

    /// Observational InstructionsLoaded for each instruction source that entered
    /// the context or changed, once per changed text.
    pub(crate) async fn instructions_loaded(
        &self,
        handle: &Handle,
        sources: Vec<super::context::LoadedInstructions>,
        reason: &str,
    ) {
        for loaded in sources {
            let fields = json!({
                "source": loaded.source,
                "load_reason": reason,
                "file_paths": loaded.files,
                "digest": format!("sha256:{:x}", Sha256::digest(loaded.text.as_bytes())),
            });
            let hook = self
                .lifecycle_hook(handle, "InstructionsLoaded", loaded.source, fields)
                .await;
            self.observe_lifecycle(hook).await;
        }
    }
}

/// TaskCreated (`{ job }`) and TaskCompleted (`{ job, status }`) fields of a Job.
fn job_fields(job: &Job, completed: bool) -> Value {
    if completed {
        json!({"job": job, "status": job.status})
    } else {
        json!({"job": job})
    }
}

/// TaskCreated and TaskCompleted fields of a team task: `{ task, team_id, member }`.
fn team_task_fields(change: &TaskUpdated) -> Value {
    json!({"task": change.task, "team_id": change.team_id, "member": change.by})
}

/// Model switch fields with an explicit `automatic` flag.
fn switched(payload: &Switched) -> Value {
    json!({"from": payload.from, "to": payload.to, "automatic": payload.automatic})
}

fn trigger_name(trigger: CompactionTrigger) -> String {
    serde_json::to_value(trigger)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Runtime {
    /// Run TaskCreated in a runtime-owned task that shutdown joins, and cancel `done`
    /// once it settled. The Job never drops it: its stop (or shutdown) cancels the
    /// handlers, which then settle their receipts and activity on their own, so a
    /// cancelled Job never leaves its parent Session's Location uncertain.
    pub(crate) fn spawn_task_created(
        &self,
        job: Job,
        cancel: CancellationToken,
        done: CancellationToken,
    ) {
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            if let Some(runtime) = weak.upgrade() {
                let hook = runtime.inner.task_created(&job, cancel.clone());
                tokio::pin!(hook);
                tokio::select! {
                    biased;
                    () = &mut hook => {}
                    () = closed.cancelled() => {
                        cancel.cancel();
                        hook.await;
                    }
                }
            }
            done.cancel();
        });
        let mut tasks = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }

    /// The Session-derived envelope identity hook events must carry (`hooks` →
    /// Supported events); hook admission refuses any other.
    pub async fn session_envelope(
        &self,
        session_id: &str,
    ) -> Result<SessionEnvelope, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        Ok(self.inner.envelope(&handle).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Team tasks reuse TaskCreated/TaskCompleted with their own fields; a Job's fields
    /// stay `{ job }` and `{ job, status }`.
    #[test]
    fn job_and_team_task_hook_fields_stay_apart() {
        let job: Job = serde_json::from_value(json!({
            "id": "job_1", "session_id": "ses_1", "child_id": "", "name": "build",
            "kind": "bash", "description": "build", "status": "completed", "started_ms": 1,
            "ended_ms": 2, "exit_code": 0, "output_path": null, "result": null, "error": null,
            "cost": 0.0, "tokens": 0, "notified": false
        }))
        .unwrap();
        assert_eq!(keys(&job_fields(&job, false)), ["job"]);
        let completed = job_fields(&job, true);
        assert_eq!(keys(&completed), ["job", "status"]);
        assert_eq!(completed["job"]["kind"], "bash");
        let change: TaskUpdated = serde_json::from_value(json!({
            "team_id": "team_1", "by": "api", "previous_status": "in_progress",
            "task": {"id": "task_1", "seq": 1, "title": "schema", "status": "done",
                "created_by": "lead", "created_ms": 1, "updated_ms": 2, "version": 2}
        }))
        .unwrap();
        let fields = team_task_fields(&change);
        assert_eq!(keys(&fields), ["member", "task", "team_id"]);
        assert_eq!(
            (&fields["task"]["id"], &fields["team_id"], &fields["member"]),
            (&json!("task_1"), &json!("team_1"), &json!("api"))
        );
        assert_eq!(TASK_SUBJECT, "team_task");
    }
}
