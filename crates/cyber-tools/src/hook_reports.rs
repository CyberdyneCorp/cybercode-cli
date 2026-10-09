//! Shared transport result policy and bounded opt-in hook IO.
use crate::hook_commands::{
    HookCommandError, HookCommandReport, HookOutcome, interpret_hook_command,
};
use cyber_core::config::Resolved;
use cyber_core::hooks::{HookDefinition, HookEvent};
const LIMIT: usize = 1024 * 1024;

pub(crate) fn log_io(resolved: &Resolved) -> bool {
    resolved.value["telemetry"]["log_hook_io"]
        .as_bool()
        .unwrap_or(false)
}

pub(crate) fn failure(
    event: &HookEvent,
    definition: Option<&HookDefinition>,
    fail_closed: bool,
    outcome: HookOutcome,
    message: String,
    acknowledged: bool,
) -> HookCommandReport {
    let id = definition
        .map(|definition| {
            definition
                .handler
                .id
                .as_deref()
                .unwrap_or(&definition.digest)
        })
        .unwrap_or("hook");
    let mut report = interpret_hook_command(
        event,
        id,
        fail_closed,
        Err(HookCommandError {
            message,
            acknowledged,
        }),
    );
    report.outcome = outcome;
    report
}

pub(crate) fn skipped(must_stop: bool) -> HookCommandReport {
    HookCommandReport {
        outcome: HookOutcome::Skipped,
        decision: Default::default(),
        ignored_fields: Vec::new(),
        diagnostic: None,
        acknowledged: true,
        must_stop,
    }
}

pub(crate) fn logged_io(event: &HookEvent, bytes: &[u8]) -> cyber_server::runtime::HookExecutionIo {
    let mut truncated = false;
    let mut bound = |mut value: String| {
        if value.len() > LIMIT {
            truncated = true;
            let mut end = LIMIT;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        value
    };
    cyber_server::runtime::HookExecutionIo {
        stdin: bound(format!("{}", event.as_json())),
        stdout: bound(String::from_utf8_lossy(bytes).into_owned()),
        stderr: String::new(),
        truncated,
    }
}
