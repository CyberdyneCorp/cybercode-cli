//! Client hook handlers (`client-sdk` → Hook callbacks): a durable `client`-scope receipt,
//! then the registering client answers `hook/execute` over its JSON-RPC channel.
use std::time::Duration;

use cyber_core::hooks::{HookAction, HookDecision, HookDefinition, HookEvent};
use cyber_server::runtime::{ClientHooks, HookExecutionResult, Runtime};
use tokio_util::sync::CancellationToken;

use crate::hook_commands::{HookCommandReport, HookOutcome};
use crate::hook_reports::{failure, skipped};

/// Admit, run and settle one client handler. A failure or timeout never blocks: the
/// handler is the client's own code, outside every configured policy.
pub(crate) async fn run_recorded(
    hooks: &dyn ClientHooks,
    runtime: &Runtime,
    definition: &HookDefinition,
    event: &HookEvent,
    cancel: CancellationToken,
) -> Result<HookCommandReport, String> {
    let registration = definition
        .source
        .strip_prefix("client:")
        .ok_or("client hook registration unavailable")?;
    if cancel.is_cancelled() {
        return Err("client hook cancelled before admission".into());
    }
    let Some(mut owner) = runtime
        .try_start_hook_execution(event, definition, false)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(skipped(false));
    };
    owner
        .mark_launch(runtime)
        .map_err(|error| error.to_string())?;
    let stop = owner.cancellation();
    let timeout = Duration::from_secs(definition.handler.timeout.into());
    let answer = tokio::select! {
        biased;
        _ = cancel.cancelled() => { stop.cancel(); None }
        answer = hooks.execute_client_hook(registration, event.as_json().clone(), timeout, stop.clone()) => Some(answer),
    };
    let mut report = match answer {
        None => skipped(true),
        Some(answer) => report(event, definition, answer),
    };
    report.must_stop |= owner.verify(runtime).is_err();
    owner
        .finish(HookExecutionResult {
            outcome: report.outcome,
            decision: report.decision.clone(),
            acknowledged: report.acknowledged,
            must_stop: report.must_stop,
            io: None,
        })
        .map_err(|error| error.to_string())?;
    Ok(report)
}

/// The client's answer as a hook decision; a refused or malformed answer is an error.
fn report(
    event: &HookEvent,
    definition: &HookDefinition,
    answer: Result<serde_json::Value, String>,
) -> HookCommandReport {
    let parsed = answer.and_then(|value| HookDecision::parse(event.event(), &value));
    match parsed {
        Ok(parsed) => HookCommandReport {
            outcome: if matches!(
                parsed.decision.decision,
                Some(HookAction::Deny | HookAction::Block)
            ) {
                HookOutcome::Blocked
            } else {
                HookOutcome::Ok
            },
            decision: parsed.decision,
            ignored_fields: parsed.ignored_fields,
            diagnostic: None,
            acknowledged: true,
            must_stop: false,
        },
        Err(error) => {
            let outcome = if error.ends_with("timed out") {
                HookOutcome::Timeout
            } else {
                HookOutcome::Error
            };
            failure(event, Some(definition), false, outcome, error, true)
        }
    }
}
