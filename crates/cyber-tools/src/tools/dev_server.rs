//! Dev servers (`browser-verification` → Verification artifacts and lifecycle): `bash`
//! with `background: true` and `ready_url` starts the command as an owned process Job,
//! then waits for an explicit readiness check, an HTTP response below 500 from
//! `ready_url` on this machine, within `ready_timeout_seconds`. A process that exits
//! first, or a check that times out (the Job is then stopped), records `startup_failed`
//! browser evidence with the log tail and fails the call: the page was not tested. The
//! Session's end stops a ready dev server, and a cancelled call stops its Job. The probe
//! never follows redirects. A sandboxed dev server whose loopback the sandbox isolates
//! from the host is refused before it starts: always on Windows (the AppContainer), and
//! on Linux and macOS unless `sandbox.network` is `on`.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyber_server::runtime::{EvidenceStatus, Job};
use serde_json::{Value, json};

use super::{ToolError, failed, text};
use crate::host::Ctx;
use crate::sandboxing::Prepared;

const DEFAULT_TIMEOUT_S: u64 = 60;
const MAX_TIMEOUT_S: u64 = 600;
const POLL: Duration = Duration::from_millis(200);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Lines and bytes of the log kept with a startup failure.
const TAIL_LINES: usize = 20;
const TAIL_BYTES: usize = 8 * 1024;

/// The readiness check of a dev server.
pub(super) struct Readiness {
    url: url::Url,
    timeout: Duration,
}

impl Readiness {
    /// `ready_url` and `ready_timeout_seconds`; both need `background: true`.
    pub(super) fn parse(input: &Value, background: bool) -> Result<Option<Self>, ToolError> {
        let raw = text(input, "ready_url").trim();
        let timeout = input.get("ready_timeout_seconds").filter(|v| !v.is_null());
        if raw.is_empty() {
            return match timeout {
                Some(_) => Err(failed("ready_timeout_seconds applies only with ready_url")),
                None => Ok(None),
            };
        }
        if !background {
            return Err(failed("ready_url applies only with background: true"));
        }
        let seconds = match timeout {
            None => DEFAULT_TIMEOUT_S,
            Some(value) => value
                .as_u64()
                .filter(|n| (1..=MAX_TIMEOUT_S).contains(n))
                .ok_or_else(|| {
                    failed(format!(
                        "ready_timeout_seconds must be an integer from 1 to {MAX_TIMEOUT_S}"
                    ))
                })?,
        };
        let url = url::Url::parse(raw).map_err(|e| failed(format!("Invalid ready_url: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") || !local(&url) {
            return Err(failed(
                "ready_url must be an http(s) URL on this machine (localhost, 127.0.0.1 or [::1])",
            ));
        }
        Ok(Some(Self {
            url,
            timeout: Duration::from_secs(seconds),
        }))
    }
}

fn local(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// How waiting for readiness ended.
enum Wait {
    Ready,
    Exited(Result<Option<i32>, String>),
    TimedOut,
}

/// Start `prepared` as a background Job of `kind`: a dev server when `ready` is given.
pub(super) async fn start_background(
    ctx: &Ctx<'_>,
    prepared: Prepared,
    (workdir, command, kind): (&Path, &str, &str),
    ready: Option<Readiness>,
) -> Result<String, ToolError> {
    match ready {
        Some(ready) => start(ctx, prepared, (workdir, command, kind), ready).await,
        None => super::background::start(ctx, prepared, workdir, command, kind).await,
    }
}

/// Start `prepared` as a dev server Job and wait for its readiness check.
async fn start(
    ctx: &Ctx<'_>,
    prepared: Prepared,
    (workdir, command, kind): (&Path, &str, &str),
    ready: Readiness,
) -> Result<String, ToolError> {
    loopback_reachable(prepared.loopback_isolated)?;
    let admitted = super::background::admit(ctx, command, kind).await?;
    let mut process = super::bash::spawn(ctx, &prepared, workdir, Stdio::piped()).await?;
    let readers = [process.stdout(), process.stderr()]
        .into_iter()
        .flatten()
        .map(|stream| tokio::spawn(super::background::pump(stream, Arc::clone(&admitted.sink))))
        .collect();
    let owned = super::background::Owned {
        process,
        prepared,
        readers,
    };
    let (job, exit) = super::background::launch_observed(admitted, owned).await?;
    let waited = wait(ctx, &ready, exit).await;
    if waited.is_err()
        && let Some(runtime) = ctx.host.runtime()
    {
        // The call was cancelled: nobody owns the half-started server, and its Turn is
        // over, so it stops without a completion notice.
        let _ = runtime.cancel_job_quietly(&job.id).await;
    }
    match waited? {
        Wait::Ready => {
            ctx.host.own_dev_server(&ctx.inv.session_id, &job.id);
            Ok(json!({
                "job_id": job.id,
                "status": job.status,
                "output_path": job.output_path,
                "ready_url": ready.url.as_str(),
                "message": format!("The dev server is ready at {}. It runs in the background until task_stop stops it or the session ends; read output_path for its output.", ready.url),
            })
            .to_string())
        }
        Wait::Exited(code) => Err(startup_failed(ctx, &job, &ready, exited(code)).await),
        Wait::TimedOut => {
            if let Some(runtime) = ctx.host.runtime() {
                let _ = runtime.cancel_job(&job.id).await;
            }
            let reason = format!(
                "the readiness check of {} timed out after {} s; the dev server was stopped",
                ready.url,
                ready.timeout.as_secs()
            );
            Err(startup_failed(ctx, &job, &ready, reason).await)
        }
    }
}

/// Whether the core can reach a dev server on loopback. A command the sandbox isolates
/// from the host's loopback could never answer the readiness check: a Windows
/// AppContainer never reaches it, Linux runs the command in its own network namespace and
/// macOS denies it listening unless `sandbox.network` is `on`. The call is refused before
/// anything starts instead of always failing or timing out.
fn loopback_reachable(isolated: bool) -> Result<(), ToolError> {
    if !isolated {
        return Ok(());
    }
    let (reason, remedy) = if cfg!(windows) {
        (
            "the Windows sandbox isolates loopback",
            "start it without ready_url, or run the Session with --sandbox full-access",
        )
    } else {
        (
            "the sandbox isolates loopback unless sandbox.network is on",
            "start it without ready_url, set sandbox.network to on, or run the Session with --sandbox full-access",
        )
    };
    Err(failed(format!(
        "ready_url is not available for this sandboxed command: {reason}, so the readiness check cannot reach the dev server; {remedy}"
    )))
}

fn exited(code: Result<Option<i32>, String>) -> String {
    match code {
        Ok(Some(code)) => format!("the dev server exited with code {code} before it was ready"),
        Ok(None) => "the dev server was stopped before it was ready".into(),
        Err(error) => format!("the dev server failed before it was ready: {error}"),
    }
}

/// Probe `ready.url` until it answers, the process ends or the timeout passes. A
/// cancelled call stops the Job.
async fn wait(
    ctx: &Ctx<'_>,
    ready: &Readiness,
    mut exit: super::background::Exit,
) -> Result<Wait, ToolError> {
    // The probe runs in the core, outside the sandbox and its proxy: following a
    // redirect would let the dev server make it fetch any host.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| failed(format!("Could not build the readiness client: {e}")))?;
    let deadline = tokio::time::Instant::now() + ready.timeout;
    loop {
        tokio::select! {
            code = &mut exit => return Ok(Wait::Exited(code.unwrap_or(Ok(None)))),
            () = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
            answered = probe(&client, &ready.url) => {
                if answered {
                    return Ok(Wait::Ready);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Wait::TimedOut);
        }
        tokio::select! {
            code = &mut exit => return Ok(Wait::Exited(code.unwrap_or(Ok(None)))),
            () = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

async fn probe(client: &reqwest::Client, url: &url::Url) -> bool {
    client
        .get(url.as_str())
        .send()
        .await
        .is_ok_and(|response| response.status().as_u16() < 500)
}

/// Record the failure as `startup_failed` evidence with the (redacted) log tail, and
/// describe it for the model.
async fn startup_failed(ctx: &Ctx<'_>, job: &Job, ready: &Readiness, reason: String) -> ToolError {
    let tail = job
        .output_path
        .as_deref()
        .map(|path| log_tail(Path::new(path)))
        .unwrap_or_default();
    let tail = cyber_core::memory::redact_credentials(&tail);
    let mut message = format!(
        "Dev server startup failed: {reason}. The page at {} was not tested.",
        ready.url
    );
    if !tail.is_empty() {
        message.push_str(&format!("\nLog tail:\n{tail}"));
    }
    match record(ctx, job, ready, &reason, &tail).await {
        Ok(id) => message.push_str(&format!("\n[browser evidence {id} recorded]")),
        Err(error) => message.push_str(&format!("\n[browser evidence not recorded: {error}]")),
    }
    failed(message)
}

async fn record(
    ctx: &Ctx<'_>,
    job: &Job,
    ready: &Readiness,
    reason: &str,
    tail: &str,
) -> Result<String, String> {
    let runtime = ctx.host.runtime().ok_or("the runtime is unavailable")?;
    let log = crate::browser::store_artifact(
        &ctx.host.browser_root(),
        &ctx.inv.session_id,
        "log",
        "text/plain",
        tail.as_bytes(),
    )?;
    let taken = crate::browser::Taken {
        url: ready.url.to_string(),
        viewport: None,
        actions: vec![format!(
            "start dev server {} ({})",
            job.command.clone().unwrap_or_default(),
            job.id
        )],
        observed: vec![reason.to_string()],
        artifacts: vec![log],
        not_run: vec![format!(
            "page at {} not tested: the dev server never became ready",
            ready.url
        )],
    };
    let evidence = crate::browser::record(
        &runtime,
        &ctx.inv.session_id,
        &ctx.inv.call_id,
        &ctx.location,
        EvidenceStatus::StartupFailed,
        taken,
    )
    .await?;
    Ok(evidence.id)
}

/// The last lines of a Job log, at most [`TAIL_BYTES`].
fn log_tail(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    let text = String::from_utf8_lossy(&bytes[start..]);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_needs_background_and_a_local_http_url() {
        let parse = |input: Value, background| Readiness::parse(&input, background);
        assert!(parse(json!({}), true).unwrap().is_none());
        let ready = parse(json!({"ready_url": "http://127.0.0.1:5173/"}), true)
            .unwrap()
            .unwrap();
        assert_eq!(ready.timeout, Duration::from_secs(60));
        assert!(parse(json!({"ready_url": "http://localhost:3000"}), false).is_err());
        assert!(parse(json!({"ready_url": "http://example.com/"}), true).is_err());
        assert!(parse(json!({"ready_url": "ftp://127.0.0.1/"}), true).is_err());
        assert!(parse(json!({"ready_timeout_seconds": 5}), true).is_err());
        assert!(
            parse(
                json!({"ready_url": "http://[::1]:8080/", "ready_timeout_seconds": 601}),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn a_dev_server_with_isolated_loopback_is_refused_before_it_starts() {
        assert!(loopback_reachable(false).is_ok());
        let Err(ToolError::Failed(message)) = loopback_reachable(true) else {
            panic!("a ready_url below an isolating sandbox must be refused");
        };
        assert!(message.contains("isolates loopback"), "{message}");
        assert!(message.contains("--sandbox full-access"), "{message}");
    }

    #[test]
    fn log_tail_keeps_the_last_twenty_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        let text: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let tail = log_tail(&path);
        assert!(tail.starts_with("line 11") && tail.ends_with("line 30"));
    }
}
