//! Runtime-owned lifecycle hooks (`hooks` → Stop hooks and loop prevention, Hook
//! context injection on lifecycle events): Stop before a Drain idles, observational
//! StopFailure and JobEnded, and UserPromptSubmit/SessionStart context admission.

use std::sync::atomic::Ordering;

use cyber_core::hooks::HookAction;
use cyber_llm::Content;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::bus::LiveEvent;
use super::events::{ADMITTED, Admitted, SESSION_STARTED, SessionStarted, event};
use super::host::LifecycleHook;
use super::jobs::Job;
use super::model::{Delivery, Entry, STOP_CONTINUATION_DIGEST, SessionState, text_of};
use super::{Handle, Inner, RuntimeError};

pub(crate) const STOP_LIMIT: &str = "stop hook continuation limit reached";

fn lifecycle(state: &SessionState, kind: &str, subject: String, fields: Value) -> LifecycleHook {
    LifecycleHook {
        session_id: state.info.id.clone(),
        directory: state.info.directory.clone(),
        agent: state.effective_agent(false).into(),
        mode: state.effective_mode(false).into(),
        kind: kind.into(),
        subject,
        fields: fields.as_object().cloned().unwrap_or_default(),
    }
}

impl Inner {
    /// Stop hooks at the point a Drain would go idle. A block below the
    /// continuation cap admits its reason as queued `hook` input and returns true.
    pub(crate) async fn stop_hooks(
        &self,
        handle: &Handle,
        cancel: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        let continuations = handle.stop_continuations.load(Ordering::SeqCst);
        let (session_id, hook) = {
            let state = handle.state.lock().await;
            // A schema-governed child ends through return_result; its host owns any re-prompt.
            if cancel.is_cancelled() || state.result.schema.is_some() {
                return Ok(false);
            }
            let agent = state.effective_agent(false).to_owned();
            let fields = json!({"stop_hook_active": continuations > 0});
            (
                state.info.id.clone(),
                lifecycle(&state, "Stop", agent, fields),
            )
        };
        let verdict = self.tools.lifecycle_hooks(hook, cancel.child_token()).await;
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let verdict = verdict.map_err(RuntimeError::Hook)?;
        if verdict.decision.decision != Some(HookAction::Block)
            || self.ensure_admission_open(&session_id).is_err()
        {
            return Ok(false);
        }
        if continuations >= verdict.max_stop_continuations {
            self.bus.publish(LiveEvent::HookNotice {
                session_id,
                hook_id: "Stop".into(),
                message: STOP_LIMIT.into(),
            });
            return Ok(false);
        }
        let message_id = cyber_core::ids::new_id("msg");
        let admitted = Admitted {
            skill_command: None,
            admission_bindings: None,
            wake: false,
            digest: format!("{STOP_CONTINUATION_DIGEST}{message_id}"),
            message_id,
            parts: vec![Content::Text {
                text: verdict
                    .decision
                    .reason
                    .unwrap_or_else(|| "Stop hook blocked".into()),
            }],
            delivery: Delivery::Queue,
            source: "hook".into(),
            hook_context: None,
        };
        self.commit(handle, vec![event(ADMITTED, &admitted)])
            .await?;
        handle
            .stop_continuations
            .store(continuations + 1, Ordering::SeqCst);
        Ok(true)
    }

    /// Observational StopFailure: decisions and handler failures never change the
    /// published error, and it never triggers another lifecycle event.
    pub(crate) async fn stop_failure(
        &self,
        handle: &Handle,
        kind: &str,
        message: &str,
        cancel: &CancellationToken,
    ) {
        if cancel.is_cancelled() {
            return;
        }
        let hook = {
            let state = handle.state.lock().await;
            let fields = json!({"error_kind": kind, "error": message});
            lifecycle(&state, "StopFailure", kind.into(), fields)
        };
        if let Err(error) = self.tools.lifecycle_hooks(hook, cancel.child_token()).await {
            cyber_core::log::warn(
                "hooks",
                "StopFailure hook unavailable",
                json!({"error": error}),
            );
        }
    }

    /// UserPromptSubmit before durable admission. A deny rejects the prompt with
    /// the hook's reason; context rides with the admission until promotion.
    pub(crate) async fn user_prompt_hooks(
        &self,
        handle: &Handle,
        parts: &[Content],
        source: &str,
    ) -> Result<Option<String>, RuntimeError> {
        let hook = {
            let state = handle.state.lock().await;
            let fields = json!({"prompt": text_of(parts), "source": source});
            lifecycle(&state, "UserPromptSubmit", String::new(), fields)
        };
        let verdict = self
            .tools
            .lifecycle_hooks(hook, self.closed.child_token())
            .await
            .map_err(RuntimeError::Hook)?;
        match verdict.decision.decision {
            Some(HookAction::Deny | HookAction::Block) => Err(RuntimeError::Invalid(format!(
                "blocked by hook {}",
                verdict.decision.reason.as_deref().unwrap_or("policy")
            ))),
            _ => Ok(verdict.decision.additional_context),
        }
    }

    /// SessionStart once for a root Session at its first-Turn Safe Boundary, before
    /// input promotion. Decisions are ignored; context becomes a system message.
    pub(crate) async fn session_start(
        &self,
        handle: &Handle,
        cancel: &CancellationToken,
    ) -> Result<(), RuntimeError> {
        let (source, hook) = {
            let state = handle.state.lock().await;
            // Children report their launch through the parent's SubagentStart. The
            // durable marker (or, for older Sessions, a started Turn) fences reruns.
            if state.info.parent_id.is_some() || state.session_started || state.first_turn_started {
                return Ok(());
            }
            // A fork begins with history it did not produce.
            let source = if state
                .entries
                .iter()
                .any(|entry| !matches!(entry, Entry::System { .. }))
            {
                "resume"
            } else {
                "startup"
            };
            let fields = json!({"source": source});
            (
                source,
                lifecycle(&state, "SessionStart", source.into(), fields),
            )
        };
        let verdict = self.tools.lifecycle_hooks(hook, cancel.child_token()).await;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let context = verdict
            .map_err(RuntimeError::Hook)?
            .decision
            .additional_context;
        // The marker and its context commit together, so a restart neither reruns
        // the hook nor admits the context twice.
        let mut events: Vec<_> = context
            .map(super::drain::hook_context)
            .into_iter()
            .collect();
        let marker = SessionStarted {
            source: source.into(),
        };
        events.push(event(SESSION_STARTED, &marker));
        self.commit(handle, events).await?;
        Ok(())
    }

    /// Observational JobEnded after the Job's terminal record. Handler failures and
    /// decisions never change settlement or the handback notice.
    pub(crate) async fn job_ended(&self, job: &Job) {
        if self.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.handle(&job.session_id).await else {
            return;
        };
        let hook = {
            let state = handle.state.lock().await;
            let fields = json!({"job": job, "exit_code": job.exit_code});
            lifecycle(&state, "JobEnded", job.kind.clone(), fields)
        };
        if let Err(error) = self
            .tools
            .lifecycle_hooks(hook, self.closed.child_token())
            .await
        {
            cyber_core::log::warn(
                "hooks",
                "JobEnded hook unavailable",
                json!({"job_id": job.id, "error": error}),
            );
        }
    }
}
