//! Host-owned literal-argv verification. No shell expansion or agent assertions.
use std::process::Stdio;
use std::time::Duration;

use cyber_workflow::solve::Check;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{ToolError, bash, failed};
use crate::host::Ctx;

const CAP: usize = 1024 * 1024;

/// Permission and hook admission precede native launch. The command text is a quoted
/// representation for policy inspection only; execution always uses the original argv.
pub(crate) async fn run(ctx: &Ctx<'_>, check: &Check) -> Result<Value, ToolError> {
    let started = std::time::Instant::now();
    let command = check
        .argv
        .iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    ctx.host.check_agent_tool(ctx.inv).map_err(failed)?;
    if !ctx
        .host
        .hooks_admit_host_bash(ctx.inv, &command, ctx.cancel.clone())
        .await
    {
        return Err(failed("verification command blocked or rewritten by hook"));
    }
    bash::authorize_injection(ctx, &command).await?;
    let mut prepared =
        crate::sandboxing::prepare_command(ctx, &check.argv[0], &check.argv[1..]).await?;
    let timeout_ms = check_timeout(ctx, check);
    if timeout_ms == 0 {
        return Ok(
            json!({"outcome":"timed_out","settled":true,"exit_code":null,"log":"","log_truncated":false,
            "limitation":"run wall budget ended before native launch"}),
        );
    }
    let mut process = bash::spawn(ctx, &prepared, &ctx.location, Stdio::piped()).await?;
    let mut readers = Vec::new();
    for stream in [process.stdout(), process.stderr()].into_iter().flatten() {
        readers.push(tokio::spawn(capture(stream)));
    }
    let (outcome, exit_code) = wait(ctx, &mut process, &mut prepared, timeout_ms).await;
    process.terminate();
    let settled = tokio::time::timeout(Duration::from_secs(2), process.wait())
        .await
        .is_ok_and(|result| result.is_ok());
    let (log, truncated, drained) = collect(readers).await;
    let settled = settled && drained;
    let (log, encoding_truncated) = text_log(&log);
    let mut result = json!({"outcome":if settled {outcome} else {"unknown"}, "exit_code":exit_code,
        "settled":settled,"log":log,"log_truncated":truncated || encoding_truncated,"log_order":"stdout_then_stderr"});
    let mut probe = ctx.inv.clone();
    probe.input = json!({"command":command});
    let outcome = if result["outcome"] == "passed" {
        cyber_server::runtime::ToolOutcome::Ok(result.to_string())
    } else {
        cyber_server::runtime::ToolOutcome::Failed(result.to_string())
    };
    if let Err(error) = ctx
        .host
        .post_tool_hooks(&probe, &outcome, started, ctx.cancel.clone())
        .await
    {
        result["outcome"] = "unknown".into();
        result["settled"] = false.into();
        result["limitation"] = format!("verification post-hook did not settle: {error:?}").into();
    }
    Ok(result)
}

fn check_timeout(ctx: &Ctx<'_>, check: &Check) -> u64 {
    let declared = check.timeout_seconds.saturating_mul(1000);
    let Some(deadline) = ctx.inv.input["deadline_ms"].as_i64() else {
        return declared;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    declared.min((deadline as u128).saturating_sub(now).min(u64::MAX as u128) as u64)
}

async fn wait(
    ctx: &Ctx<'_>,
    process: &mut super::process::Process,
    prepared: &mut crate::sandboxing::Prepared,
    timeout_ms: u64,
) -> (&'static str, Option<i32>) {
    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            status = process.wait_tree() => break match status {
                Ok(status) => (if status.success() { "passed" } else { "failed" }, status.code()),
                Err(_) => ("unknown", None),
            },
            _ = &mut deadline => break ("timed_out", None),
            _ = ctx.cancel.cancelled() => break ("cancelled", None),
            Some((host, reply)) = bash::next_ask(&mut prepared.asks) => {
                let _ = reply.send(crate::sandboxing::answer(ctx, host).await);
            }
        }
    }
}

async fn collect(
    readers: Vec<tokio::task::JoinHandle<(Vec<u8>, bool, bool)>>,
) -> (Vec<u8>, bool, bool) {
    let mut log = Vec::new();
    let mut truncated = false;
    let mut drained = true;
    for mut reader in readers {
        match tokio::time::timeout(Duration::from_secs(2), &mut reader).await {
            Ok(Ok((bytes, overflow, read_ok))) => {
                drained &= read_ok;
                let room = CAP.saturating_sub(log.len());
                truncated |= overflow || bytes.len() > room;
                log.extend(bytes.into_iter().take(room));
            }
            _ => {
                reader.abort();
                drained = false;
            }
        }
    }
    (log, truncated, drained)
}

async fn capture(mut stream: super::process::ReadStream) -> (Vec<u8>, bool, bool) {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut read_ok = true;
    let mut buffer = [0; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                let room = CAP.saturating_sub(bytes.len());
                truncated |= count > room;
                bytes.extend_from_slice(&buffer[..count.min(room)]);
            }
            Err(_) => {
                truncated = true;
                read_ok = false;
                break;
            }
        }
    }
    (bytes, truncated, read_ok)
}

fn text_log(bytes: &[u8]) -> (String, bool) {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let truncated = text.len() > CAP;
    let mut end = text.len().min(CAP);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_utf8_expansion_cannot_exceed_the_log_byte_limit() {
        let (text, truncated) = text_log(&vec![255; CAP]);
        assert!(text.len() <= CAP);
        assert!(truncated);
    }
}
