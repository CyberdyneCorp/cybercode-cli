//! Bounded stdin/stdout/stderr transport for an already authorized owned hook process.
//! The caller must retain this future under its execution owner and persist settlement.

mod launch;
mod results;

pub use launch::HookCommandRunner;
pub use results::{HookCommandReport, HookOutcome, interpret_hook_command};

use std::io;
use std::time::Duration;

use cyber_core::hooks::HookEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::HookCommandProcess;
use crate::tools::process::{ReadStream, WriteStream};

const CAPTURE_LIMIT: usize = 1024 * 1024;
const STOP_ACK: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookCommandEnd {
    Exited(Option<i32>),
    TimedOut { acknowledged: bool },
    Cancelled { acknowledged: bool },
}

pub struct HookStreamCapture {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

pub struct HookCommandCapture {
    pub end: HookCommandEnd,
    pub stdout: HookStreamCapture,
    pub stderr: HookStreamCapture,
}

#[derive(Debug, thiserror::Error)]
#[error("{message} (termination acknowledged: {acknowledged})")]
pub struct HookCommandError {
    pub message: String,
    pub acknowledged: bool,
}

/// Authorization, sandbox/profile choice, trust and receipt admission precede this call.
/// IO capture is transient; callers must respect telemetry.log_hook_io when recording it.
pub async fn capture_hook_command(
    mut process: HookCommandProcess,
    event: &HookEvent,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<HookCommandCapture, HookCommandError> {
    let Some(stdin) = process.stdin() else {
        return Err(failure(&mut process, "hook stdin unavailable".into()).await);
    };
    let Some(stdout) = process.stdout() else {
        return Err(failure(&mut process, "hook stdout unavailable".into()).await);
    };
    let Some(stderr) = process.stderr() else {
        return Err(failure(&mut process, "hook stderr unavailable".into()).await);
    };
    let mut input = match serde_json::to_vec(event) {
        Ok(input) => input,
        Err(error) => return Err(failure(&mut process, error.to_string()).await),
    };
    input.push(b'\n');
    let completed = {
        let operation = async {
            tokio::join!(
                process.wait_tree(),
                send(stdin, input),
                read(stdout),
                read(stderr)
            )
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            _ = tokio::time::sleep(timeout) => Some(None),
            result = &mut operation => Some(Some(result)),
        }
    };
    match completed {
        Some(Some((status, input, stdout, stderr))) => {
            let collected = collect(status, input, stdout, stderr);
            match collected {
                Ok(capture) => Ok(capture),
                Err(error) => Err(failure(&mut process, error.to_string()).await),
            }
        }
        stopped => {
            let acknowledged = stop(&mut process).await;
            let end = if stopped.is_none() {
                HookCommandEnd::Cancelled { acknowledged }
            } else {
                HookCommandEnd::TimedOut { acknowledged }
            };
            Ok(HookCommandCapture {
                end,
                stdout: empty(),
                stderr: empty(),
            })
        }
    }
}

fn collect(
    status: io::Result<std::process::ExitStatus>,
    input: io::Result<()>,
    stdout: io::Result<HookStreamCapture>,
    stderr: io::Result<HookStreamCapture>,
) -> io::Result<HookCommandCapture> {
    let status = status?;
    if let Err(error) = input
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error);
    }
    Ok(HookCommandCapture {
        end: HookCommandEnd::Exited(status.code()),
        stdout: stdout?,
        stderr: stderr?,
    })
}

async fn send(mut stream: WriteStream, input: Vec<u8>) -> io::Result<()> {
    stream.write_all(&input).await?;
    stream.shutdown().await
}

async fn read(mut stream: ReadStream) -> io::Result<HookStreamCapture> {
    let mut capture = empty();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(capture);
        }
        let kept = count.min(CAPTURE_LIMIT - capture.bytes.len());
        capture.bytes.extend_from_slice(&buffer[..kept]);
        capture.truncated |= kept != count;
    }
}

fn empty() -> HookStreamCapture {
    HookStreamCapture {
        bytes: Vec::new(),
        truncated: false,
    }
}

async fn stop(process: &mut HookCommandProcess) -> bool {
    process.terminate();
    matches!(
        tokio::time::timeout(STOP_ACK, process.wait_tree()).await,
        Ok(Ok(_))
    )
}

async fn failure(process: &mut HookCommandProcess, message: String) -> HookCommandError {
    HookCommandError {
        message,
        acknowledged: stop(process).await,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use cyber_core::hooks::{HookIdentity, HookLocation};
    use std::process::Stdio;

    fn event(directory: &std::path::Path) -> HookEvent {
        let fields = serde_json::json!({"text":"Δ".repeat(128*1024)})
            .as_object()
            .unwrap()
            .clone();
        HookEvent::new(
            "UserPromptSubmit",
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
            fields,
        )
        .unwrap()
    }

    async fn process(script: &str) -> HookCommandProcess {
        HookCommandProcess::spawn("/bin/sh", &["-c".into(), script.into()], None, |command| {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn hook_transport_interprets_actual_command_decisions_and_failures() {
        let directory = tempfile::tempdir().unwrap();
        let event = event(directory.path());
        for (script, outcome, action) in [
            (
                "cat >/dev/null; printf '%s' '{\"decision\":\"deny\",\"reason\":\"policy\"}'",
                HookOutcome::Blocked,
                Some(cyber_core::hooks::HookAction::Deny),
            ),
            (
                "printf blocked >&2; exit 2",
                HookOutcome::Blocked,
                Some(cyber_core::hooks::HookAction::Deny),
            ),
            ("printf failed >&2; exit 1", HookOutcome::Error, None),
        ] {
            let captured = capture_hook_command(
                process(script).await,
                &event,
                Duration::from_secs(10),
                CancellationToken::new(),
            )
            .await;
            let report = interpret_hook_command(&event, "guard", true, captured);
            assert_eq!(report.outcome, outcome);
            assert_eq!(report.decision.decision, action);
            assert!(report.acknowledged && !report.must_stop);
        }
    }

    #[tokio::test]
    async fn hook_transport_pumps_three_streams_and_preserves_block_exit() {
        let directory = tempfile::tempdir().unwrap();
        let event = event(directory.path());
        let process = process("head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2; cat; printf 'blocked' >&2; exit 2").await;
        let captured = capture_hook_command(
            process,
            &event,
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(captured.end, HookCommandEnd::Exited(Some(2)));
        let mut input = serde_json::to_vec(&event).unwrap();
        input.push(b'\n');
        assert!(captured.stdout.bytes.ends_with(&input));
        assert!(captured.stderr.bytes.ends_with(b"blocked"));
        assert!(!captured.stdout.truncated);
    }

    #[tokio::test]
    async fn hook_transport_drains_excess_output_with_bounded_capture() {
        let directory = tempfile::tempdir().unwrap();
        let process =
            process("cat >/dev/null; head -c 2097152 /dev/zero; head -c 2097152 /dev/zero >&2")
                .await;
        let captured = capture_hook_command(
            process,
            &event(directory.path()),
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(captured.end, HookCommandEnd::Exited(Some(0)));
        assert!(captured.stdout.truncated && captured.stderr.truncated);
        assert_eq!(captured.stdout.bytes.len(), CAPTURE_LIMIT);
        assert_eq!(captured.stderr.bytes.len(), CAPTURE_LIMIT);
    }

    #[tokio::test]
    async fn hook_transport_preserves_exit_when_handler_ignores_stdin() {
        let directory = tempfile::tempdir().unwrap();
        let captured = capture_hook_command(
            process("printf blocked >&2; exit 2").await,
            &event(directory.path()),
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(captured.end, HookCommandEnd::Exited(Some(2)));
        assert_eq!(captured.stderr.bytes, b"blocked");
    }

    #[tokio::test]
    async fn hook_transport_refuses_missing_streams_with_termination_acknowledgement() {
        let directory = tempfile::tempdir().unwrap();
        let process = HookCommandProcess::spawn(
            "/bin/sh",
            &["-c".into(), "sleep 30".into()],
            None,
            |command| {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            },
        )
        .await
        .unwrap();
        let error = match capture_hook_command(
            process,
            &event(directory.path()),
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("missing stdin must refuse transport"),
        };
        assert!(error.acknowledged);
        assert_eq!(error.message, "hook stdin unavailable");
    }

    #[tokio::test]
    async fn hook_transport_acknowledges_timeout_and_explicit_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let timed = capture_hook_command(
            process("sleep 30").await,
            &event(directory.path()),
            Duration::from_millis(50),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(timed.end, HookCommandEnd::TimedOut { acknowledged: true });
        let cancel = CancellationToken::new();
        cancel.cancel();
        let cancelled = capture_hook_command(
            process("sleep 30").await,
            &event(directory.path()),
            Duration::from_secs(10),
            cancel,
        )
        .await
        .unwrap();
        assert_eq!(
            cancelled.end,
            HookCommandEnd::Cancelled { acknowledged: true }
        );
    }
}
