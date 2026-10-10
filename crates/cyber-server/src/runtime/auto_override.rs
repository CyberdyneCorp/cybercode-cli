//! Confirmed exact-call replay; consumption survives restart and failed execution.
use super::{events::*, *};
use cyber_store::{NewEvent, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub(super) const CHANGED: &str = "permission.auto_override.changed.1";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AutoOverrideReceipt {
    pub id: String,
    pub decision_id: String,
    pub original_call_id: String,
    pub call_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    receipt: AutoOverrideReceipt,
    directory: String,
    activity_id: String,
    status: String,
}

#[derive(Clone)]
pub(super) struct Grant {
    session_id: String,
    call_id: String,
    tool: String,
    input: Value,
    authority: AdmissionAuthority,
    cancel: CancellationToken,
    done: CancellationToken,
}

impl Asker {
    /// A captured replay invocation cannot become an ordinary call after its grant expires.
    pub fn validate_auto_override(&self, tool: &str, input: &Value) -> Result<(), RuntimeError> {
        if self.auto_override.is_none() || self.auto_override_matches(tool, input) {
            return Ok(());
        }
        Err(RuntimeError::Invalid(
            "One-shot override does not authorize this call or is no longer active".into(),
        ))
    }
    /// Only runtime-created, confirmed exact-call grants can satisfy this check.
    pub fn auto_override_matches(&self, tool: &str, input: &Value) -> bool {
        let Some(grant) = &self.auto_override else {
            return false;
        };
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        grant.session_id == self.session_id
            && grant.call_id == self.call_id
            && grant.tool == tool
            && grant.input == *input
            && !grant.cancel.is_cancelled()
            && !grant.done.is_cancelled()
            && grant
                .authority
                .verify(&Runtime { inner }, &grant.session_id)
                .is_ok()
    }
}

fn change(record: &Record) -> NewEvent {
    event(CHANGED, record)
}

fn decision(tx: &Transaction<'_>, session: &str, id: &str) -> Result<AutoDecision, String> {
    let data: String = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND id=?2 AND type=?3",
            params![session, id, AUTO_DECIDED],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let decision: AutoDecision = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    if decision.decision != AutoEffect::Block || decision.model.is_none() {
        return Err("Only classifier-backed blocks can be overridden".into());
    }
    Ok(decision)
}

pub(super) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if e.kind != CHANGED {
        return Ok(());
    }
    let record: Record = serde_json::from_value(e.data.clone()).map_err(|e| e.to_string())?;
    let original = decision(tx, &e.aggregate_id, &record.receipt.decision_id)?;
    if original.call_id != record.receipt.original_call_id {
        return Err("Override call identity mismatch".into());
    }
    if record.receipt.id.is_empty()
        || record.receipt.call_id.is_empty()
        || record.receipt.call_id == record.receipt.original_call_id
        || original.checkout_root.is_none()
    {
        return Err("Override needs a fresh replay identity and recorded checkout".into());
    }
    let previous: Option<String> = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 AND seq<?3
        AND json_extract(data,'$.receipt.decision_id')=?4 ORDER BY seq DESC LIMIT 1",
            params![e.aggregate_id, CHANGED, e.seq, record.receipt.decision_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let previous: Option<Record> = previous
        .map(|v| serde_json::from_str(&v))
        .transpose()
        .map_err(|e| e.to_string())?;
    match record.status.as_str() {
        "pending"
            if previous.as_ref().is_none_or(|p| {
                p.status == "cancelled"
                    && p.receipt.id != record.receipt.id
                    && p.receipt.call_id != record.receipt.call_id
            }) => {}
        "consumed" | "cancelled"
            if previous.as_ref().is_some_and(|p| {
                p.status == "pending"
                    && p.receipt.id == record.receipt.id
                    && p.receipt.call_id == record.receipt.call_id
                    && p.activity_id == record.activity_id
                    && p.directory == record.directory
            }) => {}
        "settled"
            if previous.as_ref().is_some_and(|p| {
                p.status == "consumed"
                    && p.receipt.id == record.receipt.id
                    && p.receipt.call_id == record.receipt.call_id
                    && p.activity_id == record.activity_id
                    && p.directory == record.directory
            }) => {}
        _ => {
            return Err(
                "Override is already claimed, consumed, or has an invalid transition".into(),
            );
        }
    }
    if matches!(record.status.as_str(), "pending" | "consumed") {
        validate_activity(tx, &e.aggregate_id, &record)?;
    }
    if record.status == "consumed" {
        let confirmed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM event ask JOIN event reply
            ON reply.aggregate_id=ask.aggregate_id AND json_extract(reply.data,'$.request_id')=json_extract(ask.data,'$.id')
            WHERE ask.aggregate_id=?1 AND ask.type=?2 AND reply.type=?3
            AND json_extract(ask.data,'$.call_id')=?4 AND json_extract(ask.data,'$.action')='auto_override'
            AND json_extract(ask.data,'$.metadata.classifier_reason')=?5
            AND json_extract(ask.data,'$.metadata.original_call_id')=?6
            AND json_extract(ask.data,'$.metadata.requires_confirmation')=1
            AND json_array_length(json_extract(ask.data,'$.always_patterns'))=0
            AND json_extract(reply.data,'$.reply.reply') IN ('once','always'))",
            params![e.aggregate_id,PERMISSION_ASKED,PERMISSION_REPLIED,record.receipt.call_id,original.reason,original.call_id], |r|r.get(0)).map_err(|e|e.to_string())?;
        if !confirmed {
            return Err("Override requires explicit confirmation".into());
        }
    }
    Ok(())
}

fn validate_activity(tx: &Transaction<'_>, session: &str, record: &Record) -> Result<(), String> {
    let directory: String = tx
        .query_row(
            "SELECT directory FROM session WHERE id=?1",
            [session],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if directory != record.directory {
        return Err("Override checkout changed".into());
    }
    let data: String = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1",
            params![record.activity_id, super::activity::CHANGED],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let activity: super::activity::Record =
        serde_json::from_str(&data).map_err(|e| e.to_string())?;
    if activity.session_id != session || activity.status != "pending" {
        return Err("Override activity is no longer owned".into());
    }
    if record.status == "consumed" && activity.phase != "launching" {
        return Err("Override native admission has not started".into());
    }
    let bindings = activity
        .admission_bindings
        .ok_or("Override activity has no authority")?;
    if !super::admission_authority::bindings_current(tx, &bindings).map_err(|e| e.to_string())? {
        return Err("Override admission authority is stale".into());
    }
    Ok(())
}

impl Runtime {
    /// Start an owned replay and return before its interactive confirmation.
    pub async fn approve_auto(
        &self,
        session_id: &str,
    ) -> Result<AutoOverrideReceipt, RuntimeError> {
        let admission = self.inner.open().await?;
        let handle = self.inner.handle(session_id).await?;
        let session = session_id.to_owned();
        let selected = self.inner.store.read(move |db| {
            Ok(db.query_row("SELECT id,data FROM event WHERE aggregate_id=?1 AND type=?2
                AND json_extract(data,'$.decision')='block' AND json_extract(data,'$.model') IS NOT NULL
                ORDER BY seq DESC LIMIT 1", params![session,AUTO_DECIDED], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?)
        })?.ok_or_else(||RuntimeError::Invalid("No classifier-blocked call to approve".into()))?;
        let decision: AutoDecision =
            serde_json::from_str(&selected.1).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
        let state = handle.state.lock().await;
        state.ensure_worktree_ready()?;
        let call =
            state.calls.get(&decision.call_id).cloned().ok_or_else(|| {
                RuntimeError::Invalid("Blocked call is no longer available".into())
            })?;
        if call.status != CallStatus::Error || call.input.is_none() {
            return Err(RuntimeError::Invalid(
                "Blocked call has no settled replayable input".into(),
            ));
        }
        if state
            .calls
            .values()
            .any(|c| c.status == CallStatus::OutcomeUnknown)
        {
            return Err(RuntimeError::Invalid(
                "Resolve unfinished calls before approval replay".into(),
            ));
        }
        let turn = super::drain::turn_context_for(&state, call.name == "apply_patch");
        if !self
            .inner
            .tools
            .definitions(&turn)
            .iter()
            .any(|def| def.spec.name == call.name)
        {
            return Err(RuntimeError::Invalid(
                "Blocked tool is no longer available".into(),
            ));
        }
        let directory = state.info.directory.clone();
        let root = cyber_core::config::project_root(std::path::Path::new(&directory));
        let root = std::fs::canonicalize(&root)
            .unwrap_or(root)
            .display()
            .to_string();
        if decision.checkout_root.as_deref() != Some(&root) {
            return Err(RuntimeError::Invalid(
                "Blocked call belongs to a different or unrecorded checkout".into(),
            ));
        }
        drop(state);
        let scope = super::activity::Scope::reserve(&self.inner, &handle).await?;
        let record = Record {
            receipt: AutoOverrideReceipt {
                id: cyber_core::ids::new_id("ovr"),
                decision_id: selected.0,
                original_call_id: call.call_id.clone(),
                call_id: cyber_core::ids::new_id("call"),
            },
            directory,
            activity_id: scope.id().into(),
            status: "pending".into(),
        };
        self.inner.commit(&handle, vec![change(&record)]).await?;
        let receipt = record.receipt.clone();
        let inner = self.inner.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = inner
                .replay_auto(&handle, record, decision, call, scope)
                .await
            {
                inner.bus.publish(LiveEvent::Error {
                    session_id: handle.state.lock().await.info.id.clone(),
                    kind: "auto_override".into(),
                    message: error.to_string(),
                });
            }
        });
        self.inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(task);
        drop(admission);
        Ok(receipt)
    }
}

impl Inner {
    async fn replay_auto(
        &self,
        handle: &Handle,
        mut record: Record,
        decision: AutoDecision,
        call: CallState,
        mut scope: super::activity::Scope,
    ) -> Result<(), RuntimeError> {
        let session_id = handle.state.lock().await.info.id.clone();
        let message_id = cyber_core::ids::new_id("msg");
        let asker = Asker::new(
            &self.me.upgrade().ok_or(RuntimeError::ShuttingDown)?,
            &session_id,
            &record.receipt.call_id,
            &message_id,
            "user",
        );
        let ask = PermissionAsk {
            action: "auto_override".into(),
            resources: decision.resources,
            always_patterns: vec![],
            metadata: json!({"requires_confirmation":true,"tool":call.name,"input":call.input,"classifier_reason":decision.reason,"original_call_id":call.call_id,"directory":record.directory}),
        };
        let cancel = scope.control.stop.clone();
        let reply = scope
            .run(async {
                tokio::select! {
                    _ = cancel.cancelled() => PermissionReply::Unattended,
                    reply = asker.permission(ask) => reply,
                }
            })
            .await;
        self.abandon_call_requests(&session_id, &record.receipt.call_id)
            .await?;
        if !matches!(reply, PermissionReply::Once | PermissionReply::Always)
            || cancel.is_cancelled()
        {
            record.status = "cancelled".into();
            self.commit(handle, vec![change(&record)]).await?;
            scope.settle()?;
            return Ok(());
        }
        let runtime = Runtime {
            inner: self.me.upgrade().ok_or(RuntimeError::ShuttingDown)?,
        };
        let ready = scope.run(async { tokio::select! {
            _ = cancel.cancelled() => Err(RuntimeError::Invalid("Approval replay cancelled".into())),
            _ = runtime.wait_idle(&session_id) => Ok(()),
        }}).await;
        let result = match ready {
            Err(error) => Err(error),
            Ok(()) => {
                self.with_location(handle, cancel.child_token(), async {
                    self.ensure_idle(&session_id)?;
                    scope.launch()?;
                    scope
                        .run(self.execute_auto_replay(
                            handle,
                            &mut record,
                            &call,
                            &message_id,
                            &scope,
                        ))
                        .await
                })
                .await
            }
        };
        if record.status == "pending" {
            record.status = "cancelled".into();
            self.commit(handle, vec![change(&record)]).await?;
        }
        if !handle
            .location_uncertain
            .load(std::sync::atomic::Ordering::SeqCst)
            && !handle.state.lock().await.calls.values().any(|c| {
                matches!(
                    c.status,
                    CallStatus::Dispatched | CallStatus::OutcomeUnknown
                )
            })
        {
            scope.settle()?;
        }
        result
    }

    async fn execute_auto_replay(
        &self,
        handle: &Handle,
        record: &mut Record,
        call: &CallState,
        message_id: &str,
        scope: &super::activity::Scope,
    ) -> Result<(), RuntimeError> {
        self.commit_staged_revert(handle).await?;
        let mut state = handle.state.lock().await;
        if state
            .calls
            .values()
            .any(|c| !c.status.is_settled() || c.status == CallStatus::OutcomeUnknown)
        {
            return Err(RuntimeError::Invalid(
                "Resolve unfinished calls before approval replay".into(),
            ));
        }
        if state.calls.get(&call.call_id).is_none_or(|current| {
            current.status != CallStatus::Error
                || current.name != call.name
                || current.input != call.input
        }) {
            return Err(RuntimeError::Invalid(
                "Blocked call is no longer available".into(),
            ));
        }
        // Reuse the stored edit tool instead of changing it to match a model preference.
        let turn = super::drain::turn_context_for(&state, call.name == "apply_patch");
        let def = self
            .tools
            .definitions(&turn)
            .into_iter()
            .find(|d| d.spec.name == call.name)
            .ok_or_else(|| RuntimeError::Invalid("Blocked tool is no longer available".into()))?;
        let input = call
            .input
            .clone()
            .ok_or_else(|| RuntimeError::Invalid("Blocked input is unavailable".into()))?;
        let replay = cyber_llm::ToolCall {
            id: record.receipt.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            input: Some(input.clone()),
        };
        let mut consumed = record.clone();
        consumed.status = "consumed".into();
        self.commit_locked(
            &mut state,
            vec![
                change(&consumed),
                event(
                    TOOL_CALLED,
                    &ToolCalled {
                        message_id: message_id.into(),
                        call_id: replay.id.clone(),
                        name: replay.name.clone(),
                        arguments: replay.arguments.clone(),
                        input: Some(input.clone()),
                        retry_safety: def.retry_safety,
                    },
                ),
            ],
        )?;
        record.status = "consumed".into();
        drop(state);
        let mut invocation = self
            .dispatch(handle, &turn, message_id, &replay, &def, input.clone())
            .await?;
        let invocation_done = CancellationToken::new();
        invocation.asker.auto_override = Some(Grant {
            session_id: turn.session_id,
            call_id: replay.id.clone(),
            tool: replay.name.clone(),
            input,
            authority: scope.authority(),
            cancel: scope.control.stop.clone(),
            done: invocation_done.clone(),
        });
        let (_, def, outcome) = self
            .run_tool(handle, def, invocation, scope.control.stop.clone())
            .await;
        invocation_done.cancel();
        let mut state = handle.state.lock().await;
        let mut seen = state
            .epoch
            .as_ref()
            .map(|epoch| epoch.reminded_skills.clone())
            .unwrap_or_default();
        let settled = super::drain::settlement_with_skills(
            &replay.id,
            &def,
            outcome,
            &mut seen,
            |output, reminder| {
                self.tools
                    .finalize_skill_output(&state.info.directory, output, reminder)
            },
        );
        let output = settled.data["output"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        record.status = "settled".into();
        self.commit_locked(&mut state, vec![settled, change(record)])?;
        drop(state);
        if scope.control.stop.is_cancelled()
            || self.ensure_admission_open(&scope.control.source).is_err()
        {
            return Ok(());
        }
        self.commit(
            handle,
            vec![
                event(
                    ADMITTED,
                    &Admitted {
                        admission_bindings: None,
                        wake: false,
                        message_id: message_id.into(),
                        digest: format!("auto-override:{}", record.receipt.id),
                        parts: vec![cyber_llm::Content::Text {
                            text: format!(
                                "/approve replayed {} {}\n{output}",
                                replay.name, replay.arguments
                            ),
                        }],
                        delivery: Delivery::Queue,
                        source: "auto_override".into(),
                    },
                ),
                event(
                    PROMOTED,
                    &Promoted {
                        message_id: message_id.into(),
                    },
                ),
            ],
        )
        .await?;
        Ok(())
    }
}
