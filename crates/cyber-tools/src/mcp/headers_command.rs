//! `headers_command` for remote MCP servers: run the authorized command outside the
//! sandbox under owned process-tree (Windows Job) ownership, require its stdout to be a
//! JSON object of string headers. The caller layers that output over the static headers and
//! the stored OAuth bearer.
//!
//! Diagnostics are fixed messages plus the exit status. Command output, header names and
//! header values never reach errors, logs, events or status.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use reqwest::header::HeaderMap;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::HookCommandProcess;
use crate::tools::process::ReadStream;

/// Larger stdout fails closed; the rest is drained so the command can still exit.
const OUTPUT_LIMIT: usize = 64 * 1024;
const STOP_ACK: Duration = Duration::from_secs(2);

/// A failed run. `acknowledged` is false when the process tree may still be alive.
#[derive(Debug)]
pub(super) struct CommandFailure {
    pub diagnostic: String,
    pub acknowledged: bool,
}

impl CommandFailure {
    fn settled(diagnostic: impl Into<String>) -> Self {
        Self {
            diagnostic: diagnostic.into(),
            acknowledged: true,
        }
    }
}

/// Everything needed to compute one remote server's request headers.
pub(super) struct HeadersCommand {
    command: String,
    base: HeaderMap,
    location: PathBuf,
    helper: Option<PathBuf>,
}

impl HeadersCommand {
    /// `base` holds the already validated static headers that command output overrides.
    pub fn new(command: &str, base: HeaderMap, location: &Path, helper: Option<&Path>) -> Self {
        Self {
            command: command.into(),
            base,
            location: location.into(),
            helper: helper.map(Path::to_path_buf),
        }
    }

    /// Run the command once before `deadline` and return the merged header set.
    /// Authorization and durable admission belong to the caller.
    pub async fn headers(
        &self,
        deadline: tokio::time::Instant,
        cancel: Option<&CancellationToken>,
    ) -> Result<HeaderMap, CommandFailure> {
        let output = self.run(deadline, cancel).await?;
        merge(&self.base, &output).map_err(CommandFailure::settled)
    }

    fn program(&self) -> Result<(String, Vec<String>), CommandFailure> {
        #[cfg(windows)]
        let program = (
            crate::tools::powershell::installed(&self.location)
                .ok_or_else(|| {
                    CommandFailure::settled("PowerShell is required for MCP headers_command")
                })?
                .display()
                .to_string(),
            crate::tools::powershell::arguments(&self.command),
        );
        #[cfg(not(windows))]
        let program = (
            "/bin/sh".to_string(),
            vec!["-c".into(), self.command.clone()],
        );
        Ok(program)
    }

    async fn run(
        &self,
        deadline: tokio::time::Instant,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, CommandFailure> {
        let (program, args) = self.program()?;
        let mut process =
            HookCommandProcess::spawn(&program, &args, self.helper.as_deref(), |command| {
                command
                    .current_dir(&self.location)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            })
            .await
            .map_err(|_| CommandFailure::settled("could not launch MCP headers_command"))?;
        let (Some(stdout), Some(stderr)) = (process.stdout(), process.stderr()) else {
            return Err(stop(&mut process, "MCP headers_command output is unavailable").await);
        };
        let completed = {
            let operation =
                async { tokio::join!(process.wait_tree(), capture(stdout), capture(stderr)) };
            tokio::pin!(operation);
            tokio::select! {
                biased;
                _ = super::connection::startup_cancelled(cancel) => None,
                _ = tokio::time::sleep_until(deadline) => None,
                result = &mut operation => Some(result),
            }
        };
        let Some((status, stdout, _)) = completed else {
            return Err(stop(
                &mut process,
                "MCP headers_command timed out or was cancelled",
            )
            .await);
        };
        let Ok(status) = status else {
            return Err(stop(&mut process, "MCP headers_command wait failed").await);
        };
        if !status.success() {
            return Err(CommandFailure::settled(match status.code() {
                Some(code) => format!("MCP headers_command exited with status {code}"),
                None => "MCP headers_command was terminated by a signal".into(),
            }));
        }
        match stdout {
            Ok(Captured::Complete(bytes)) => Ok(bytes),
            Ok(Captured::Oversized) => Err(CommandFailure::settled(
                "MCP headers_command output exceeds 64 KiB",
            )),
            Err(_) => Err(CommandFailure::settled(
                "MCP headers_command output could not be read",
            )),
        }
    }
}

enum Captured {
    Complete(Vec<u8>),
    Oversized,
}

async fn capture(mut stream: ReadStream) -> io::Result<Captured> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(if oversized {
                Captured::Oversized
            } else {
                Captured::Complete(bytes)
            });
        }
        oversized |= count > OUTPUT_LIMIT - bytes.len();
        if !oversized {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
}

async fn stop(process: &mut HookCommandProcess, diagnostic: &str) -> CommandFailure {
    process.terminate();
    CommandFailure {
        diagnostic: diagnostic.into(),
        acknowledged: matches!(
            tokio::time::timeout(STOP_ACK, process.wait_tree()).await,
            Ok(Ok(_))
        ),
    }
}

/// Strict output: one JSON object whose values are all strings, then the same name and
/// value checks as static headers, including the transport-owned name refusal.
fn merge(base: &HeaderMap, output: &[u8]) -> Result<HeaderMap, &'static str> {
    let text = std::str::from_utf8(output)
        .map_err(|_| "MCP headers_command output is not UTF-8")?
        .trim_start_matches('\u{feff}');
    let headers: BTreeMap<String, String> = serde_json::from_str(text)
        .map_err(|_| "MCP headers_command output is not a JSON object of strings")?;
    let headers = crate::net_client::headers(
        &headers,
        "MCP headers_command",
        cyber_core::config::transport_owned_header,
    )
    .map_err(|_| "MCP headers_command output is not a valid header set")?;
    let mut merged = base.clone();
    for (name, value) in &headers {
        merged.insert(name.clone(), value.clone());
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_overrides_static_names_case_insensitively() {
        let mut base = HeaderMap::new();
        base.insert("authorization", "Bearer static".parse().unwrap());
        base.insert("x-static", "kept".parse().unwrap());
        let merged = merge(&base, br#"{"Authorization":"Bearer fresh"}"#).unwrap();
        assert_eq!(merged["authorization"], "Bearer fresh");
        assert_eq!(merged["x-static"], "kept");
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn invalid_output_errors_carry_no_output() {
        for output in [
            &b"\xffPRIVATE"[..],
            br#"PRIVATE"#,
            br#"{"x":"PRIVATE","y":1}"#,
            br#"{"Accept":"PRIVATE"}"#,
            br#"{"x":"PRIVATE\n"}"#,
        ] {
            let error = merge(&HeaderMap::new(), output).unwrap_err();
            assert!(!error.contains("PRIVATE"), "{error}");
        }
    }
}
