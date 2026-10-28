//! Non-tool lifecycle hook events owned by the built-in host. Dispatch reuses the
//! tool-hook path: trust checks, once claims, durable receipts and fail_closed.
use std::sync::Weak;

use cyber_core::hooks::{
    HookAction, HookCatalog, HookDecision, HookDefinition, HookEvent, HookOutcome,
};
use cyber_server::runtime::{LifecycleHook, LifecycleVerdict};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::HookCommandReport;
use crate::tool_hooks::HookOwner;
use crate::tools::ToolError;

/// Events whose synthesized deny/block must not read as a decision: observational
/// events (Post* included) ignore decisions, and a Stop block means "continue". An
/// unavailable fail_closed handler still refuses the owner's next effect; any other
/// one, even when its runner forced the failure closed (trust or launch
/// re-authorization), only leaves a notice.
const DISTINCT_UNAVAILABLE: &[&str] = &[
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "Notification",
    "ElicitationResult",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "SessionStart",
    "SessionEnd",
    "JobEnded",
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "Interrupt",
    "PostModelSwitch",
    "PreCompact",
    "PostCompact",
    "InstructionsLoaded",
    "WorkflowRunStart",
    "WorkflowRunEnd",
    "WorktreeRemove",
    "Setup",
    "ConfigChange",
    "CwdChanged",
    "FileChanged",
    "GoalEvaluated",
    "GoalCompleted",
];

/// Lifecycle events the runtime dispatches through its narrow host port.
const RUNTIME_EVENTS: &[&str] = &[
    "GoalEvaluated",
    "GoalCompleted",
    "Stop",
    "StopFailure",
    "UserPromptSubmit",
    "SessionStart",
    "SessionEnd",
    "JobEnded",
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "Interrupt",
    "PreModelSwitch",
    "PostModelSwitch",
    "PreCompact",
    "PostCompact",
    "PostToolBatch",
    "InstructionsLoaded",
    "WorkflowRunStart",
    "WorkflowRunEnd",
    "Setup",
    "ConfigChange",
    "CwdChanged",
    "FileChanged",
];

/// Events whose additional_context the runtime applies: lifecycle context with its
/// prompt or first Turn, tool-event and PostToolBatch context with the Turn's
/// settlement, and PreCompact context in the summary instructions.
const CONTEXT_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "SessionStart",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PreCompact",
];

/// Events whose `continue: false` the runtime applies by ending the Drain after the
/// current Turn (Stop: instead of any continuation).
const CONTINUE_EVENTS: &[&str] = &[
    "Stop",
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
];

fn admits_context(event: &HookEvent) -> bool {
    CONTEXT_EVENTS.contains(&event.event())
}

/// A decision carries context or a stop request this event cannot apply yet; the
/// dispatcher notices it and a fail_closed handler refuses rather than drop it.
pub(crate) fn unapplied_effect(event: &HookEvent, decision: &HookDecision) -> bool {
    (decision.additional_context.is_some() && !admits_context(event))
        || (decision.continuation == Some(false) && !CONTINUE_EVENTS.contains(&event.event()))
}

/// Prepare a fixed-input report for merging. A synthesized unavailable decision on a
/// DISTINCT_UNAVAILABLE event is dropped (fail_closed handlers were already refused
/// by `observed_unavailable`), and admitted context and a stop request's reason are
/// tagged with their hook id before concatenation erases the source.
pub(crate) fn mergeable_report(
    event: &HookEvent,
    definition: &HookDefinition,
    mut report: HookCommandReport,
) -> HookCommandReport {
    if synthesized_unavailable(event, &report) {
        report.decision = HookDecision::default();
    }
    let id = definition
        .handler
        .id
        .as_deref()
        .unwrap_or(&definition.digest);
    if admits_context(event)
        && let Some(context) = report.decision.additional_context.take()
    {
        report.decision.additional_context = Some(format!("[hook {id}] {context}"));
    }
    if report.decision.continuation == Some(false) {
        let reason = report.decision.stop_reason.take();
        let reason = reason.as_deref().unwrap_or("continue: false");
        report.decision.stop_reason = Some(format!("[hook {id}] {reason}"));
    }
    report
}

/// A fail_closed handler reported unavailable (as opposed to an explicit decision).
pub(crate) fn observed_unavailable(
    event: &HookEvent,
    definition: &HookDefinition,
    report: &HookCommandReport,
) -> bool {
    definition.handler.fail_closed && synthesized_unavailable(event, report)
}

/// A deny/block the runner synthesized for a failure: only a parsed decision is
/// `Blocked`, and unacknowledged runs already stop through `must_stop`.
fn synthesized_unavailable(event: &HookEvent, report: &HookCommandReport) -> bool {
    DISTINCT_UNAVAILABLE.contains(&event.event())
        && report.outcome != HookOutcome::Blocked
        && matches!(
            report.decision.decision,
            Some(HookAction::Deny | HookAction::Block)
        )
}

impl BuiltinHost {
    /// Runtime-owned lifecycle events with the same trust, receipts and fail_closed
    /// handling as tool hooks. The runtime alone applies the merged decision.
    pub(crate) async fn runtime_lifecycle_hooks(
        &self,
        hook: LifecycleHook,
        cancel: CancellationToken,
    ) -> Result<LifecycleVerdict, String> {
        if !RUNTIME_EVENTS.contains(&hook.kind.as_str()) {
            return Err(format!("{} is not a runtime lifecycle hook", hook.kind));
        }
        let owner = HookOwner {
            session_id: hook.session_id,
            directory: hook.directory,
            agent: hook.agent,
            mode: hook.mode,
            workflow_run_id: hook.workflow_run_id,
        };
        let dispatched = self
            .dispatch_owned(&hook.kind, &owner, hook.fields, &hook.subject, cancel)
            .await
            .map_err(failure)?;
        Ok(
            dispatched.map_or_else(LifecycleVerdict::default, |(decision, cap)| {
                LifecycleVerdict {
                    decision,
                    max_stop_continuations: cap,
                    configured: true,
                }
            }),
        )
    }

    /// SubagentStart runs before any child effect. Deny refuses the launch even in
    /// bypass Mode; allow and ask never grant or prompt for delegation.
    pub(crate) async fn subagent_start_hooks(
        &self,
        owner: &HookOwner,
        agent: &str,
        fields: Map<String, Value>,
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        let decision = self
            .dispatch_owned("SubagentStart", owner, fields, agent, cancel)
            .await?
            .map(|(decision, _)| decision)
            .unwrap_or_default();
        match decision.decision {
            Some(HookAction::Deny | HookAction::Block) => Err(ToolError::Failed(format!(
                "blocked by hook {}",
                decision.reason.as_deref().unwrap_or("policy")
            ))),
            Some(action) => {
                cyber_core::log::debug(
                    "hooks",
                    "SubagentStart decision ignored",
                    json!({"decision":action}),
                );
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl BuiltinHost {
    /// WorktreeCreate runs after worktree permission and before any git effect. A
    /// deny, or an unavailable `fail_closed` handler, refuses the creation; allow
    /// and ask never grant it.
    pub(crate) async fn worktree_create_hooks(
        &self,
        inv: &cyber_server::runtime::Invocation,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        let fields = json!({"name": name});
        let decision = self
            .dispatch_owned(
                "WorktreeCreate",
                &HookOwner::of(inv),
                fields.as_object().cloned().unwrap_or_default(),
                name,
                cancel,
            )
            .await?
            .map(|(decision, _)| decision)
            .unwrap_or_default();
        match decision.decision {
            Some(HookAction::Deny | HookAction::Block) => Err(ToolError::Failed(format!(
                "blocked by hook {}",
                decision.reason.as_deref().unwrap_or("policy")
            ))),
            _ => Ok(()),
        }
    }

    /// Observational WorktreeRemove after a managed checkout and its branch are gone.
    /// Decisions are ignored and an unavailable handler is only logged: the removal
    /// already happened and its report stays accurate. The event runs as `inv`'s
    /// Session at its current Location, which hook admission requires; a Session whose
    /// Location was the removed checkout has none left to run it in.
    pub(crate) async fn worktree_remove_hooks(
        &self,
        inv: &cyber_server::runtime::Invocation,
        managed: &cyber_core::worktrees::Managed,
        cancel: CancellationToken,
    ) {
        let mut inv = inv.clone();
        if let Some(runtime) = self.runtime()
            && let Ok(state) = runtime.state(&inv.session_id).await
        {
            inv.directory = state.info.directory;
        }
        if !std::path::Path::new(&inv.directory).is_dir() {
            cyber_core::log::debug(
                "hooks",
                "WorktreeRemove skipped: the Session's Location was the removed worktree",
                json!({"session_id": inv.session_id, "name": managed.name}),
            );
            return;
        }
        let inv = &inv;
        let fields = json!({
            "id": managed.id,
            "name": managed.name,
            "path": managed.path,
            "branch": managed.branch,
        });
        let fields = fields.as_object().cloned().unwrap_or_default();
        let owner = HookOwner::of(inv);
        if let Err(error) = self
            .dispatch_owned("WorktreeRemove", &owner, fields, &managed.name, cancel)
            .await
        {
            cyber_core::log::warn(
                "hooks",
                "WorktreeRemove hook unavailable",
                json!({"session_id": inv.session_id, "error": failure(error)}),
            );
        }
    }
}

/// Captured identity for the observational SubagentStop event at child handback.
pub(crate) struct SubagentStop {
    pub(crate) host: Weak<BuiltinHost>,
    pub(crate) owner: HookOwner,
    pub(crate) agent_type: String,
    pub(crate) child_session_id: String,
    pub(crate) subagent_name: Option<String>,
}

impl SubagentStop {
    /// Fire SubagentStop for a settled child; aborted children skip it like Post*.
    /// Decisions are ignored. Only a fail_closed unavailable handler fails the handback.
    pub(crate) async fn handback(
        &self,
        result: Result<String, ToolError>,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        let (status, error) = match &result {
            Ok(_) => ("completed", None),
            Err(ToolError::Failed(error)) => ("error", Some(error.clone())),
            Err(ToolError::Aborted) => return result,
        };
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| ToolError::Failed("Hook host unavailable at handback".into()))?;
        if host.hook_config.get().is_none() {
            return result;
        }
        let fields = json!({
            "child_session_id": self.child_session_id,
            "agent_type": self.agent_type,
            "subagent_name": self.subagent_name,
            "status": status,
            "error": error,
        });
        let fields = fields.as_object().cloned().unwrap_or_default();
        match host
            .dispatch_owned(
                "SubagentStop",
                &self.owner,
                fields,
                &self.agent_type,
                cancel,
            )
            .await
        {
            Ok(_) => result,
            Err(_) if result.is_err() => result,
            Err(ToolError::Aborted) => Err(ToolError::Aborted),
            Err(ToolError::Failed(error)) => Err(ToolError::Failed(format!(
                "Subagent handback failed: {error}"
            ))),
        }
    }
}

/// Any definition exists for this event, matched or not.
/// Whether `event` has definitions to run, or policy-skipped ones to report.
pub(crate) fn configured(catalog: &HookCatalog, event: &HookEvent) -> bool {
    catalog
        .definitions
        .iter()
        .chain(catalog.skipped_for(event.event()))
        .any(|definition| definition.event == event.event())
}

fn failure(error: ToolError) -> String {
    match error {
        ToolError::Failed(message) => message,
        ToolError::Aborted => "hook dispatch aborted".into(),
    }
}

#[cfg(test)]
mod tests {
    use cyber_core::config::{self, LoadRequest};
    use cyber_core::hooks::{HookAction, HookCatalog, HookEvent, HookLocation, HookOutcome};
    use cyber_core::paths::Paths;
    use serde_json::json;

    use super::{mergeable_report, observed_unavailable};
    use crate::hook_reports::failure;

    fn stop_definition(dir: &std::path::Path, fail_closed: bool) -> super::HookDefinition {
        let env = std::collections::HashMap::from([(
            "CYBER_HOME".into(),
            dir.join("config").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, dir);
        paths.ensure().unwrap();
        let hook = json!({"type":"http","url":"http://127.0.0.1:9/stop","id":"remote","fail_closed":fail_closed});
        std::fs::write(
            paths.config.join("cyber.jsonc"),
            json!({"hooks":{"Stop":[{"hooks":[hook]}]}}).to_string(),
        )
        .unwrap();
        let resolved = config::load(&LoadRequest {
            location: dir,
            paths: &paths,
            env: &env,
            home: dir,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        HookCatalog::from_config(&resolved)
            .unwrap()
            .definitions
            .remove(0)
    }

    fn stop_event(dir: &std::path::Path) -> HookEvent {
        let location = HookLocation {
            directory: dir.to_path_buf(),
            workspace: None,
        };
        let fields = json!({"stop_hook_active": false});
        HookEvent::synthetic(
            "Stop",
            location,
            "global".into(),
            "build".into(),
            "default".into(),
            1,
            fields,
        )
        .unwrap()
    }

    /// A runner that forces a failure closed (failed launch re-authorization) for a
    /// handler without fail_closed must neither continue the Drain nor fail it.
    #[test]
    fn forced_closed_stop_failure_is_a_notice_unless_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let event = stop_event(dir.path());
        for fail_closed in [false, true] {
            let definition = stop_definition(dir.path(), fail_closed);
            let report = failure(
                &event,
                Some(&definition),
                true,
                HookOutcome::Error,
                "hook handler digest is untrusted".into(),
                true,
            );
            assert_eq!(report.decision.decision, Some(HookAction::Block));
            assert_eq!(
                observed_unavailable(&event, &definition, &report),
                fail_closed
            );
            let merged = mergeable_report(&event, &definition, report);
            assert_eq!(merged.decision.decision, None, "fail_closed={fail_closed}");
            assert!(merged.diagnostic.is_some());
        }
    }

    #[test]
    fn a_parsed_stop_block_still_continues() {
        let dir = tempfile::tempdir().unwrap();
        let event = stop_event(dir.path());
        let definition = stop_definition(dir.path(), true);
        let mut report = failure(
            &event,
            Some(&definition),
            true,
            HookOutcome::Error,
            "x".into(),
            true,
        );
        report.outcome = HookOutcome::Blocked;
        assert!(!observed_unavailable(&event, &definition, &report));
        let merged = mergeable_report(&event, &definition, report);
        assert_eq!(merged.decision.decision, Some(HookAction::Block));
    }
}
