//! Non-tool lifecycle hook events owned by the built-in host. Dispatch reuses the
//! tool-hook path: trust checks, once claims, durable receipts and fail_closed.
use std::sync::Weak;

use cyber_core::hooks::{
    HookAction, HookDecision, HookDefinition, HookEvent, HookIdentity, HookLocation, HookOutcome,
};
use cyber_server::runtime::{LifecycleHook, LifecycleVerdict};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::HookCommandReport;
use crate::tool_hooks::lifecycle_event;
use crate::tools::ToolError;

/// Events whose synthesized deny/block must not read as a decision: observational
/// events ignore decisions, and a Stop block means "continue". An unavailable
/// fail_closed handler still refuses the owner's next effect; any other one, even
/// when its runner forced the failure closed (trust or launch re-authorization),
/// only leaves a notice.
const DISTINCT_UNAVAILABLE: &[&str] = &[
    "SubagentStop",
    "Stop",
    "StopFailure",
    "SessionStart",
    "JobEnded",
];

/// Lifecycle events the runtime dispatches through its narrow host port.
const RUNTIME_EVENTS: &[&str] = &[
    "Stop",
    "StopFailure",
    "UserPromptSubmit",
    "SessionStart",
    "JobEnded",
];

/// Events whose additional_context the runtime admits as a system message.
const CONTEXT_EVENTS: &[&str] = &["UserPromptSubmit", "SessionStart"];

pub(crate) fn admits_context(event: &HookEvent) -> bool {
    CONTEXT_EVENTS.contains(&event.event())
}

/// Prepare a fixed-input report for merging. A synthesized unavailable decision on a
/// DISTINCT_UNAVAILABLE event is dropped (fail_closed handlers were already refused
/// by `observed_unavailable`), and admitted context is tagged with its hook id
/// before concatenation erases the source.
pub(crate) fn mergeable_report(
    event: &HookEvent,
    definition: &HookDefinition,
    mut report: HookCommandReport,
) -> HookCommandReport {
    if synthesized_unavailable(event, &report) {
        report.decision = HookDecision::default();
    }
    if admits_context(event)
        && let Some(context) = report.decision.additional_context.take()
    {
        let id = definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest);
        report.decision.additional_context = Some(format!("[hook {id}] {context}"));
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
        if self.hook_config.get().is_none() {
            return Ok(LifecycleVerdict::default());
        }
        let identity = HookIdentity {
            session_id: hook.session_id,
            location: HookLocation {
                directory: hook.directory.into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: hook.agent,
            mode: hook.mode,
        };
        let event = lifecycle_event(&hook.kind, identity, hook.fields).map_err(failure)?;
        let Some((resolved, catalog)) = self.hook_catalog(&event).map_err(failure)? else {
            return Ok(LifecycleVerdict::default());
        };
        let max_stop_continuations = catalog.settings.max_stop_continuations;
        let decision =
            Box::pin(self.dispatch_catalog(event, &hook.subject, &resolved, catalog, cancel))
                .await
                .map_err(failure)?;
        Ok(LifecycleVerdict {
            decision,
            max_stop_continuations,
        })
    }

    /// SubagentStart runs before any child effect. Deny refuses the launch even in
    /// bypass Mode; allow and ask never grant or prompt for delegation.
    pub(crate) async fn subagent_start_hooks(
        &self,
        identity: HookIdentity,
        agent: &str,
        fields: Map<String, Value>,
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        if self.hook_config.get().is_none() {
            return Ok(());
        }
        let event = lifecycle_event("SubagentStart", identity, fields)?;
        let decision = self.dispatch_tool_hooks(event, agent, cancel).await?;
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

/// Captured identity for the observational SubagentStop event at child handback.
pub(crate) struct SubagentStop {
    pub(crate) host: Weak<BuiltinHost>,
    pub(crate) identity: HookIdentity,
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
        let event = lifecycle_event(
            "SubagentStop",
            self.identity.clone(),
            fields.as_object().cloned().unwrap_or_default(),
        )?;
        match host
            .dispatch_tool_hooks(event, &self.agent_type, cancel)
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
