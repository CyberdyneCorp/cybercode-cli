//! Command exit and decision semantics; raw diagnostics remain separate from receipts.
use cyber_core::hooks::{HookAction, HookEvent, HookIdentity, HookLocation};
use cyber_tools::hook_commands::{
    HookCommandCapture, HookCommandEnd, HookCommandError, HookOutcome, HookStreamCapture,
    interpret_hook_command,
};

fn event(kind: &str, directory: &std::path::Path) -> HookEvent {
    HookEvent::new(
        kind,
        HookIdentity {
            session_id: "ses_hook".into(),
            location: HookLocation {
                directory: directory.into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "coder".into(),
            mode: "default".into(),
        },
        1,
        serde_json::Map::new(),
    )
    .unwrap()
}
fn capture(end: HookCommandEnd, stdout: &[u8], stderr: &[u8]) -> HookCommandCapture {
    HookCommandCapture {
        end,
        stdout: HookStreamCapture {
            bytes: stdout.into(),
            truncated: false,
        },
        stderr: HookStreamCapture {
            bytes: stderr.into(),
            truncated: false,
        },
    }
}

#[test]
fn successful_json_decisions_and_plain_output_follow_command_contract() {
    let directory = tempfile::tempdir().unwrap();
    let event = event("PreToolUse", directory.path());
    let report = interpret_hook_command(
        &event,
        "guard",
        false,
        Ok(capture(
            HookCommandEnd::Exited(Some(0)),
            br#"{"decision":"deny","reason":"policy","updated_input":{"command":"safe"}}"#,
            b"",
        )),
    );
    assert_eq!(report.outcome, HookOutcome::Blocked);
    assert_eq!(report.decision.decision, Some(HookAction::Deny));
    assert_eq!(report.decision.reason.as_deref(), Some("policy"));
    assert!(report.decision.updated_input.is_some());
    assert!(!report.must_stop);
    for output in [b"notification".as_slice(), b"[1,2]", b""] {
        let report = interpret_hook_command(
            &event,
            "guard",
            false,
            Ok(capture(HookCommandEnd::Exited(Some(0)), output, b"")),
        );
        assert_eq!(report.outcome, HookOutcome::Ok);
        assert!(report.decision.decision.is_none());
    }
}

#[test]
fn exit_two_blocks_and_other_codes_report_nonblocking_stderr_errors() {
    let directory = tempfile::tempdir().unwrap();
    for kind in ["PreToolUse", "Stop"] {
        let event = event(kind, directory.path());
        let report = interpret_hook_command(
            &event,
            "guard",
            false,
            Ok(capture(
                HookCommandEnd::Exited(Some(2)),
                b"",
                b" forbidden ",
            )),
        );
        assert_eq!(report.outcome, HookOutcome::Blocked);
        assert_eq!(report.decision.reason.as_deref(), Some("forbidden"));
        assert_eq!(
            report.decision.decision,
            Some(if kind == "Stop" {
                HookAction::Block
            } else {
                HookAction::Deny
            })
        );
    }
    let event = event("PreToolUse", directory.path());
    let report = interpret_hook_command(
        &event,
        "guard",
        true,
        Ok(capture(
            HookCommandEnd::Exited(Some(1)),
            b"",
            b"script failed",
        )),
    );
    assert_eq!(report.outcome, HookOutcome::Error);
    assert!(report.decision.decision.is_none());
    assert!(report.diagnostic.unwrap().contains("script failed"));
}

#[test]
fn truncated_or_malformed_decisions_cannot_produce_allow() {
    let directory = tempfile::tempdir().unwrap();
    let event = event("PreToolUse", directory.path());
    let mut truncated = capture(
        HookCommandEnd::Exited(Some(0)),
        br#"{"decision":"allow"}"#,
        b"",
    );
    truncated.stdout.truncated = true;
    let report = interpret_hook_command(&event, "guard", true, Ok(truncated));
    assert_eq!(report.outcome, HookOutcome::Error);
    assert_eq!(report.decision.decision, Some(HookAction::Deny));
    for output in [b"{bad".as_slice(), br#"{"decision":42}"#] {
        let report = interpret_hook_command(
            &event,
            "guard",
            true,
            Ok(capture(HookCommandEnd::Exited(Some(0)), output, b"")),
        );
        assert_eq!(report.outcome, HookOutcome::Error);
        assert_eq!(report.decision.decision, Some(HookAction::Deny));
        assert_eq!(
            report.decision.reason.as_deref(),
            Some("hook guard unavailable")
        );
    }
}

#[test]
fn timeout_policy_and_unknown_termination_remain_distinct() {
    let directory = tempfile::tempdir().unwrap();
    let event = event("PreToolUse", directory.path());
    for fail_closed in [false, true] {
        let report = interpret_hook_command(
            &event,
            "guard",
            fail_closed,
            Ok(capture(
                HookCommandEnd::TimedOut { acknowledged: true },
                b"",
                b"",
            )),
        );
        assert_eq!(report.outcome, HookOutcome::Timeout);
        assert_eq!(
            report.decision.decision,
            fail_closed.then_some(HookAction::Deny)
        );
        assert!(!report.must_stop);
    }
    let unknown = interpret_hook_command(
        &event,
        "guard",
        false,
        Err(HookCommandError {
            message: "lost owner".into(),
            acknowledged: false,
        }),
    );
    assert_eq!(unknown.outcome, HookOutcome::Error);
    assert!(unknown.must_stop && !unknown.acknowledged);
    assert_eq!(unknown.decision.decision, Some(HookAction::Deny));
    let cancelled = interpret_hook_command(
        &event,
        "guard",
        true,
        Ok(capture(
            HookCommandEnd::Cancelled { acknowledged: true },
            b"",
            b"",
        )),
    );
    assert_eq!(cancelled.outcome, HookOutcome::Skipped);
    assert!(cancelled.must_stop);
    assert!(cancelled.decision.decision.is_none());
}
