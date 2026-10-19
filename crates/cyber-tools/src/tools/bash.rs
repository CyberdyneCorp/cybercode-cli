//! `bash` (`builtin-tools` → bash tool, Bash permission analysis).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::process::Process;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{Tool, ToolError, def, failed, number, text};
use crate::bash_analysis::{Analysis, analyze};
use crate::host::Ctx;
use crate::permissions::{RemovalScope, Request, bash_removal, literal_edits};

const CAPTURE: usize = 1024 * 1024;
pub(super) const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub(crate) const MAX_TIMEOUT_MS: u64 = 600_000;
const SHELLS: &[&str] = &["bash", "zsh", "sh", "dash"];

pub(crate) struct Bash;

impl Tool for Bash {
    fn def(&self) -> ToolDef {
        def(
            "bash",
            "Run a shell command in the project. stdout and stderr are combined. timeout_ms defaults to 120000 (max 600000). When the sandbox denies an operation the result says `Sandbox denied: <operation> <target>`; to ask the user to run that one command outside the sandbox, call again with sandbox: \"escalate\" and a justification. background: true starts a background Job and returns its job_id and log path at once; the Job keeps running after the Turn, its output is read with `read` on that path, `task_stop` stops it, and a completion notice arrives as a queued message unless notify is false. For a dev server add ready_url (an http(s) URL on localhost): the call then returns once that URL answers, or fails with the log tail when the server exits first or ready_timeout_seconds (default 60, max 600) passes.",
            json!({"type": "object", "required": ["command"], "properties": {
                "command": {"type": "string"}, "timeout_ms": {"type": "integer"}, "workdir": {"type": "string"},
                "description": {"type": "string"}, "background": {"type": "boolean"},
                "notify": {"type": "boolean"},
                "ready_url": {"type": "string"}, "ready_timeout_seconds": {"type": "integer"},
                "sandbox": {"type": "string", "enum": ["escalate"]},
                "justification": {"type": "string"}}}),
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
            let background = super::background::requested(input)?;
            let ready = super::dev_server::Readiness::parse(input, background)?;
            let escalation = escalation(input)?;
            let raw = text(input, "workdir");
            let workdir = ctx.resolve(if raw.is_empty() { "." } else { raw });
            let analysis = analyze(command, &workdir);
            let excluded = excluded(ctx, command);
            authorize(ctx, command, &workdir, &analysis, excluded).await?;
            if let Some(justification) = &escalation {
                escalate(ctx, command, justification).await?;
            }
            let timeout = number(input, "timeout_ms")
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS);
            let unsandboxed = excluded || escalation.is_some();
            if background {
                let shell = shell(&ctx.host.opts.shell, &ctx.location)?;
                let prepared =
                    crate::sandboxing::prepare_shell(ctx, &shell, command, unsandboxed).await?;
                return super::dev_server::start_background(
                    ctx,
                    prepared,
                    (&workdir, command, "bash"),
                    ready,
                )
                .await;
            }
            // Ctrl+B may move this call to the background while it runs.
            let handoff = ctx
                .host
                .runtime()
                .map(|runtime| runtime.offer_handoff(&ctx.inv.session_id, &ctx.inv.call_id));
            let offer = handoff.as_ref().map(|handoff| Offer { handoff, command });
            let output = execute(ctx, command, &workdir, timeout, unsandboxed, offer).await?;
            super::pr_links::observe(ctx, command, &workdir, &output).await;
            Ok(output)
        })
    }
}

/// A user shell command (`!`): sandboxed, but never asks for permission. A PR it
/// creates or pushes to links the Session, as for the `bash` tool.
pub(crate) async fn run_user(ctx: &Ctx<'_>, command: &str) -> Result<String, ToolError> {
    let output = execute(ctx, command, &ctx.location, DEFAULT_TIMEOUT_MS, false, None).await?;
    super::pr_links::observe(ctx, command, &ctx.location, &output).await;
    Ok(output)
}

/// A user background command (`& cmd`): sandboxed like `!`, never asking, started as a
/// process Job of the Session. Returns the Job ID.
pub(crate) async fn run_user_background(ctx: &Ctx<'_>, command: &str) -> Result<String, ToolError> {
    if command.trim().is_empty() {
        return Err(failed("command is empty"));
    }
    let shell = shell(&ctx.host.opts.shell, &ctx.location)?;
    let prepared = crate::sandboxing::prepare_shell(ctx, &shell, command, false).await?;
    super::background::start_job(ctx, prepared, &ctx.location, command, "bash")
        .await
        .map(|job| job.id)
}

/// A template's `` !`cmd` `` (`skills-commands` → Shell output injection): `bash`
/// permission for the command text in the Session's mode, asking when the rules say so.
pub(crate) async fn authorize_injection(ctx: &Ctx<'_>, command: &str) -> Result<(), ToolError> {
    let analysis = analyze(command, &ctx.location);
    authorize(ctx, command, &ctx.location, &analysis, false).await
}

/// `{ sandbox: "escalate", justification }` (`sandbox` → Escalation requests).
fn escalation(input: &Value) -> Result<Option<String>, ToolError> {
    match input.get("sandbox") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value == "escalate" => {
            let justification = text(input, "justification").trim();
            if justification.is_empty() {
                return Err(failed(
                    "sandbox: \"escalate\" needs a justification explaining why the command must run outside the sandbox",
                ));
            }
            Ok(Some(justification.to_string()))
        }
        Some(other) => Err(failed(format!("sandbox must be \"escalate\", got {other}"))),
    }
}

/// Ask `sandbox.escalate` for this one call, showing the command and justification.
/// The sandbox is a ceiling: `bypass` still asks, `dont-ask` denies and `auto` classifies.
async fn escalate(ctx: &Ctx<'_>, command: &str, justification: &str) -> Result<(), ToolError> {
    let req = Request {
        action: "sandbox.escalate".into(),
        resources: vec![command.to_string()],
        sandbox_boundary: true,
        ..Request::default()
    };
    let metadata = json!({
        "command": command,
        "justification": justification,
        "warning": "Runs this one command outside the sandbox",
    });
    ctx.authorize(req, Vec::new(), metadata).await?;
    let _ = ctx
        .inv
        .asker
        .record_sandbox(
            cyber_server::runtime::SandboxEvent::Escalated,
            json!({
                "session_id": ctx.inv.session_id,
                "call_id": ctx.inv.call_id,
                "command": command,
                "justification": justification,
            }),
        )
        .await;
    Ok(())
}

/// A single literal command matching `sandbox.excluded_commands` runs outside the
/// sandbox (`sandbox` → Excluded commands). Lists, pipes and expansions never qualify.
fn excluded(ctx: &Ctx<'_>, command: &str) -> bool {
    excluded_by(&crate::sandboxing::effective(ctx), command)
}

pub(crate) fn excluded_by(sandbox: &cyber_sandbox::SandboxConfig, command: &str) -> bool {
    sandbox.policy != cyber_sandbox::Policy::FullAccess
        && !sandbox.excluded_commands.is_empty()
        && crate::bash_analysis::literal_argv(command).is_some()
        && sandbox
            .excluded_commands
            .iter()
            .any(|pattern| cyber_core::wildcard::matches(pattern, command.trim()))
}

/// Run an authorized injection in the sandbox and return its stdout, bounded by
/// `timeout_ms` and the cancellation token. Only the first `max_bytes` are kept: reading
/// stops there and nothing is spilled to a file. A timeout is noted after the output.
pub(crate) async fn run_injected(
    ctx: &Ctx<'_>,
    command: &str,
    timeout_ms: u64,
    max_bytes: usize,
) -> Result<String, ToolError> {
    let prepared =
        crate::sandboxing::prepare(ctx, &shell(&ctx.host.opts.shell, &ctx.location)?, command)
            .await?;
    let output = Output::Stdout { max_bytes };
    let (capture, ended) =
        run_process(ctx, prepared, &ctx.location, timeout_ms, output, None).await?;
    // Like shell command substitution, trailing newlines are dropped.
    let stdout = String::from_utf8_lossy(&capture.tail);
    let stdout = stdout.trim_end_matches(['\n', '\r']);
    Ok(match ended {
        Ended::TimedOut => format!("{stdout}\n[timed out after {timeout_ms} ms]"),
        _ => stdout.to_string(),
    })
}

pub(super) async fn authorize(
    ctx: &Ctx<'_>,
    command: &str,
    workdir: &Path,
    analysis: &Analysis,
    ask_floor: bool,
) -> Result<(), ToolError> {
    let mut facts = Facts::of(command, workdir, analysis, &ctx.location, &ctx.policy.home);
    facts.ask_floor = ask_floor;
    facts.boundary = ask_floor && ctx.host.sandbox_isolates_alone();
    authorize_facts(ctx, command, workdir, analysis, facts).await
}

impl Facts {
    /// Mutated paths and removal risk of `command`, as the `bash` tool decides them.
    pub(crate) fn of(
        command: &str,
        workdir: &Path,
        analysis: &Analysis,
        location: &Path,
        home: &Path,
    ) -> Self {
        let edits = literal_edits(command, workdir);
        let mutates: Vec<PathBuf> = match &edits {
            Some(paths) => paths.clone(),
            None => analysis
                .commands
                .iter()
                .flat_map(|c| c.mutates.clone())
                .collect(),
        };
        let project = cyber_core::config::project_root(location);
        let removal_risk = bash_removal(
            command,
            &RemovalScope {
                location,
                project: &project,
                home,
                workdir,
            },
        );
        Self {
            removal_risk,
            file_edit: edits.is_some(),
            mutates,
            ask_floor: false,
            boundary: false,
        }
    }

    /// The `bash` request for `analysis`'s commands.
    pub(crate) fn request(self, analysis: &Analysis) -> Request {
        Request {
            removal_risk: self.removal_risk,
            action: "bash".into(),
            resources: analysis.commands.iter().map(|c| c.text.clone()).collect(),
            file_edit: self.file_edit,
            mutates: self.mutates,
            ask_floor: self.ask_floor,
            sandbox_boundary: self.boundary,
            ..Request::default()
        }
    }
}

/// What the permission request of one shell command carries beyond its resources.
#[derive(Default)]
pub(crate) struct Facts {
    pub removal_risk: Option<crate::permissions::RemovalRisk>,
    pub file_edit: bool,
    pub mutates: Vec<PathBuf>,
    /// An excluded command: at least `ask` outside `bypass`.
    pub ask_floor: bool,
    /// The excluded command leaves the sandbox that alone isolates this process, so
    /// `bypass` asks for it too (`sandbox` → Excluded commands).
    pub boundary: bool,
}

pub(crate) async fn authorize_facts(
    ctx: &Ctx<'_>,
    command: &str,
    workdir: &Path,
    analysis: &Analysis,
    facts: Facts,
) -> Result<(), ToolError> {
    let mut external = facts.mutates.clone();
    external.push(workdir.to_path_buf());
    let always: Vec<String> = analysis.commands.iter().map(|c| c.always.clone()).collect();
    let mut metadata = json!({"command": command});
    if let Some(risk) = &facts.removal_risk {
        metadata["warning"] = json!(risk.warning());
        metadata["severity"] = json!("danger");
        metadata["requires_confirmation"] = json!(true);
    } else if facts.ask_floor {
        metadata["warning"] = json!("Excluded command: runs outside the sandbox");
    }
    let req = facts.request(analysis);
    // Critical warnings must reach the user before an external-directory approval.
    if req.removal_risk.is_some() {
        ctx.authorize(req, always, metadata).await?;
        ctx.check_external(&external).await
    } else {
        ctx.check_external(&external).await?;
        ctx.authorize(req, always, metadata).await
    }
}

/// The shell program for `bash` and injections. Outside Windows: the configured shell when it
/// is an existing bash/zsh/sh/dash, else `/bin/sh`. On Windows: an existing absolute path to a
/// supported shell as configured, else that shell's name (or `bash`) resolved by
/// [`shell_resolve`](super::shell_resolve), never the WSL launcher; no shell is an error.
#[cfg(not(windows))]
pub(crate) fn shell(configured: &str, _location: &Path) -> Result<String, ToolError> {
    let supported = Path::new(configured)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| SHELLS.contains(&name));
    Ok(if supported && Path::new(configured).exists() {
        configured.to_string()
    } else {
        "/bin/sh".into()
    })
}

#[cfg(windows)]
pub(crate) fn shell(configured: &str, location: &Path) -> Result<String, ToolError> {
    use super::shell_resolve::{NOT_FOUND, Search, resolve};
    let lower = Path::new(configured)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    let (given, name) = if SHELLS.contains(&name) {
        (Some(Path::new(configured)), name)
    } else {
        (None, "bash")
    };
    let checkout = crate::host::canonical(&cyber_core::config::project_root(location));
    resolve(given, name, &Search::from_env(), &checkout)
        .map(|p| p.display().to_string())
        .ok_or_else(|| failed(NOT_FOUND))
}

/// Merged output: the last 1 MiB in memory, everything in a managed file once that overflows.
/// A bounded capture instead keeps only the first `limit` bytes and never writes a file.
struct Capture {
    tail: Vec<u8>,
    dropped: usize,
    file: Option<(PathBuf, std::fs::File)>,
    dir: PathBuf,
    limit: Option<usize>,
    /// Set once the command moved to a background Job: output goes to its log.
    forward: Option<super::background::SharedSink>,
}

impl Capture {
    fn new(dir: PathBuf, limit: Option<usize>) -> Self {
        Self {
            tail: Vec::new(),
            dropped: 0,
            file: None,
            dir,
            limit,
            forward: None,
        }
    }

    /// Returns `false` once a bounded capture is full, so its reader stops.
    fn push(&mut self, chunk: &[u8]) -> bool {
        if let Some(sink) = &self.forward {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(chunk);
            return true;
        }
        if let Some(limit) = self.limit {
            let room = limit.saturating_sub(self.tail.len()).min(chunk.len());
            self.tail.extend_from_slice(&chunk[..room]);
            return self.tail.len() < limit;
        }
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
        true
    }

    /// Send everything from now on to `sink`, starting with what was captured so far.
    fn forward_to(&mut self, sink: super::background::SharedSink) {
        {
            let mut log = sink.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((path, _)) = &self.file {
                let note = format!(
                    "[earlier foreground output: {} bytes omitted; full output at {}]\n",
                    self.dropped,
                    path.display()
                );
                log.write(note.as_bytes());
            }
            log.write(&self.tail);
        }
        self.forward = Some(sink);
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

/// A foreground tool call the user may move to the background (Ctrl+B).
#[derive(Clone, Copy)]
pub(super) struct Offer<'a> {
    pub handoff: &'a cyber_server::runtime::Handoff,
    pub command: &'a str,
}

async fn execute(
    ctx: &Ctx<'_>,
    command: &str,
    workdir: &Path,
    timeout_ms: u64,
    unsandboxed: bool,
    offer: Option<Offer<'_>>,
) -> Result<String, ToolError> {
    let shell = shell(&ctx.host.opts.shell, &ctx.location)?;
    let prepared = crate::sandboxing::prepare_shell(ctx, &shell, command, unsandboxed).await?;
    execute_prepared(ctx, prepared, workdir, timeout_ms, offer).await
}

pub(super) async fn execute_prepared(
    ctx: &Ctx<'_>,
    mut prepared: crate::sandboxing::Prepared,
    workdir: &Path,
    timeout_ms: u64,
    offer: Option<Offer<'_>>,
) -> Result<String, ToolError> {
    let boundary = prepared.boundary.take();
    let refused = Arc::clone(&prepared.refused);
    let (capture, ended) =
        run_process(ctx, prepared, workdir, timeout_ms, Output::Merged, offer).await?;
    if let Ended::Handed(job) = ended {
        return Ok(job);
    }
    let mut out = render(&capture, ended, timeout_ms);
    if let Some(boundary) = boundary {
        let refused = refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let text = String::from_utf8_lossy(&capture.tail);
        let denials = crate::sandboxing::denials(&text, workdir, &boundary, &refused);
        out.push_str(&crate::sandboxing::report(ctx, &denials).await);
    }
    Ok(out)
}

/// Which output a command contributes: tool calls merge stderr into stdout and keep the
/// full output; injections keep only the head of stdout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    Merged,
    Stdout { max_bytes: usize },
}

async fn run_process(
    ctx: &Ctx<'_>,
    mut prepared: crate::sandboxing::Prepared,
    workdir: &Path,
    timeout_ms: u64,
    output: Output,
    mut offer: Option<Offer<'_>>,
) -> Result<(Capture, Ended), ToolError> {
    let (stderr, limit) = match output {
        Output::Merged => (Stdio::piped(), None),
        Output::Stdout { max_bytes } => (Stdio::null(), Some(max_bytes)),
    };
    let mut child = spawn(ctx, &prepared, workdir, stderr).await?;
    let capture = Arc::new(Mutex::new(Capture::new(
        ctx.host.opts.tool_output_dir.clone(),
        limit,
    )));
    let readers: Vec<_> = [child.stdout(), child.stderr()]
        .into_iter()
        .flatten()
        .map(|s| tokio::spawn(pump(s, Arc::clone(&capture))))
        .collect();
    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);
    let ended = loop {
        let ended = wait(ctx, &mut child, &mut prepared.asks, &mut deadline, offer).await;
        let (Ended::Handoff, Some(Offer { command, .. })) = (&ended, offer) else {
            break ended;
        };
        match super::background::admit(ctx, command, "bash").await {
            Ok(admitted) => {
                capture
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .forward_to(Arc::clone(&admitted.sink));
                let owned = super::background::Owned {
                    process: child,
                    prepared,
                    readers,
                };
                let job = super::background::launch(admitted, owned).await?;
                let handed = Ended::Handed(super::background::describe(&job));
                return Ok((Capture::new(PathBuf::new(), limit), handed));
            }
            // The command keeps running in the foreground.
            Err(ToolError::Failed(reason)) => {
                cyber_core::log::error(
                    "bash",
                    &format!("Could not move to the background: {reason}"),
                    json!({}),
                );
                offer = None;
            }
            Err(ToolError::Aborted) => break Ended::Cancelled,
        }
    };
    if !matches!(ended, Ended::Exited(_)) {
        child.terminate();
        let _ = child.wait().await;
    }
    for reader in readers {
        // Background grandchildren may hold the pipes open; do not wait on them forever.
        let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    }
    if matches!(ended, Ended::Cancelled) {
        return Err(ToolError::Aborted);
    }
    let capture = std::mem::replace(
        &mut *capture.lock().unwrap_or_else(PoisonError::into_inner),
        Capture::new(PathBuf::new(), limit),
    );
    Ok((capture, ended))
}

/// Start a prepared shell command with stdin closed, stdout piped and the `CYBER_*`
/// environment, in its own process group (Windows: Job Object).
pub(super) async fn spawn(
    ctx: &Ctx<'_>,
    prepared: &crate::sandboxing::Prepared,
    workdir: &Path,
    stderr: Stdio,
) -> Result<Process, ToolError> {
    let depth = ctx.host.process_mcp_depth(&ctx.inv.session_id);
    Process::spawn(
        &prepared.program,
        &prepared.args,
        ctx.host.opts.sandbox_helper.as_deref(),
        |cmd| {
            cmd.env_clear()
                .envs(prepared.env.iter().map(|(k, v)| (k, v)))
                .current_dir(workdir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(stderr)
                .kill_on_drop(true)
                .env("CYBER", "1")
                .env("CYBER_PID", std::process::id().to_string())
                .env("CYBER_SESSION_ID", &ctx.inv.session_id)
                .env("CYBER_MCP_DEPTH", &depth)
                .env("CYBER_PROJECT_DIR", &ctx.location);
        },
    )
    .await
    .map_err(|e| failed(format!("Could not start the shell: {e}")))
}

/// Wait for exit, timeout, cancellation or a move to the background, answering the
/// proxy's network questions meanwhile.
async fn wait(
    ctx: &Ctx<'_>,
    child: &mut Process,
    asks: &mut Option<tokio::sync::mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
    deadline: &mut std::pin::Pin<&mut tokio::time::Sleep>,
    offer: Option<Offer<'_>>,
) -> Ended {
    let handoff = async {
        match offer {
            Some(offer) => offer.handoff.requested().cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(handoff);
    loop {
        tokio::select! {
            status = child.wait() => return Ended::Exited(status.ok().and_then(|s| s.code())),
            _ = deadline.as_mut() => return Ended::TimedOut,
            _ = ctx.cancel.cancelled() => return Ended::Cancelled,
            () = &mut handoff => return Ended::Handoff,
            Some((host, reply)) = next_ask(asks) => {
                let _ = reply.send(crate::sandboxing::answer(ctx, host).await);
            }
        }
    }
}

pub(super) async fn next_ask(
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
    /// The user asked to move the command to the background.
    Handoff,
    /// The command now runs as this background Job (its JSON description).
    Handed(String),
}

async fn pump(mut stream: impl tokio::io::AsyncRead + Unpin, capture: Arc<Mutex<Capture>>) {
    let mut buf = vec![0_u8; 8192];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let more = capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(&buf[..n]);
        // A full bounded capture closes its pipe; the writer then sees a broken pipe.
        if !more {
            break;
        }
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
        Ended::Cancelled | Ended::Handoff => out,
        Ended::Handed(job) => job,
    }
}

#[cfg(all(test, windows))]
mod shell_tests {
    use std::path::Path;

    /// Native: the Windows CI runner has Git for Windows and the WSL launcher in System32.
    #[test]
    fn bare_and_posix_defaults_resolve_to_a_real_shell_not_the_wsl_launcher() {
        let here = std::env::current_dir().unwrap();
        for configured in [
            "bash",
            "bash.exe",
            "BASH.EXE",
            "/bin/sh",
            "unrecognized-shell",
        ] {
            let found = super::shell(configured, &here).unwrap();
            let lower = found.to_ascii_lowercase();
            assert!(Path::new(&found).is_absolute(), "{configured} -> {found}");
            assert!(!lower.contains("\\system32\\"), "{configured} -> {found}");
            assert!(!lower.contains("windowsapps"), "{configured} -> {found}");
        }
    }
}
