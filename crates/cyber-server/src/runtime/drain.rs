//! The Drain: Safe Boundaries, Turns, tool dispatch and recovery
//! (`session-runtime` → Drain loop, Turn assembly, Interrupt; `tool-registry` → Tool recovery contract).

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use cyber_llm::catalog::compute_cost;
use cyber_llm::{
    ErrorKind, EventStream, FinishReason, LlmError, LlmEvent, ToolCall, Usage, open_with_retry,
};
use futures::future::join_all;
use futures::{FutureExt, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::bus::LiveEvent;
use super::events::*;
use super::host::{Invocation, Reconciliation, ResolvedModel, ToolDef, ToolOutcome, TurnContext};
use super::model::SessionState;
use super::model::{CallState, CallStatus, Delivery, RetrySafety};
use super::requests::{Asker, PermissionAsk, PermissionReply};
use super::view::{self, INTERRUPTED, UNKNOWN};
use super::{CompactionTrigger, Handle, Inner, RuntimeError, context};

const STEP_LIMIT_TEXT: &str = "Tools are disabled because this agent reached its maximum number of steps. \
Reply with a text summary of the work completed and the work that remains.";
const CONTINUE_TEXT: &str = "The conversation was compacted automatically. Continue the current task from where \
you left off; do not ask the user to repeat anything.";
const MAX_PARALLEL: usize = 8;

pub(crate) enum TurnEnd {
    Tools,
    Done,
    Overflow,
    Stopped,
    SelectionChanged,
}

struct PreparedModel {
    model: ResolvedModel,
    revision: i64,
    steps: Option<u64>,
}

pub(crate) async fn run(inner: Arc<Inner>, id: String, forced: bool, cancel: CancellationToken) {
    if let Ok(handle) = inner.handle(&id).await {
        *handle.auto_blocks.lock().await = 0;
    }
    let mut forced = forced;
    loop {
        // A defect must not leave the Session registered as running forever.
        let outcome = std::panic::AssertUnwindSafe(pass(&inner, &id, forced, &cancel))
            .catch_unwind()
            .await;
        let failure = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some((error_kind(&e).to_string(), e.to_string())),
            Err(_) => Some((
                "internal".to_string(),
                "the Drain stopped on an internal error; see the log".to_string(),
            )),
        };
        let succeeded = failure.is_none();
        if let Some((kind, message)) = failure {
            inner.bus.publish(LiveEvent::Error {
                session_id: id.clone(),
                kind,
                message,
            });
        }
        inner.settle_user_child(&id, &cancel, succeeded).await;
        if !inner.finish_pass(&id, &cancel) {
            if succeeded
                && !cancel.is_cancelled()
                && let Ok(handle) = inner.handle(&id).await
                && handle.state.lock().await.info.parent_id.is_some()
            {
                super::Runtime {
                    inner: Arc::clone(&inner),
                }
                .dispatch_queued_child(&id);
            }
            break;
        }
        forced = false;
    }
}

fn error_kind(e: &RuntimeError) -> &'static str {
    match e {
        RuntimeError::BudgetExceeded { .. } => "budget_exceeded",
        RuntimeError::McpRequired(_) => "mcp_required",
        RuntimeError::ContextBlocked(_) => "context_initialization_blocked",
        RuntimeError::Model(_) => "model",
        RuntimeError::Compaction(_) => "compaction_failed",
        RuntimeError::Store(_) => "storage",
        _ => "runtime",
    }
}

/// One Drain pass: run Turns while anything is eligible.
async fn pass(
    inner: &Arc<Inner>,
    id: &str,
    forced: bool,
    cancel: &CancellationToken,
) -> Result<(), RuntimeError> {
    let handle = inner.handle(id).await?;
    inner
        .with_location(
            &handle,
            cancel.child_token(),
            pass_owned(inner, &handle, forced, cancel),
        )
        .await
}

async fn pass_owned(
    inner: &Arc<Inner>,
    handle: &Arc<Handle>,
    forced: bool,
    cancel: &CancellationToken,
) -> Result<(), RuntimeError> {
    inner.ensure_admission_open(&handle.state.lock().await.info.id)?;
    inner.recover(handle).await?;
    let mut continue_tools = false;
    let mut first = forced;
    let mut overflow_retried = false;
    loop {
        if !eligible(handle, continue_tools, first, cancel).await {
            return Ok(());
        }
        let Some(resolved) = prepare_pass(inner, handle, continue_tools, first, cancel).await?
        else {
            return Ok(());
        };
        first = false;
        match inner.run_turn(handle, &resolved, cancel).await? {
            TurnEnd::SelectionChanged => first = true,
            TurnEnd::Tools => continue_tools = true,
            TurnEnd::Done => continue_tools = false,
            TurnEnd::Overflow if inner.options.compaction.auto && !overflow_retried => {
                overflow_retried = true;
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Ok(()),
                    result = inner.compact_and_continue(handle, CompactionTrigger::Overflow, &resolved.model) => result?,
                }
                continue_tools = true;
            }
            TurnEnd::Overflow => {
                return Err(RuntimeError::Model(
                    "ContextOverflow: the request exceeds the model's context window".into(),
                ));
            }
            TurnEnd::Stopped => return Ok(()),
        }
    }
}

/// Resolve the safe boundary and automatic compaction before any tool can dispatch.
async fn prepare_pass(
    inner: &Arc<Inner>,
    handle: &Arc<Handle>,
    continue_tools: bool,
    first: bool,
    cancel: &CancellationToken,
) -> Result<Option<PreparedModel>, RuntimeError> {
    if !prepare_location(inner, handle, cancel).await? {
        return Ok(None);
    }
    let (resolved, promoted) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(None),
        result = boundary(inner, handle, continue_tools) => result?,
    };
    if promoted {
        let info = handle.state.lock().await.info.clone();
        inner.tools.open_location(&info);
    }
    if !(continue_tools || promoted || first) {
        return Ok(None);
    }
    if inner.needs_compaction(handle, &resolved.model).await? {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            result = inner.compact_and_continue(handle, CompactionTrigger::Auto, &resolved.model) => result?,
        }
    }
    Ok(Some(resolved))
}

/// Initial service admission owns its readiness wait before input promotion.
async fn prepare_location(
    inner: &Arc<Inner>,
    handle: &Arc<Handle>,
    cancel: &CancellationToken,
) -> Result<bool, RuntimeError> {
    let initial = {
        let state = handle.state.lock().await;
        (!state.first_turn_started).then(|| state.info.clone())
    };
    if let Some(info) = initial {
        inner.tools.open_location(&info);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(false),
            result = inner.tools.wait_for_required_mcp(&info, cancel.child_token()) => result?,
        }
    }
    Ok(true)
}

async fn eligible(
    handle: &Handle,
    continue_tools: bool,
    first: bool,
    cancel: &CancellationToken,
) -> bool {
    if cancel.is_cancelled() {
        return false;
    }
    let compaction = handle
        .pending_compaction
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some();
    let state = handle.state.lock().await;
    state.result.returned.is_none()
        && (continue_tools
            || first
            || compaction
            || state.pending(Delivery::Steer).next().is_some()
            || state.pending(Delivery::Queue).next().is_some())
}

/// The Safe Boundary: epoch, promoted input, context updates, then requested compaction.
async fn boundary(
    inner: &Arc<Inner>,
    handle: &Arc<Handle>,
    continue_tools: bool,
) -> Result<(PreparedModel, bool), RuntimeError> {
    let prepared = {
        let mut state = handle.state.lock().await;
        let mut selected = inner.resolve_selection(&state.info, &state.model_selection)?;
        if state.mode_default_pending {
            let mode = selected
                .permission_mode
                .clone()
                .unwrap_or_else(|| "default".into());
            super::selection::validate_mode(&mode)?;
            let payload = Switched {
                from: state.info.mode.clone(),
                to: mode,
                automatic: true,
            };
            inner.commit_locked(&mut state, vec![event(MODE_SWITCHED, &payload)])?;
            selected = inner.resolve_selection(&state.info, &state.model_selection)?;
        }
        if state.info.model != selected.reference {
            let payload = Switched {
                from: state.info.model.clone(),
                to: selected.reference,
                automatic: true,
            };
            inner.commit_locked(&mut state, vec![event(MODEL_SWITCHED, &payload)])?;
        }
        PreparedModel {
            model: selected.model,
            steps: selected.steps,
            revision: state.selection_revision,
        }
    };
    let resolved = &prepared.model;
    inner.ensure_epoch(handle, &resolved.provider).await?;
    let promoted = promote(inner, handle, continue_tools).await?;
    inner.reconcile_context(handle).await?;
    let requested = handle
        .pending_compaction
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(instructions) = requested {
        inner
            .compact(handle, CompactionTrigger::Manual, instructions)
            .await?;
        inner.ensure_epoch(handle, &resolved.provider).await?;
    }
    Ok((prepared, promoted))
}

/// Promote every pending `steer` row; when the Session would otherwise go idle, the next `queue` row.
async fn promote(
    inner: &Arc<Inner>,
    handle: &Arc<Handle>,
    continue_tools: bool,
) -> Result<bool, RuntimeError> {
    let mut state = handle.state.lock().await;
    let mut ids: Vec<String> = state
        .pending(Delivery::Steer)
        .map(|r| r.message_id.clone())
        .collect();
    if ids.is_empty() && !continue_tools {
        ids.extend(
            state
                .pending(Delivery::Queue)
                .next()
                .map(|r| r.message_id.clone()),
        );
    }
    if ids.is_empty() {
        return Ok(false);
    }
    let events = ids
        .into_iter()
        .map(|message_id| event(PROMOTED, &Promoted { message_id }))
        .collect();
    inner.commit_locked(&mut state, events)?;
    let wants_title = state.info.default_title && state.info.parent_id.is_none();
    drop(state);
    if wants_title
        && !handle
            .title_requested
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        inner.spawn_title(Arc::clone(handle));
    }
    Ok(true)
}

struct Accum {
    text: String,
    reasoning: String,
    signature: Option<String>,
    calls: Vec<ToolCall>,
    usage: Usage,
    finish: Option<FinishReason>,
}

impl Inner {
    pub(crate) async fn ensure_epoch(
        &self,
        handle: &Handle,
        provider: &str,
    ) -> Result<(), RuntimeError> {
        {
            let state = handle.state.lock().await;
            if let Some(epoch) = &state.epoch
                && !state.epoch_stale
                && epoch.provider == provider
            {
                return Ok(());
            }
        }
        self.start_epoch(handle, provider).await
    }

    /// Render a fresh baseline from all current sources and start a new Context Epoch.
    pub(crate) async fn start_epoch(
        &self,
        handle: &Handle,
        provider: &str,
    ) -> Result<(), RuntimeError> {
        let mut state = handle.state.lock().await;
        let observed = self.observe(&state).await;
        let snapshot = context::snapshot(&observed).map_err(RuntimeError::ContextBlocked)?;
        let system_prefix = snapshot
            .get("core/agent")
            .cloned()
            .unwrap_or_else(|| context::base_prompt(provider));
        let payload = EpochStarted {
            epoch: state.epoch.as_ref().map_or(1, |e| e.number + 1),
            baseline: context::render_baseline(&snapshot),
            snapshot,
            provider: provider.into(),
            system_prefix: Some(system_prefix),
        };
        self.commit_locked(&mut state, vec![event(EPOCH_STARTED, &payload)])?;
        Ok(())
    }

    /// Compare sources with the epoch snapshot; changes become one system message.
    async fn reconcile_context(&self, handle: &Handle) -> Result<(), RuntimeError> {
        let mut state = handle.state.lock().await;
        let Some(epoch) = state.epoch.as_ref().filter(|_| !state.epoch_stale) else {
            return Ok(());
        };
        let observed = self.observe(&state).await;
        let (snapshot, mut text) = context::reconcile(&epoch.snapshot, &observed);
        if epoch.snapshot.contains_key("core/agent")
            && matches!(observed.get("core/agent"), Some(context::Observed::Absent))
            && let Some(text) = &mut text
        {
            text.push_str("\n\nThe default agent instructions now apply:\n");
            text.push_str(&context::base_prompt(&epoch.provider));
        }
        if let Some(text) = text {
            let payload = ContextUpdated {
                message_id: cyber_core::ids::new_id("msg"),
                text,
                snapshot,
            };
            self.commit_locked(&mut state, vec![event(CONTEXT_UPDATED, &payload)])?;
        }
        Ok(())
    }

    /// Built-in sources plus those the tool host contributes.
    async fn observe(
        &self,
        state: &SessionState,
    ) -> std::collections::BTreeMap<String, context::Observed> {
        let mut observed = context::observe(&self.context_inputs(&state.info.directory));
        let turn = turn_context_for(state, false);
        for (key, value) in self.options.tools.context_observations_owned(&turn).await {
            observed.insert(key, value);
        }
        observed
    }

    pub(crate) fn context_inputs(&self, directory: &str) -> context::ContextInputs {
        context::ContextInputs {
            directory: directory.into(),
            global_config_dir: self.options.global_config_dir.clone(),
            shell: self.options.shell.clone(),
            claude_compat: self.options.claude_compat,
            today: self.options.today.clone(),
        }
    }

    async fn compact_and_continue(
        &self,
        handle: &Handle,
        trigger: CompactionTrigger,
        resolved: &ResolvedModel,
    ) -> Result<(), RuntimeError> {
        self.compact(handle, trigger, None).await?;
        let payload = SystemAdded {
            message_id: cyber_core::ids::new_id("msg"),
            text: CONTINUE_TEXT.into(),
            reason: "auto_continue".into(),
        };
        self.commit(handle, vec![event(SYSTEM_ADDED, &payload)])
            .await?;
        self.ensure_epoch(handle, &resolved.provider).await
    }

    /// One provider request plus settlement of its tool calls.
    /// One Turn between a pre-Turn and a post-settlement snapshot.
    async fn run_turn(
        &self,
        handle: &Handle,
        resolved: &PreparedModel,
        cancel: &CancellationToken,
    ) -> Result<TurnEnd, RuntimeError> {
        let pre = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(TurnEnd::Stopped),
            pre = self.take_snapshot(handle) => pre,
        };
        let result = self.turn(handle, resolved, cancel, pre.clone()).await;
        if matches!(result, Ok(TurnEnd::SelectionChanged)) {
            return result;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = self.after_turn(handle, pre) => {},
        }
        result
    }

    async fn turn(
        &self,
        handle: &Handle,
        prepared: &PreparedModel,
        cancel: &CancellationToken,
        snapshot: Option<String>,
    ) -> Result<TurnEnd, RuntimeError> {
        self.check_budget(handle).await?;
        let Some((request, defs, limited, message_id)) =
            self.prepare_turn(handle, prepared, snapshot).await?
        else {
            return Ok(TurnEnd::SelectionChanged);
        };
        let resolved = &prepared.model;
        let session_id = request.cache_key.clone().unwrap_or_default();
        let bus = self.bus.clone();
        let gated = super::budget::GatedAdapter::new(self, handle, resolved.adapter.as_ref());
        let opened = tokio::select! {
            _ = cancel.cancelled() => None,
            r = open_with_retry(&gated, &request, &self.options.retry, |attempt, delay, e| {
                bus.publish(LiveEvent::Retry {
                    session_id: session_id.clone(), attempt, delay_ms: delay.as_millis() as u64, error: e.to_string(),
                });
            }) => Some(r),
        };
        let stream = match opened {
            None => {
                return self
                    .stop_step(
                        handle,
                        &message_id,
                        Accum::empty(),
                        "interrupted",
                        "Provider turn interrupted",
                    )
                    .await;
            }
            Some(Err(e)) => {
                if let Some(error) = gated.take_error() {
                    self.stop_step(
                        handle,
                        &message_id,
                        Accum::empty(),
                        error_kind(&error),
                        &error.to_string(),
                    )
                    .await?;
                    return Err(error);
                }
                return self.fail_step(handle, &message_id, e).await;
            }
            Some(Ok(stream)) => stream,
        };
        let acc = match self
            .consume(handle, &message_id, &defs, stream, cancel)
            .await?
        {
            Ok(acc) => acc,
            Err(end) => return Ok(end),
        };
        self.end_step(handle, &message_id, &acc, resolved).await?;
        if acc.calls.is_empty() {
            return Ok(TurnEnd::Done);
        }
        let cancelled = self
            .execute_tools(
                handle,
                resolved,
                &message_id,
                &acc.calls,
                &defs,
                limited,
                cancel,
            )
            .await?;
        if handle.state.lock().await.result.returned.is_some() {
            return Ok(TurnEnd::Stopped);
        }
        Ok(match (cancelled, limited) {
            (true, _) => TurnEnd::Stopped,
            // The step-limit Turn is final: its tool calls were refused and the Drain ends.
            (false, true) => TurnEnd::Done,
            (false, false) => TurnEnd::Tools,
        })
    }

    async fn prepare_turn(
        &self,
        handle: &Handle,
        prepared: &PreparedModel,
        snapshot: Option<String>,
    ) -> Result<Option<(cyber_llm::LlmRequest, Vec<ToolDef>, bool, String)>, RuntimeError> {
        let mut state = handle.state.lock().await;
        if state.selection_revision != prepared.revision {
            return Ok(None);
        }
        let resolved = &prepared.model;
        let limit = match (self.options.max_steps, state.info.max_steps) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let limited = limit.is_some_and(|m| state.steps_since_input >= u64::from(m))
            || prepared
                .steps
                .is_some_and(|m| state.steps_since_input >= m.saturating_sub(1));
        if limited {
            let payload = SystemAdded {
                message_id: cyber_core::ids::new_id("msg"),
                text: STEP_LIMIT_TEXT.into(),
                reason: "step_limit".into(),
            };
            self.commit_locked(&mut state, vec![event(SYSTEM_ADDED, &payload)])?;
        }
        let mut defs = if limited {
            Vec::new()
        } else {
            self.tools.refresh_location(&state.info);
            self.tools.definitions(&turn_context(&state, resolved))
        };
        defs.retain(|def| def.spec.name != "return_result");
        if !limited && let Some(schema) = &state.result.schema {
            defs.push(schema.definition());
        }
        let epoch = state
            .epoch
            .as_ref()
            .ok_or_else(|| RuntimeError::Corrupt("no context epoch".into()))?;
        let mut request = resolved.template.clone();
        request.system = vec![
            epoch
                .system_prefix
                .clone()
                .unwrap_or_else(|| context::base_prompt(&resolved.provider)),
            epoch.baseline.clone(),
        ];
        request.messages = view::messages(&state, &resolved.provider, &resolved.model);
        let settings = self
            .tools
            .deferred_tool_settings(&turn_context(&state, resolved))
            .map_err(RuntimeError::Invalid)?;
        let catalog = super::deferred_tools::materialize(defs, settings, &state.loaded_tools)
            .map_err(RuntimeError::Invalid)?;
        if !catalog.deferred.is_empty() {
            request.system.push(format!("Deferred tool names and descriptions follow as JSON. Load full schemas with tool_search before calling them.\n<deferred_tools>\n{}\n</deferred_tools>", catalog.summary().map_err(RuntimeError::Invalid)?));
        }
        request.tools = catalog.callable.iter().map(|d| d.spec.clone()).collect();
        let mut defs = catalog.callable;
        defs.extend(catalog.deferred);
        request.tools_disabled |= limited;
        request.cache_key = Some(state.info.id.clone());
        let message_id = cyber_core::ids::new_id("msg");
        let payload = StepStarted {
            agent: Some(state.info.agent.clone()),
            mode: Some(state.info.mode.clone()),
            message_id: message_id.clone(),
            provider: resolved.provider.clone(),
            model: resolved.model.clone(),
            tools: !defs.is_empty(),
            snapshot,
        };
        self.commit_locked(&mut state, vec![event(STEP_STARTED, &payload)])?;
        Ok(Some((request, defs, limited, message_id)))
    }

    /// Drain the provider stream, recording each complete tool call as it arrives.
    async fn consume(
        &self,
        handle: &Handle,
        message_id: &str,
        defs: &[ToolDef],
        mut stream: EventStream,
        cancel: &CancellationToken,
    ) -> Result<Result<Accum, TurnEnd>, RuntimeError> {
        let session_id = handle.state.lock().await.info.id.clone();
        let mut acc = Accum::empty();
        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => None,
                item = stream.next() => Some(item),
            };
            match next {
                None => {
                    let end = self
                        .stop_step(
                            handle,
                            message_id,
                            acc,
                            "interrupted",
                            "Provider turn interrupted",
                        )
                        .await?;
                    return Ok(Err(end));
                }
                Some(None) => return Ok(Ok(acc)),
                Some(Some(Err(e))) => {
                    // Output already exists, so the request is not replayed; the Turn is settled.
                    self.bus.publish(LiveEvent::Error {
                        session_id: session_id.clone(),
                        kind: kind_name(e.kind).into(),
                        message: e.message.clone(),
                    });
                    let end = self
                        .stop_step(handle, message_id, acc, kind_name(e.kind), &e.message)
                        .await?;
                    return Ok(Err(end));
                }
                Some(Some(Ok(event))) => {
                    if matches!(&event, LlmEvent::ToolCallDone(call) if acc.calls.iter().any(|previous| previous.id == call.id))
                    {
                        let end = self
                            .stop_step(
                                handle,
                                message_id,
                                acc,
                                "invalid_tool_call",
                                "Duplicate tool call id in provider response",
                            )
                            .await?;
                        return Ok(Err(end));
                    }
                    self.on_stream_event(handle, &session_id, message_id, defs, &mut acc, event)
                        .await?
                }
            }
        }
    }

    async fn on_stream_event(
        &self,
        handle: &Handle,
        session_id: &str,
        message_id: &str,
        defs: &[ToolDef],
        acc: &mut Accum,
        event_: LlmEvent,
    ) -> Result<(), RuntimeError> {
        let live = |text: String, reasoning: bool| {
            let (session_id, message_id) = (session_id.to_string(), message_id.to_string());
            if reasoning {
                LiveEvent::ReasoningDelta {
                    session_id,
                    message_id,
                    text,
                }
            } else {
                LiveEvent::TextDelta {
                    session_id,
                    message_id,
                    text,
                }
            }
        };
        match event_ {
            LlmEvent::TextDelta { text } => {
                acc.text.push_str(&text);
                self.bus.publish(live(text, false));
            }
            LlmEvent::ReasoningDelta { text } => {
                acc.reasoning.push_str(&text);
                self.bus.publish(live(text, true));
            }
            LlmEvent::ReasoningSignature { signature } => acc.signature = Some(signature),
            LlmEvent::ToolCallDelta { id, arguments, .. } => {
                self.bus.publish(LiveEvent::ToolInputDelta {
                    session_id: session_id.into(),
                    call_id: id,
                    arguments,
                });
            }
            LlmEvent::ToolCallDone(call) => {
                let retry_safety =
                    find_def(defs, &call.name).map_or(RetrySafety::Never, |d| d.retry_safety);
                let payload = ToolCalled {
                    message_id: message_id.into(),
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    input: call.input.clone(),
                    retry_safety,
                };
                self.commit(handle, vec![event(TOOL_CALLED, &payload)])
                    .await?;
                acc.calls.push(call);
            }
            LlmEvent::Usage(usage) => acc.usage.add(&usage),
            LlmEvent::Finish { reason } => acc.finish = Some(reason),
        }
        Ok(())
    }

    async fn end_step(
        &self,
        handle: &Handle,
        message_id: &str,
        acc: &Accum,
        resolved: &ResolvedModel,
    ) -> Result<(), RuntimeError> {
        let mut events = content_events(message_id, acc);
        let cost = compute_cost(resolved.cost.as_ref(), &acc.usage);
        let finish = acc.finish.clone().unwrap_or(FinishReason::Stop);
        events.push(event(
            STEP_ENDED,
            &StepEnded {
                message_id: message_id.into(),
                finish,
                usage: acc.usage,
                cost,
            },
        ));
        let session_id = {
            let mut state = handle.state.lock().await;
            self.commit_locked(&mut state, events)?;
            state.info.id.clone()
        };
        self.observe_budget(handle).await?;
        let context_tokens = acc.usage.context_tokens() + acc.usage.output + acc.usage.reasoning;
        let limit = resolved.context_limit;
        self.bus.publish(LiveEvent::Usage {
            session_id,
            usage: acc.usage,
            context_tokens,
            context_limit: limit,
            utilization: if limit == 0 {
                0.0
            } else {
                context_tokens as f64 / limit as f64
            },
        });
        Ok(())
    }

    /// Record a provider failure before any output.
    async fn fail_step(
        &self,
        handle: &Handle,
        message_id: &str,
        e: LlmError,
    ) -> Result<TurnEnd, RuntimeError> {
        let payload = StepFailed {
            message_id: message_id.into(),
            kind: kind_name(e.kind).into(),
            message: e.message.clone(),
        };
        let session_id = {
            let mut state = handle.state.lock().await;
            self.commit_locked(&mut state, vec![event(STEP_FAILED, &payload)])?;
            state.info.id.clone()
        };
        if e.kind == ErrorKind::ContextOverflow {
            return Ok(TurnEnd::Overflow);
        }
        self.bus.publish(LiveEvent::Error {
            session_id,
            kind: kind_name(e.kind).into(),
            message: e.to_string(),
        });
        Ok(TurnEnd::Stopped)
    }

    /// Keep partial output, fail the step, and settle calls that never ran as interrupted.
    async fn stop_step(
        &self,
        handle: &Handle,
        message_id: &str,
        acc: Accum,
        kind: &str,
        message: &str,
    ) -> Result<TurnEnd, RuntimeError> {
        let mut state = handle.state.lock().await;
        let mut events = content_events(message_id, &acc);
        events.push(event(
            STEP_FAILED,
            &StepFailed {
                message_id: message_id.into(),
                kind: kind.into(),
                message: message.into(),
            },
        ));
        events.extend(
            state
                .calls
                .values()
                .filter(|c| c.message_id == message_id && c.status == CallStatus::Called)
                .map(|c| {
                    settled(
                        &c.call_id,
                        CallStatus::Interrupted,
                        INTERRUPTED,
                        Some("turn stopped before dispatch"),
                    )
                }),
        );
        self.commit_locked(&mut state, events)?;
        Ok(TurnEnd::Stopped)
    }

    /// Dispatch the Turn's calls: concurrency-safe neighbours in parallel, the rest in order.
    /// Returns whether the Session was interrupted or halted.
    #[allow(clippy::too_many_arguments)]
    async fn execute_tools(
        &self,
        handle: &Handle,
        resolved: &ResolvedModel,
        message_id: &str,
        calls: &[ToolCall],
        defs: &[ToolDef],
        limited: bool,
        cancel: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        let (turn, paused) = {
            let state = handle.state.lock().await;
            let paused: Vec<String> = state
                .unresolved()
                .iter()
                .map(|c| c.call_id.clone())
                .collect();
            let mut turn = turn_context(&state, resolved);
            turn.mode = state.effective_mode(true).into();
            turn.agent = state.effective_agent(true).into();
            (turn, paused)
        };
        let groups = groups(calls, defs);
        for (index, group) in groups.iter().enumerate() {
            if handle.state.lock().await.result.returned.is_some() {
                let events = groups[index..]
                    .iter()
                    .flatten()
                    .map(|call| {
                        settled(
                            &call.id,
                            CallStatus::Interrupted,
                            "Subagent already returned its result",
                            Some("not dispatched after return_result"),
                        )
                    })
                    .collect();
                self.commit(handle, events).await?;
                return Ok(false);
            }
            if cancel.is_cancelled() || handle.halt.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                // Nothing from here on was dispatched, so it is safe to report as interrupted.
                let events = groups[index..]
                    .iter()
                    .flatten()
                    .map(|c| {
                        settled(
                            &c.id,
                            CallStatus::Interrupted,
                            INTERRUPTED,
                            Some("stopped before dispatch"),
                        )
                    })
                    .collect();
                self.commit(handle, events).await?;
                return Ok(true);
            }
            let mut runs = Vec::new();
            for call in group.iter().copied() {
                let checked = self.precheck(call, defs, limited, &paused, &turn);
                let checked = match checked {
                    Ok(ok) => self
                        .doom_check(handle, &turn, message_id, call)
                        .await
                        .map(|()| ok),
                    Err(e) => Err(e),
                };
                match checked {
                    Err((status, output)) => {
                        self.commit(handle, vec![settled(&call.id, status, &output, None)])
                            .await?;
                    }
                    Ok((def, input)) => {
                        let invocation = self
                            .dispatch(handle, &turn, message_id, call, &def, input)
                            .await?;
                        runs.push(self.run_tool(handle, def, invocation, cancel.child_token()));
                    }
                }
            }
            let outcomes = join_all(runs).await;
            let mut state = handle.state.lock().await;
            let mut seen = state
                .epoch
                .as_ref()
                .map(|epoch| epoch.reminded_skills.clone())
                .unwrap_or_default();
            let events = outcomes
                .into_iter()
                .map(|(call_id, def, outcome)| {
                    settlement_with_skills(&call_id, &def, outcome, &mut seen)
                })
                .collect();
            self.commit_locked(&mut state, events)?;
        }
        let halted = handle.halt.swap(false, std::sync::atomic::Ordering::SeqCst);
        Ok(cancel.is_cancelled() || halted)
    }

    /// Three identical calls in a row raise a `doom_loop` request
    /// (`permissions-modes` → Doom-loop detection). Unattended Modes halt instead.
    async fn doom_check(
        &self,
        handle: &Handle,
        turn: &TurnContext,
        message_id: &str,
        call: &ToolCall,
    ) -> Result<(), (CallStatus, String)> {
        let repeated = {
            let mut recent = handle
                .recent_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            recent.push(format!("{}\u{0}{}", call.name, call.arguments));
            let n = recent.len();
            n >= 3 && recent[n - 3..].iter().all(|f| *f == recent[n - 1])
        };
        if !repeated {
            return Ok(());
        }
        let halt = || {
            handle.halt.store(true, std::sync::atomic::Ordering::SeqCst);
            Err((
                CallStatus::Error,
                "Repeated identical tool calls detected".to_string(),
            ))
        };
        if matches!(turn.mode.as_str(), "auto" | "dont-ask" | "bypass") {
            return halt();
        }
        let asker = Asker::new(
            &self.me.upgrade().expect("runtime alive"),
            &turn.session_id,
            &call.id,
            message_id,
            &turn.agent,
        );
        let ask = PermissionAsk {
            action: "doom_loop".into(),
            resources: vec![call.name.clone()],
            always_patterns: vec![call.name.clone()],
            metadata: serde_json::json!({ "arguments": call.arguments }),
        };
        match asker.permission(ask).await {
            PermissionReply::Once | PermissionReply::Always => Ok(()),
            PermissionReply::Reject { message: Some(m) } => {
                Err((CallStatus::Error, format!("Rejected by user: {m}")))
            }
            PermissionReply::Reject { message: None } | PermissionReply::Unattended => halt(),
        }
    }

    /// Settlement that needs no execution, or the definition and input to dispatch.
    fn precheck(
        &self,
        call: &ToolCall,
        defs: &[ToolDef],
        limited: bool,
        paused: &[String],
        turn: &TurnContext,
    ) -> Result<(ToolDef, Value), (CallStatus, String)> {
        if limited {
            return Err((
                CallStatus::Error,
                "Tools are disabled after the maximum agent steps".into(),
            ));
        }
        let def = find_def(defs, &call.name)
            .ok_or_else(|| (CallStatus::Error, format!("Unknown tool: {}", call.name)))?;
        if def.deferred {
            return Err((
                CallStatus::Error,
                format!(
                    "Tool {} is deferred. Load it with tool_search first.",
                    def.spec.name
                ),
            ));
        }
        let input = call
            .input
            .clone()
            .ok_or_else(|| (CallStatus::Error, invalid_arguments(&call.arguments)))?;
        if def.retry_safety != RetrySafety::ReadOnly && !paused.is_empty() {
            return Err((
                CallStatus::Error,
                format!(
                    "Mutating tools are paused until the unknown outcome of {} is reconciled. Ask the user to resolve it.",
                    paused.join(", ")
                ),
            ));
        }
        // The registration may have been removed or replaced since it was advertised.
        let current = self.tools.definitions(turn);
        if def.spec.name != "return_result"
            && !current.iter().any(|d| {
                d.spec.name == def.spec.name
                    && d.registration == def.registration
                    && d.scope == def.scope
            })
        {
            return Err((CallStatus::Error, format!("Stale tool call: {}", call.name)));
        }
        Ok((def.clone(), input))
    }

    /// Persist the dispatch record before running anything.
    pub(super) async fn dispatch(
        &self,
        handle: &Handle,
        turn: &TurnContext,
        message_id: &str,
        call: &ToolCall,
        def: &ToolDef,
        input: Value,
    ) -> Result<Invocation, RuntimeError> {
        let attempt = handle
            .state
            .lock()
            .await
            .calls
            .get(&call.id)
            .map_or(0, |c| c.attempt)
            + 1;
        let operation_key = format!("{}:{}", turn.session_id, call.id);
        let payload = ToolDispatched {
            call_id: call.id.clone(),
            attempt,
            input_digest: format!("sha256:{:x}", Sha256::digest(input.to_string())),
            operation_key: operation_key.clone(),
        };
        self.commit(handle, vec![event(TOOL_DISPATCHED, &payload)])
            .await?;
        let inner = self.me.upgrade().expect("runtime alive");
        Ok(Invocation {
            registration: def.registration.clone(),
            session_id: turn.session_id.clone(),
            directory: turn.directory.clone(),
            agent: turn.agent.clone(),
            mode: turn.mode.clone(),
            message_id: message_id.into(),
            call_id: call.id.clone(),
            name: def.spec.name.clone(),
            input,
            attempt,
            operation_key,
            asker: Asker::new(&inner, &turn.session_id, &call.id, message_id, &turn.agent),
            rules: turn.rules.clone(),
        })
    }

    /// Run one call; after cancellation the tool has 2 seconds to stop.
    pub(super) async fn run_tool(
        &self,
        handle: &Handle,
        def: ToolDef,
        invocation: Invocation,
        cancel: CancellationToken,
    ) -> (String, ToolDef, ToolOutcome) {
        let call_id = invocation.call_id.clone();
        if invocation.name == "return_result" {
            let state = handle.state.lock().await;
            let outcome = state.result.schema.as_ref().map_or_else(
                || ToolOutcome::Failed("return_result requires an output schema".into()),
                |schema| schema.outcome(invocation.input),
            );
            return (call_id, def, outcome);
        }
        let run = self.tools.execute(invocation, cancel.clone());
        tokio::pin!(run);
        let outcome = tokio::select! {
            out = &mut run => out,
            _ = cancel.cancelled() => match tokio::time::timeout(Duration::from_secs(2), &mut run).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    handle.location_uncertain.store(true, std::sync::atomic::Ordering::SeqCst);
                    ToolOutcome::Aborted
                }
            },
        };
        (call_id, def, outcome)
    }

    /// Settle what a previous process left unfinished (`session-runtime` → Drain loop).
    pub(crate) async fn recover(&self, handle: &Handle) -> Result<(), RuntimeError> {
        let (open_step, unfinished, directory) = {
            let state = handle.state.lock().await;
            let unfinished: Vec<CallState> = state
                .calls
                .values()
                .filter(|c| !c.status.is_settled())
                .cloned()
                .collect();
            (
                state.open_step.clone(),
                unfinished,
                state.info.directory.clone(),
            )
        };
        let mut events = Vec::new();
        if let Some(message_id) = open_step {
            events.push(event(
                STEP_FAILED,
                &StepFailed {
                    message_id,
                    kind: "interrupted".into(),
                    message: "Provider turn interrupted".into(),
                },
            ));
        }
        for call in unfinished {
            events.push(self.recover_call(&directory, &call).await);
        }
        self.commit(handle, events).await?;
        Ok(())
    }

    async fn recover_call(&self, directory: &str, call: &CallState) -> cyber_store::NewEvent {
        if call.status == CallStatus::Called {
            return settled(
                &call.call_id,
                CallStatus::Interrupted,
                INTERRUPTED,
                Some("recovered: never dispatched"),
            );
        }
        if call.retry_safety == RetrySafety::ReadOnly {
            return settled(
                &call.call_id,
                CallStatus::Interrupted,
                INTERRUPTED,
                Some("recovered: read-only call"),
            );
        }
        match self.tools.reconcile(directory, call).await {
            Reconciliation::Succeeded { evidence } => settled(
                &call.call_id,
                CallStatus::Ok,
                &format!("[Recovered after a restart: the call completed] {evidence}"),
                Some(&evidence),
            ),
            Reconciliation::NotApplied { evidence } => settled(
                &call.call_id,
                CallStatus::Interrupted,
                INTERRUPTED,
                Some(&evidence),
            ),
            Reconciliation::Unknown => settled(
                &call.call_id,
                CallStatus::OutcomeUnknown,
                UNKNOWN,
                Some("recovered: outcome unknown"),
            ),
        }
    }
}

impl Accum {
    fn empty() -> Self {
        Self {
            text: String::new(),
            reasoning: String::new(),
            signature: None,
            calls: Vec::new(),
            usage: Usage::default(),
            finish: None,
        }
    }
}

fn content_events(message_id: &str, acc: &Accum) -> Vec<cyber_store::NewEvent> {
    let mut events = Vec::new();
    if !acc.reasoning.is_empty() {
        let payload = ContentEnded {
            message_id: message_id.into(),
            text: acc.reasoning.clone(),
            signature: acc.signature.clone(),
        };
        events.push(event(REASONING_ENDED, &payload));
    }
    if !acc.text.is_empty() {
        events.push(event(
            TEXT_ENDED,
            &ContentEnded {
                message_id: message_id.into(),
                text: acc.text.clone(),
                signature: None,
            },
        ));
    }
    events
}

fn settled(
    call_id: &str,
    status: CallStatus,
    output: &str,
    detail: Option<&str>,
) -> cyber_store::NewEvent {
    let payload = ToolSettled {
        skill_reminders: Vec::new(),
        structured_output: None,
        call_id: call_id.into(),
        status,
        output: output.into(),
        detail: detail.map(str::to_string),
    };
    event(TOOL_SETTLED, &payload)
}

/// The caller holds the Session lock and commits the resulting events as one transaction.
pub(super) fn settlement_with_skills(
    call_id: &str,
    def: &ToolDef,
    outcome: ToolOutcome,
    seen: &mut std::collections::BTreeSet<String>,
) -> cyber_store::NewEvent {
    let ToolOutcome::SkillSuggestions {
        mut output,
        value,
        skills,
    } = outcome
    else {
        return settlement(call_id, def, outcome);
    };
    let mut names = Vec::new();
    let mut lines = Vec::new();
    for skill in skills {
        if cyber_core::skills::validate_name(&skill.name).is_err() {
            continue;
        }
        if seen.insert(skill.name.clone()) {
            names.push(skill.name.clone());
            let description: String = skill
                .description
                .chars()
                .take(120)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let escaped = description
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            lines.push(format!("- {}: {}", skill.name, escaped));
        }
    }
    if !lines.is_empty() {
        output.push_str(&format!("\n\n<system-reminder>\nConsider loading these matching skills with the skill tool:\n{}\n</system-reminder>", lines.join("\n")));
    }
    event(
        TOOL_SETTLED,
        &ToolSettled {
            skill_reminders: names,
            structured_output: value,
            call_id: call_id.into(),
            status: CallStatus::Ok,
            output,
            detail: None,
        },
    )
}

/// Map an outcome to a settlement. A dispatched mutation that did not finish has an unknown outcome.
pub(super) fn settlement(
    call_id: &str,
    def: &ToolDef,
    outcome: ToolOutcome,
) -> cyber_store::NewEvent {
    let read_only = def.retry_safety == RetrySafety::ReadOnly;
    match outcome {
        ToolOutcome::Ok(output)
        | ToolOutcome::SkillSuggestions {
            output,
            value: None,
            ..
        } => settled(call_id, CallStatus::Ok, &output, None),
        ToolOutcome::Structured { output, value }
        | ToolOutcome::SkillSuggestions {
            output,
            value: Some(value),
            ..
        } => event(
            TOOL_SETTLED,
            &ToolSettled {
                skill_reminders: Vec::new(),
                structured_output: Some(value),
                call_id: call_id.into(),
                status: CallStatus::Ok,
                output,
                detail: None,
            },
        ),
        ToolOutcome::Failed(message) => settled(call_id, CallStatus::Error, &message, None),
        ToolOutcome::Aborted if read_only => settled(
            call_id,
            CallStatus::Interrupted,
            INTERRUPTED,
            Some("aborted"),
        ),
        ToolOutcome::Aborted => settled(
            call_id,
            CallStatus::OutcomeUnknown,
            UNKNOWN,
            Some("aborted after dispatch"),
        ),
        ToolOutcome::Crashed(detail) => {
            let reference = cyber_core::ids::new_id("err");
            cyber_core::log::error(
                "tools",
                "tool crashed",
                serde_json::json!({ "call_id": call_id, "ref": reference, "detail": detail }),
            );
            let status = if read_only {
                CallStatus::Error
            } else {
                CallStatus::OutcomeUnknown
            };
            settled(
                call_id,
                status,
                &format!("Tool crashed: {reference}"),
                Some(&format!("{reference}: {detail}")),
            )
        }
    }
}

/// Look a tool up by name, repairing a case mismatch.
fn find_def<'a>(defs: &'a [ToolDef], name: &str) -> Option<&'a ToolDef> {
    defs.iter()
        .find(|d| d.spec.name == name)
        .or_else(|| defs.iter().find(|d| d.spec.name == name.to_lowercase()))
}

fn groups<'a>(calls: &'a [ToolCall], defs: &[ToolDef]) -> Vec<Vec<&'a ToolCall>> {
    let mut groups: Vec<Vec<&ToolCall>> = Vec::new();
    let mut parallel_open = false;
    for call in calls {
        let safe = find_def(defs, &call.name).is_some_and(|d| d.concurrency_safe);
        match groups.last_mut() {
            Some(group) if safe && parallel_open && group.len() < MAX_PARALLEL => group.push(call),
            _ => groups.push(vec![call]),
        }
        parallel_open = safe;
    }
    groups
}

fn invalid_arguments(arguments: &str) -> String {
    let error = serde_json::from_str::<Value>(arguments)
        .err()
        .map_or_else(|| "not a JSON object".to_string(), |e| e.to_string());
    format!("The arguments provided to the tool are invalid: {error}")
}

fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Authentication => "authentication",
        ErrorKind::RateLimit => "rate_limit",
        ErrorKind::QuotaExceeded => "quota_exceeded",
        ErrorKind::ContextOverflow => "context_overflow",
        ErrorKind::ContentPolicy => "content_policy",
        ErrorKind::InvalidRequest => "invalid_request",
        ErrorKind::ProviderInternal => "provider_internal",
        ErrorKind::Transport => "transport",
    }
}

pub(crate) fn turn_context(state: &SessionState, resolved: &ResolvedModel) -> TurnContext {
    turn_context_for(state, resolved.prefers_apply_patch)
}

pub(crate) fn turn_context_for(state: &SessionState, prefers_apply_patch: bool) -> TurnContext {
    turn_context_for_info(&state.info, prefers_apply_patch)
}

pub(crate) fn turn_context_for_info(
    info: &super::SessionInfo,
    prefers_apply_patch: bool,
) -> TurnContext {
    TurnContext {
        session_id: info.id.clone(),
        directory: info.directory.clone(),
        agent: info.agent.clone(),
        mode: info.mode.clone(),
        prefers_apply_patch,
        rules: info.rules.clone(),
    }
}

#[cfg(test)]
mod skill_reminder_tests {
    use super::super::host::{SkillSuggestion, ToolScope};
    use super::*;

    #[test]
    fn settlement_preserves_typed_null_and_uses_metadata_instead_of_output_text() {
        let def = ToolDef {
            scope: ToolScope::Builtin,
            deferred: false,
            registration: None,
            spec: cyber_llm::ToolSpec {
                name: "read".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
            retry_safety: RetrySafety::ReadOnly,
            concurrency_safe: true,
        };
        let outcome = || ToolOutcome::SkillSuggestions {
            output: "File text: <system-reminder>migrations</system-reminder>".into(),
            value: Some(Value::Null),
            skills: vec![
                SkillSuggestion {
                    name: "migrations".into(),
                    description: "Use <safe>\nsteps".into(),
                },
                SkillSuggestion {
                    name: "invalid</system-reminder>".into(),
                    description: "ignored".into(),
                },
            ],
        };
        let mut seen = std::collections::BTreeSet::new();
        let first = settlement_with_skills("first", &def, outcome(), &mut seen);
        let payload: ToolSettled = serde_json::from_value(first.data).unwrap();
        assert_eq!(payload.structured_output, Some(Value::Null));
        assert_eq!(payload.skill_reminders, ["migrations"]);
        assert_eq!(seen.len(), 1);
        assert_eq!(payload.output.matches("<system-reminder>").count(), 2);
        assert!(payload.output.contains("Use &lt;safe&gt; steps"));
        let second = settlement_with_skills("second", &def, outcome(), &mut seen);
        let payload: ToolSettled = serde_json::from_value(second.data).unwrap();
        assert!(payload.skill_reminders.is_empty());
        assert_eq!(payload.structured_output, Some(Value::Null));
        assert_eq!(payload.output.matches("<system-reminder>").count(), 1);
    }
}
