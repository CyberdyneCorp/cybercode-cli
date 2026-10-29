//! Runtime-owned lifecycle hooks (`hooks` → Stop hooks and loop prevention, Hook
//! context injection on lifecycle events): Stop before a Drain idles, observational
//! StopFailure and JobEnded, UserPromptSubmit/SessionStart context admission, and
//! `continue: false` for Stop, SessionStart and UserPromptSubmit.

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

pub(crate) fn lifecycle(
    state: &SessionState,
    kind: &str,
    subject: String,
    fields: Value,
) -> LifecycleHook {
    LifecycleHook {
        session_id: state.info.id.clone(),
        directory: state.info.directory.clone(),
        agent: state.effective_agent(false).into(),
        mode: state.effective_mode(false).into(),
        kind: kind.into(),
        subject,
        fields: fields.as_object().cloned().unwrap_or_default(),
        workflow_run_id: None,
    }
}

/// What UserPromptSubmit left for an admitted prompt: its `[hook <id>]`-tagged
/// context and stop request, both applied when the row is promoted.
#[derive(Debug, Default)]
pub(crate) struct PromptEffects {
    pub(crate) context: Option<String>,
    pub(crate) stop: Option<String>,
}

impl PromptEffects {
    /// The context of a prompt that runs no Turn of its own here (a delegated or
    /// forked prompt). A stop request has nothing to bound, so it refuses the prompt
    /// rather than being dropped.
    pub(crate) fn without_turn(self) -> Result<Option<String>, RuntimeError> {
        match self.stop {
            Some(reason) => Err(RuntimeError::Invalid(format!(
                "stopped by hook {reason}; the delegated prompt was not started"
            ))),
            None => Ok(self.context),
        }
    }

    /// A forked skill command hands its prompt to a child instead of a Turn here.
    pub(crate) fn for_prompt(self, fork: bool) -> Result<Self, RuntimeError> {
        if !fork {
            return Ok(self);
        }
        Ok(Self {
            context: self.without_turn()?,
            stop: None,
        })
    }
}

/// The host tags a merged stop reason with its hook; an untagged one still records.
pub(crate) fn stop_text(reason: Option<String>) -> String {
    reason.unwrap_or_else(|| "continue: false".into())
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
        // `continue: false` wins over a block: no continuation, the Drain idles.
        if verdict.decision.continuation == Some(false) {
            self.record_hook_stop(handle, &session_id, verdict.decision.stop_reason)
                .await?;
            return Ok(false);
        }
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
            observation_scope: None,
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
            hook_stop: None,
            resources: Vec::new(),
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
        let fields = json!({"error_kind": kind, "error": message});
        let hook = self
            .lifecycle_hook(handle, "StopFailure", kind.into(), fields)
            .await;
        if let Err(error) = self.tools.lifecycle_hooks(hook, cancel.child_token()).await {
            cyber_core::log::warn(
                "hooks",
                "StopFailure hook unavailable",
                json!({"error": error}),
            );
        }
    }

    /// UserPromptSubmit before durable admission. A deny rejects the prompt with
    /// the hook's reason; context and a `continue: false` ride with the admission
    /// until promotion.
    pub(crate) async fn user_prompt_hooks(
        &self,
        handle: &Handle,
        parts: &[Content],
        source: &str,
    ) -> Result<PromptEffects, RuntimeError> {
        let fields = json!({"prompt": text_of(parts), "source": source});
        let hook = self
            .lifecycle_hook(handle, "UserPromptSubmit", String::new(), fields)
            .await;
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
            _ => Ok(PromptEffects {
                context: verdict.decision.additional_context,
                stop: (verdict.decision.continuation == Some(false))
                    .then(|| stop_text(verdict.decision.stop_reason)),
            }),
        }
    }

    /// SessionStart once for a root Session at its first-Turn Safe Boundary, before
    /// input promotion. Its allow/deny decision is ignored; context becomes a system
    /// message and `continue: false` ends the Drain once the first Turn completes.
    pub(crate) async fn session_start(
        &self,
        handle: &Handle,
        cancel: &CancellationToken,
    ) -> Result<(), RuntimeError> {
        let (source, steps, hook) = {
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
                state.totals.steps,
                lifecycle(&state, "SessionStart", source.into(), fields),
            )
        };
        let session_id = hook.session_id.clone();
        let verdict = self.tools.lifecycle_hooks(hook, cancel.child_token()).await;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let verdict = verdict.map_err(RuntimeError::Hook)?;
        // Without a SessionStart definition nothing ran, so there is nothing to fence
        // and no durable write; the first Turn's start fences any later pass.
        if !verdict.configured {
            return Ok(());
        }
        let decision = verdict.decision;
        // The marker, its context and any stop request commit together, so a restart
        // neither reruns the hook nor admits the context twice, and the first Turn
        // still ends the Drain after a restart.
        let mut events: Vec<_> = decision
            .additional_context
            .map(super::drain::hook_context)
            .into_iter()
            .collect();
        let stop = (decision.continuation == Some(false)).then(|| stop_text(decision.stop_reason));
        events.extend(stop.clone().map(super::drain::hook_stop));
        let marker = SessionStarted {
            source: source.into(),
            stop_after_step: stop.is_some().then_some(steps + 1),
        };
        events.push(event(SESSION_STARTED, &marker));
        self.commit(handle, events).await?;
        if let Some(reason) = stop {
            self.publish_hook_stop(&session_id, &reason);
        }
        Ok(())
    }

    /// A SessionStart `continue: false` ends the Drain once the first Turn completes.
    pub(crate) async fn hook_stop_reached(&self, handle: &Handle) -> bool {
        let state = handle.state.lock().await;
        state.hook_stop_step == Some(state.totals.steps)
    }

    /// Durably record a lifecycle hook's `continue: false` and show its reason.
    async fn record_hook_stop(
        &self,
        handle: &Handle,
        session_id: &str,
        reason: Option<String>,
    ) -> Result<(), RuntimeError> {
        let reason = stop_text(reason);
        self.commit(handle, vec![super::drain::hook_stop(reason.clone())])
            .await?;
        self.publish_hook_stop(session_id, &reason);
        Ok(())
    }

    /// Show a hook stop reason as a notice from the hook its tag names.
    pub(crate) fn publish_hook_stop(&self, session_id: &str, reason: &str) {
        let (hook_id, message) = reason
            .strip_prefix("[hook ")
            .and_then(|rest| rest.split_once("] "))
            .unwrap_or(("hook", reason));
        self.bus.publish(LiveEvent::HookNotice {
            session_id: session_id.into(),
            hook_id: hook_id.into(),
            message: message.into(),
        });
    }

    /// Whether any Session, archived ones included, already lives in `directory`.
    pub(crate) fn location_has_sessions(&self, directory: &str) -> Result<bool, RuntimeError> {
        let directory = directory.to_owned();
        Ok(self.store.read(move |conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE directory=?1)",
                [&directory],
                |row| row.get::<_, bool>(0),
            )?)
        })?)
    }

    /// Observational Setup for the first root Session of a Location. It runs once,
    /// after creation commits: a crash in between skips it rather than repeating it.
    pub(crate) async fn setup(&self, session_id: &str) {
        let Ok(handle) = self.handle(session_id).await else {
            return;
        };
        let fields = json!({"trigger": "first_session"});
        let hook = self
            .lifecycle_hook(&handle, "Setup", "first_session".into(), fields)
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// Observational ConfigChange at a Safe Boundary when the Location configuration
    /// differs from what this Session last saw. The first look only records it, so a
    /// change made while no Drain ran is reported at the next one of this process.
    pub(crate) async fn config_change(&self, handle: &Handle) {
        let directory = handle.state.lock().await.info.directory.clone();
        let Some(digest) = self.tools.config_digest(&directory) else {
            return;
        };
        let previous = handle
            .config_digest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(digest.clone());
        let Some(previous) = previous.filter(|previous| *previous != digest) else {
            return;
        };
        let fields = json!({"previous_digest": previous, "digest": digest});
        let hook = self
            .lifecycle_hook(handle, "ConfigChange", "configuration".into(), fields)
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// Observational JobEnded after the Job's terminal record. Handler failures and
    /// decisions never change settlement or the handback notice.
    pub(crate) async fn job_ended(&self, job: &Job) {
        self.job_hook(
            job,
            "JobEnded",
            json!({"job": job, "exit_code": job.exit_code}),
        )
        .await;
    }

    /// An observational Job event on the Job's parent Session, matched by Job kind.
    pub(crate) async fn job_hook(&self, job: &Job, kind: &str, fields: Value) {
        if self.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.handle(&job.session_id).await else {
            return;
        };
        let hook = self
            .lifecycle_hook(&handle, kind, job.kind.clone(), fields)
            .await;
        self.observe_lifecycle(hook).await;
    }

    /// Capture a Session's lifecycle hook under its lock. The caller awaits the
    /// hook only after the lock drops.
    pub(crate) async fn lifecycle_hook(
        &self,
        handle: &Handle,
        kind: &str,
        subject: String,
        fields: Value,
    ) -> LifecycleHook {
        let state = handle.state.lock().await;
        lifecycle(&state, kind, subject, fields)
    }

    /// Dispatch an observational lifecycle event: decisions are ignored and handler
    /// failures, including an unavailable `fail_closed` handler, are logged only.
    /// Never call this while holding the runtime lifecycle guard or a Session lock.
    pub(crate) async fn observe_lifecycle(&self, hook: LifecycleHook) {
        if self.closed.is_cancelled() {
            return;
        }
        let (kind, session_id) = (hook.kind.clone(), hook.session_id.clone());
        if let Err(error) = self.owned_lifecycle_hooks(hook).await {
            cyber_core::log::warn(
                "hooks",
                "lifecycle hook unavailable",
                json!({"event": kind, "session_id": session_id, "error": error}),
            );
        }
    }

    /// Dispatch a runtime lifecycle hook under its Session's Drain when one runs: the
    /// handler's token is a child of the Drain token (of shutdown otherwise), so an
    /// interrupt cancels it cooperatively, and a cancelled Safe Boundary waits for it
    /// to settle rather than dropping it after launch (`drain::unless_cancelled`).
    pub(crate) async fn owned_lifecycle_hooks(
        &self,
        hook: LifecycleHook,
    ) -> Result<super::host::LifecycleVerdict, String> {
        self.owned_lifecycle_hooks_cancelled(hook, None).await
    }

    /// An independent observational owner may cancel its own work. Keep the
    /// launched future alive until its runner cooperatively settles the receipt.
    pub(crate) async fn owned_lifecycle_hooks_cancelled(
        &self,
        hook: LifecycleHook,
        owner: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<super::host::LifecycleVerdict, String> {
        let drain = self
            .drains
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&hook.session_id)
            .map(|entry| entry.cancel.child_token());
        let handle = drain.as_ref().and_then(|_| {
            self.sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&hook.session_id)
                .cloned()
        });
        let _in_flight = handle.map(DrainHook::enter);
        let cancel = drain.unwrap_or_else(|| self.closed.child_token());
        let Some(owner) = owner else {
            return self.tools.lifecycle_hooks(hook, cancel).await;
        };
        if owner.is_cancelled() {
            cancel.cancel();
        }
        let future = self.tools.lifecycle_hooks(hook, cancel.clone());
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => result,
            _ = owner.cancelled() => {
                cancel.cancel();
                future.await
            }
        }
    }
}

/// One Drain-owned lifecycle hook in flight on its Session's handle.
struct DrainHook(std::sync::Arc<Handle>);

impl DrainHook {
    fn enter(handle: std::sync::Arc<Handle>) -> Self {
        handle.drain_hooks.fetch_add(1, Ordering::SeqCst);
        Self(handle)
    }
}

impl Drop for DrainHook {
    fn drop(&mut self) {
        self.0.drain_hooks.fetch_sub(1, Ordering::SeqCst);
        self.0.drain_hooks_settled.notify_waiters();
    }
}
