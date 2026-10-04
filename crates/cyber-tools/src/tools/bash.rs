//! `bash` (`builtin-tools` → bash tool, Bash permission analysis).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{Tool, ToolError, def, failed, number, text};
use crate::bash_analysis::{Analysis, analyze};
use crate::host::Ctx;
use crate::permissions::Request;

const CAPTURE: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const SHELLS: &[&str] = &["bash", "zsh", "sh", "dash"];

pub(crate) struct Bash;

impl Tool for Bash {
    fn def(&self) -> ToolDef {
        def(
            "bash",
            "Run a shell command in the project. stdout and stderr are combined. timeout_ms defaults to 120000 (max 600000).",
            json!({"type": "object", "required": ["command"], "properties": {
                "command": {"type": "string"}, "timeout_ms": {"type": "integer"}, "workdir": {"type": "string"},
                "description": {"type": "string"}, "background": {"type": "boolean"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let command = text(input, "command");
            if command.trim().is_empty() {
                return Err(failed("command is empty"));
            }
            if input.get("background").and_then(Value::as_bool) == Some(true) {
                return Err(failed(
                    "background: true starts a background Job, which arrives in P1; run in the foreground",
                ));
            }
            let raw = text(input, "workdir");
            let workdir = ctx.resolve(if raw.is_empty() { "." } else { raw });
            let analysis = analyze(command, &workdir);
            authorize(ctx, command, &workdir, &analysis).await?;
            let timeout = number(input, "timeout_ms")
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS);
            execute(ctx, command, &workdir, timeout).await
        })
    }
}

/// A user shell command (`!`): sandboxed, but never asks for permission.
pub(crate) async fn run_user(ctx: &Ctx<'_>, command: &str) -> Result<String, ToolError> {
    execute(ctx, command, &ctx.location, DEFAULT_TIMEOUT_MS).await
}

async fn authorize(
    ctx: &Ctx<'_>,
    command: &str,
    workdir: &Path,
    analysis: &Analysis,
) -> Result<(), ToolError> {
    let mutates: Vec<PathBuf> = analysis
        .commands
        .iter()
        .flat_map(|c| c.mutates.clone())
        .collect();
    let mut external = mutates.clone();
    external.push(workdir.to_path_buf());
    ctx.check_external(&external).await?;
    let resources: Vec<String> = analysis.commands.iter().map(|c| c.text.clone()).collect();
    let always: Vec<String> = analysis.commands.iter().map(|c| c.always.clone()).collect();
    let req = Request {
        action: "bash".into(),
        resources,
        file_edit: plain_file_commands(analysis),
        mutates,
        ..Request::default()
    };
    ctx.authorize(req, always, json!({ "command": command }))
        .await
}

/// `accept-edits` allows mkdir, touch, mv, cp and non-recursive rm.
fn plain_file_commands(analysis: &Analysis) -> bool {
    !analysis.unparseable
        && analysis.commands.iter().all(|c| {
            let mut words = c.text.split_whitespace();
            match words.next() {
                Some("mkdir" | "touch" | "mv" | "cp") => true,
                Some("rm") => {
                    !words.any(|w| w.starts_with('-') && (w.contains('r') || w.contains('R')))
                }
                _ => false,
            }
        })
}

fn shell(configured: &str) -> String {
    let name = Path::new(configured)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if SHELLS.contains(&name) && Path::new(configured).exists() {
        configured.to_string()
    } else {
        "/bin/sh".into()
    }
}

/// Merged output: the last 1 MiB in memory, everything in a managed file once that overflows.
struct Capture {
    tail: Vec<u8>,
    dropped: usize,
    file: Option<(PathBuf, std::fs::File)>,
    dir: PathBuf,
}

impl Capture {
    fn push(&mut self, chunk: &[u8]) {
        if let Some((_, f)) = &mut self.file {
            let _ = f.write_all(chunk);
        }
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > 2 * CAPTURE {
            self.spill();
            let excess = self.tail.len() - CAPTURE;
            self.tail.drain(..excess);
            self.dropped += excess;
        }
    }

    /// Start the full-output file with everything captured so far.
    fn spill(&mut self) {
        if self.file.is_some() {
            return;
        }
        let path = self.dir.join(format!(
            "tool_{}",
            cyber_core::ids::new_id("out").trim_start_matches("out_")
        ));
        if let Ok(mut f) =
            std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::File::create_new(&path))
        {
            let _ = f.write_all(&self.tail);
            self.file = Some((path, f));
        }
    }
}

async fn execute(
    ctx: &Ctx<'_>,
    command: &str,
    workdir: &Path,
    timeout_ms: u64,
) -> Result<String, ToolError> {
    let mut prepared =
        crate::sandboxing::prepare(ctx, &shell(&ctx.host.opts.shell), command).await?;
    let mut cmd = tokio::process::Command::new(&prepared.program);
    cmd.args(&prepared.args)
        .env_clear()
        .envs(prepared.env.iter().map(|(k, v)| (k, v)))
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("CYBER", "1")
        .env("CYBER_PID", std::process::id().to_string())
        .env("CYBER_SESSION_ID", &ctx.inv.session_id)
        .env("CYBER_PROJECT_DIR", &ctx.location);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| failed(format!("Could not start the shell: {e}")))?;
    let capture = Arc::new(Mutex::new(Capture {
        tail: Vec::new(),
        dropped: 0,
        file: None,
        dir: ctx.host.opts.tool_output_dir.clone(),
    }));
    let readers = [
        child
            .stdout
            .take()
            .map(|s| tokio::spawn(pump(s, Arc::clone(&capture)))),
        child
            .stderr
            .take()
            .map(|s| tokio::spawn(pump(s, Arc::clone(&capture)))),
    ];
    let pid = child.id();
    let ended = wait(ctx, &mut child, &mut prepared.asks, timeout_ms).await;
    if !matches!(ended, Ended::Exited(_)) {
        kill_group(pid);
        let _ = child.wait().await;
    }
    for reader in readers.into_iter().flatten() {
        // Background grandchildren may hold the pipes open; do not wait on them forever.
        let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    }
    if matches!(ended, Ended::Cancelled) {
        return Err(ToolError::Aborted);
    }
    Ok(render(
        &capture.lock().unwrap_or_else(PoisonError::into_inner),
        ended,
        timeout_ms,
    ))
}

/// Wait for exit, timeout or cancellation, answering the proxy's network questions meanwhile.
async fn wait(
    ctx: &Ctx<'_>,
    child: &mut tokio::process::Child,
    asks: &mut Option<tokio::sync::mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
    timeout_ms: u64,
) -> Ended {
    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            status = child.wait() => return Ended::Exited(status.ok().and_then(|s| s.code())),
            _ = &mut deadline => return Ended::TimedOut,
            _ = ctx.cancel.cancelled() => return Ended::Cancelled,
            Some((host, reply)) = next_ask(asks) => {
                let _ = reply.send(crate::sandboxing::answer(ctx, host).await);
            }
        }
    }
}

async fn next_ask(
    asks: &mut Option<tokio::sync::mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
) -> Option<crate::sandboxing::NetworkAsk> {
    match asks {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

async fn pump(mut stream: impl tokio::io::AsyncRead + Unpin, capture: Arc<Mutex<Capture>>) {
    let mut buf = vec![0_u8; 8192];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(&buf[..n]);
    }
}

fn render(capture: &Capture, ended: Ended, timeout_ms: u64) -> String {
    let mut out = String::from_utf8_lossy(&capture.tail)
        .trim_end()
        .to_string();
    if let Some((path, _)) = &capture.file {
        out = format!(
            "[output truncated: first {} bytes omitted; full output at {}]\n{out}",
            capture.dropped,
            path.display()
        );
    }
    if out.is_empty() {
        out = "(no output)".into();
    }
    match ended {
        Ended::Exited(Some(0)) => out,
        Ended::Exited(Some(code)) => format!("{out}\nExit code: {code}"),
        Ended::Exited(None) => format!("{out}\nTerminated by a signal"),
        Ended::TimedOut => format!("{out}\nCommand timed out after {timeout_ms} ms"),
        Ended::Cancelled => out,
    }
}

/// Terminate the command's whole process group.
#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) else {
        return;
    };
    // SAFETY: killpg only sends a signal; the group was created by process_group(0) at spawn.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}
