//! Interpret captured command results without persisting raw IO.

use cyber_core::hooks::{HookAction, HookDecision, HookEvent};
use serde_json::Value;

use super::{HookCommandCapture, HookCommandEnd, HookCommandError};

pub use cyber_core::hooks::HookOutcome;

pub struct HookCommandReport {
    pub outcome: HookOutcome,
    pub decision: HookDecision,
    pub ignored_fields: Vec<String>,
    /// Transient user diagnostic; raw stderr must not enter receipts by default.
    pub diagnostic: Option<String>,
    /// The dispatcher must stop admission on cancellation or unknown termination.
    pub must_stop: bool,
    pub acknowledged: bool,
}

pub fn interpret_hook_command(
    event: &HookEvent,
    hook_id: &str,
    fail_closed: bool,
    capture: Result<HookCommandCapture, HookCommandError>,
) -> HookCommandReport {
    let capture = match capture {
        Ok(capture) => capture,
        Err(error) => {
            return unavailable(
                event,
                hook_id,
                fail_closed,
                HookOutcome::Error,
                error.message,
                error.acknowledged,
            );
        }
    };
    match capture.end {
        HookCommandEnd::Exited(Some(0)) => success(event, hook_id, fail_closed, &capture),
        HookCommandEnd::Exited(Some(2)) => {
            let mut reason = String::from_utf8_lossy(&capture.stderr.bytes)
                .trim()
                .to_string();
            if reason.is_empty() {
                reason = format!("hook {hook_id} blocked");
            }
            if capture.stderr.truncated {
                reason.push_str("\n[hook stderr truncated]");
            }
            let mut report = empty(HookOutcome::Blocked, true);
            report.decision = block(event, reason);
            report
        }
        HookCommandEnd::Exited(code) => {
            // The command contract makes other exit codes non-blocking errors.
            let mut report = empty(HookOutcome::Error, true);
            report.diagnostic = Some(format!(
                "hook {hook_id} exited with {code:?}: {}",
                String::from_utf8_lossy(&capture.stderr.bytes).trim()
            ));
            report
        }
        HookCommandEnd::TimedOut { acknowledged } => unavailable(
            event,
            hook_id,
            fail_closed,
            HookOutcome::Timeout,
            format!("hook {hook_id} timed out"),
            acknowledged,
        ),
        HookCommandEnd::Cancelled { acknowledged } => {
            let mut report = if acknowledged {
                empty(HookOutcome::Skipped, true)
            } else {
                unavailable(
                    event,
                    hook_id,
                    true,
                    HookOutcome::Error,
                    "hook cancellation was not acknowledged".into(),
                    false,
                )
            };
            report.must_stop = true;
            report
        }
    }
}

fn success(
    event: &HookEvent,
    hook_id: &str,
    fail_closed: bool,
    capture: &HookCommandCapture,
) -> HookCommandReport {
    if capture.stdout.truncated {
        return unavailable(
            event,
            hook_id,
            fail_closed,
            HookOutcome::Error,
            "hook stdout exceeded the capture limit".into(),
            true,
        );
    }
    let value = match serde_json::from_slice::<Value>(&capture.stdout.bytes) {
        Ok(value) if value.is_object() => value,
        Err(error)
            if capture
                .stdout
                .bytes
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(&b'{') =>
        {
            return unavailable(
                event,
                hook_id,
                fail_closed,
                HookOutcome::Error,
                format!("invalid hook JSON: {error}"),
                true,
            );
        }
        _ => return empty(HookOutcome::Ok, true),
    };
    match HookDecision::parse(event.event(), &value) {
        Ok(parsed) => {
            let outcome = if matches!(
                parsed.decision.decision,
                Some(HookAction::Deny | HookAction::Block)
            ) {
                HookOutcome::Blocked
            } else {
                HookOutcome::Ok
            };
            HookCommandReport {
                outcome,
                decision: parsed.decision,
                ignored_fields: parsed.ignored_fields,
                diagnostic: None,
                must_stop: false,
                acknowledged: true,
            }
        }
        Err(error) => unavailable(event, hook_id, fail_closed, HookOutcome::Error, error, true),
    }
}

fn block(event: &HookEvent, reason: String) -> HookDecision {
    HookDecision {
        decision: Some(if event.event() == "Stop" {
            HookAction::Block
        } else {
            HookAction::Deny
        }),
        reason: Some(reason),
        ..Default::default()
    }
}

fn unavailable(
    event: &HookEvent,
    hook_id: &str,
    fail_closed: bool,
    outcome: HookOutcome,
    diagnostic: String,
    acknowledged: bool,
) -> HookCommandReport {
    let mut report = empty(outcome, acknowledged);
    report.diagnostic = Some(diagnostic);
    report.must_stop = !acknowledged;
    if fail_closed || !acknowledged {
        report.decision = block(event, format!("hook {hook_id} unavailable"));
    }
    report
}

fn empty(outcome: HookOutcome, acknowledged: bool) -> HookCommandReport {
    HookCommandReport {
        outcome,
        decision: HookDecision::default(),
        ignored_fields: Vec::new(),
        diagnostic: None,
        must_stop: false,
        acknowledged,
    }
}
