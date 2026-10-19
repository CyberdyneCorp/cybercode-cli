//! `pty_start`, `pty_write` and `pty_read` (`background-tasks` → Model interaction with
//! PTYs, PTY sessions): an interactive program on a pseudo-terminal, owned by the runtime
//! as a process Job of kind `pty`. Starting is authorized and sandboxed as `bash` for the
//! command; input is authorized against the same command. Unix only.

use std::sync::Arc;
use std::time::Duration;

use cyber_server::runtime::{
    JobIo, Pty, PtyChunk, PtyCursor, PtyFeed, PtySpec, RetrySafety, ToolDef,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::background::{SharedSink, Sink};
use super::process::Process;
use super::terminal::Master;
use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::sandboxing::Prepared;

mod typed;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
const MAX_SIZE: u64 = 1000;
const DEFAULT_WAIT_MS: u64 = 1000;
const MAX_WAIT_MS: u64 = 30_000;
/// Output is complete once the program has been quiet this long.
const SETTLE: Duration = Duration::from_millis(200);
const DEFAULT_READ_BYTES: u64 = 16 * 1024;
const MAX_READ_BYTES: u64 = 256 * 1024;
/// Bytes of one `pty_write`.
const MAX_INPUT_BYTES: usize = 64 * 1024;

pub(crate) struct PtyStart;
pub(crate) struct PtyWrite;
pub(crate) struct PtyRead;

impl Tool for PtyStart {
    fn def(&self) -> ToolDef {
        def(
            "pty_start",
            "Start an interactive program (a REPL, an installer that prompts, a TUI) on a pseudo-terminal in the background, or the preferred shell when command is omitted. Returns the job_id. Then send input with pty_write and read output with pty_read; task_stop ends it. Use bash for commands that need no input.",
            json!({"type": "object", "additionalProperties": false, "properties": {
                "command": {"type": "string"},
                "cols": {"type": "integer", "minimum": 20},
                "rows": {"type": "integer", "minimum": 5},
                "description": {"type": "string"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(start(ctx))
    }
}

impl Tool for PtyWrite {
    fn def(&self) -> ToolDef {
        def(
            "pty_write",
            "Send input to a PTY started with pty_start: input is typed as given (end a line with \\r or \\n), and keys are named keys sent after it: enter, tab, esc, backspace, up, down, left, right, home, end, ctrl-a … ctrl-z (ctrl-c interrupts, ctrl-d ends input). Read the result with pty_read.",
            json!({"type": "object", "required": ["job_id"], "additionalProperties": false, "properties": {
                "job_id": {"type": "string"},
                "input": {"type": "string"},
                "keys": {"type": "array", "items": {"type": "string"}}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(write(ctx))
    }
}

impl Tool for PtyRead {
    fn def(&self) -> ToolDef {
        def(
            "pty_read",
            "Read a PTY started with pty_start. mode output (default) returns the output since your last pty_read as plain text, waiting up to wait_ms (default 1000) for the program to print and go quiet; mode screen returns what the terminal shows now. Pass cursor to read from an earlier position.",
            json!({"type": "object", "required": ["job_id"], "additionalProperties": false, "properties": {
                "job_id": {"type": "string"},
                "mode": {"type": "string", "enum": ["output", "screen"]},
                "wait_ms": {"type": "integer", "minimum": 0},
                "cursor": {"type": "integer", "minimum": 0},
                "max_bytes": {"type": "integer", "minimum": 1}}}),
            RetrySafety::ReadOnly,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(read(ctx))
    }
}

fn dimension(input: &Value, key: &str, default: u16, min: u64) -> Result<u16, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|n| (min..=MAX_SIZE).contains(n))
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| failed(format!("{key} must be an integer from {min} to {MAX_SIZE}"))),
    }
}

/// The command to run as authorized, and its sandboxed launch.
async fn prepare(ctx: &Ctx<'_>) -> Result<(String, Prepared), ToolError> {
    let shell = super::bash::shell(&ctx.host.opts.shell, &ctx.location)?;
    let command = text(&ctx.inv.input, "command").trim();
    let label = if command.is_empty() {
        shell.as_str()
    } else {
        command
    };
    let analysis = crate::bash_analysis::analyze(label, &ctx.location);
    super::bash::authorize(ctx, label, &ctx.location, &analysis, false).await?;
    let prepared = if command.is_empty() {
        crate::sandboxing::prepare_command(ctx, &shell, &["-i".to_string()]).await?
    } else {
        crate::sandboxing::prepare_shell(ctx, &shell, &script(command), false).await?
    };
    Ok((label.to_string(), prepared))
}

/// A single literal command replaces the wrapping shell, so the program leads the
/// terminal's session: ctrl-c reaches it alone and its exit code is the Job's. Shell
/// syntax keeps the shell, which a ctrl-c may end first.
fn script(command: &str) -> String {
    if crate::bash_analysis::literal_argv(command).is_some() {
        format!("exec {command}")
    } else {
        command.to_string()
    }
}

async fn start(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let input = &ctx.inv.input;
    let size = (
        dimension(input, "cols", DEFAULT_COLS, 20)?,
        dimension(input, "rows", DEFAULT_ROWS, 5)?,
    );
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("PTYs require the runtime"))?;
    let (command, mut prepared) = prepare(ctx).await?;
    let admitted = super::background::admit(ctx, &command, "pty").await?;
    let (process, master) = spawn(ctx, &prepared, size)?;
    let asks = prepared.asks.take();
    // Registered before the call returns, so the model can write to it at once.
    let master = Arc::new(master);
    let feed = runtime.register_pty(PtySpec {
        session_id: ctx.inv.session_id.clone(),
        job_id: admitted.id().to_string(),
        command,
        cols: size.0,
        rows: size.1,
        input: input_writer(Arc::clone(&master)),
    });
    let job = super::background::launch_with(admitted, move |io, sink| {
        let terminal = Terminal {
            process,
            master,
            feed,
            _prepared: prepared,
        };
        Box::pin(own(terminal, asks, sink, io))
    })
    .await?;
    Ok(json!({
        "job_id": job.id,
        "status": job.status,
        "output_path": job.output_path,
        "message": "The program runs on a terminal in the background. Send input with pty_write and read its output with pty_read; task_stop ends it.",
    })
    .to_string())
}

/// Start the prepared command on a new terminal with the shell environment, plus
/// `TERM=xterm-256color` and `CYBER_TERMINAL=1`.
fn spawn(
    ctx: &Ctx<'_>,
    prepared: &Prepared,
    size: (u16, u16),
) -> Result<(Process, Master), ToolError> {
    let depth = ctx.host.process_mcp_depth(&ctx.inv.session_id);
    let (args, delegated) = terminal_args(ctx, prepared);
    super::terminal::spawn(&prepared.program, &args, size, delegated, |cmd| {
        cmd.env_clear()
            .envs(prepared.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&ctx.location)
            .kill_on_drop(true)
            .env("TERM", "xterm-256color")
            .env("CYBER_TERMINAL", "1")
            .env("CYBER", "1")
            .env("CYBER_PID", std::process::id().to_string())
            .env("CYBER_SESSION_ID", &ctx.inv.session_id)
            .env("CYBER_MCP_DEPTH", &depth)
            .env("CYBER_PROJECT_DIR", &ctx.location);
    })
    .map_err(|e| failed(format!("Could not start the terminal: {e}")))
}

/// The arguments to start, and whether a helper inside them takes the terminal. Under
/// bubblewrap the command runs in a session of its own, so bubblewrap must not hold the
/// terminal: ctrl-c's SIGINT would reach it instead of the command and end the sandbox.
/// `cyber-sandbox-exec --tty` gives the command the terminal inside it.
#[cfg(target_os = "linux")]
fn terminal_args(ctx: &Ctx<'_>, prepared: &Prepared) -> (Vec<String>, bool) {
    let bubblewrap = std::path::Path::new(&prepared.program).file_name() == Some("bwrap".as_ref());
    // The command follows bubblewrap's last `--` (after the proxy bridge's, if any).
    let command = prepared.args.iter().rposition(|arg| arg == "--");
    match (bubblewrap, command, &ctx.host.opts.sandbox_helper) {
        (true, Some(at), Some(helper)) => {
            let mut args = prepared.args[..=at].to_vec();
            args.extend([helper.display().to_string(), "--tty".into(), "--".into()]);
            args.extend_from_slice(&prepared.args[at + 1..]);
            (args, true)
        }
        _ => (prepared.args.clone(), false),
    }
}

#[cfg(not(target_os = "linux"))]
fn terminal_args(_ctx: &Ctx<'_>, prepared: &Prepared) -> (Vec<String>, bool) {
    (prepared.args.clone(), false)
}

/// Input goes through one writer task, in order, without blocking the caller.
fn input_writer(master: Arc<Master>) -> cyber_server::runtime::PtyInput {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if master.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    Box::new(move |bytes| {
        tx.send(bytes.to_vec())
            .map_err(|_| "The terminal is closed".to_string())
    })
}

/// A running PTY Job's terminal. `_prepared` keeps the sandbox proxy alive.
struct Terminal {
    process: Process,
    master: Arc<Master>,
    feed: PtyFeed,
    _prepared: Prepared,
}

/// The PTY Job: retain and log output until the program exits or the Job is stopped.
/// Network questions from the sandbox proxy are refused: no person can answer them.
async fn own(
    mut terminal: Terminal,
    mut asks: Option<mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
    sink: SharedSink,
    io: JobIo,
) -> Result<Option<i32>, String> {
    let stop = Sink::attach(&sink, io).stop.clone();
    let mut buf = vec![0_u8; 8192];
    let exited = loop {
        tokio::select! {
            read = terminal.master.read(&mut buf) => match read {
                Ok(n) if n > 0 => record(&terminal.feed, &sink, &buf[..n]),
                _ => break true,
            },
            _ = stop.cancelled() => break false,
            Some((_, reply)) = super::bash::next_ask(&mut asks) => { let _ = reply.send(false); }
        }
    };
    let code = if exited {
        // The terminal closed: the program is gone or about to be.
        match tokio::time::timeout(Duration::from_secs(5), terminal.process.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => {
                super::background::stop_tree(&mut terminal.process).await;
                None
            }
        }
    } else {
        super::background::stop_tree(&mut terminal.process).await;
        None
    };
    drop(terminal.feed);
    Ok(code)
}

fn record(feed: &PtyFeed, sink: &SharedSink, bytes: &[u8]) {
    feed.push(bytes);
    sink.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .write(bytes);
}

/// The running PTY of this Session's Job.
fn owned(ctx: &Ctx<'_>, job_id: &str) -> Result<Option<Arc<Pty>>, ToolError> {
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("PTYs require the runtime"))?;
    if let Some(pty) = runtime.pty(job_id) {
        return if pty.session_id == ctx.inv.session_id {
            Ok(Some(pty))
        } else {
            Err(not_found(job_id))
        };
    }
    match runtime.find_job(job_id) {
        Ok(Some(job)) if job.session_id == ctx.inv.session_id && job.kind == "pty" => Ok(None),
        _ => Err(not_found(job_id)),
    }
}

fn not_found(job_id: &str) -> ToolError {
    failed(format!("No PTY {job_id} in this session"))
}

fn exited(job_id: &str) -> ToolError {
    failed(format!(
        "PTY {job_id} has exited; read its output_path for the rest of its output"
    ))
}

async fn write(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let job_id = text(&ctx.inv.input, "job_id");
    let bytes = keystrokes(&ctx.inv.input)?;
    let pty = owned(ctx, job_id)?.ok_or_else(|| exited(job_id))?;
    let mut typed = pty.typed().await;
    let pending = authorize_input(ctx, &pty, &typed, &bytes).await?;
    pty.write(&bytes).map_err(failed)?;
    *typed = pending;
    Ok(json!({"job_id": job_id, "written": bytes.len()}).to_string())
}

/// Each command typed into a shell (continued lines joined) is authorized as a `bash`
/// command (rules, the critical-path removal guard, mutated paths and the
/// excluded-command floor) before any of the input is written; an alias definition is
/// refused. The result is the new pending input. Input to another
/// program is checked against the `bash` rules for its command.
async fn authorize_input(
    ctx: &Ctx<'_>,
    pty: &Pty,
    pending: &[u8],
    bytes: &[u8],
) -> Result<Vec<u8>, ToolError> {
    if !typed::is_shell(&pty.command) {
        authorize_program_input(ctx, pty, bytes).await?;
        return Ok(Vec::new());
    }
    let (lines, rest) = typed::submit(pending, bytes).map_err(failed)?;
    for line in &lines {
        let analysis = crate::bash_analysis::analyze(line, &ctx.location);
        if typed::defines_alias(&analysis) {
            return Err(failed(
                "Aliases cannot be defined in a shell PTY: the shell would expand them in later lines no check sees. Type the command itself.",
            ));
        }
        let floor = super::bash::excluded_by(&crate::sandboxing::effective(ctx), line);
        super::bash::authorize(ctx, line, &ctx.location, &analysis, floor).await?;
    }
    Ok(rest)
}

/// A rule that denies or asks for the program's command applies to everything typed
/// into it. Its removal risk was confirmed when the PTY started.
async fn authorize_program_input(ctx: &Ctx<'_>, pty: &Pty, bytes: &[u8]) -> Result<(), ToolError> {
    let analysis = crate::bash_analysis::analyze(&pty.command, &ctx.location);
    let req = crate::permissions::Request {
        action: "bash".into(),
        resources: analysis.commands.iter().map(|c| c.text.clone()).collect(),
        ..crate::permissions::Request::default()
    };
    let always = analysis.commands.iter().map(|c| c.always.clone()).collect();
    let metadata = json!({
        "command": pty.command,
        "pty_input": String::from_utf8_lossy(bytes),
    });
    ctx.authorize(req, always, metadata).await
}

/// `input` followed by the named `keys`.
fn keystrokes(input: &Value) -> Result<Vec<u8>, ToolError> {
    let mut bytes = text(input, "input").as_bytes().to_vec();
    let keys = match input.get("keys") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(keys)) => keys.as_slice(),
        Some(other) => return Err(failed(format!("keys must be an array, got {other}"))),
    };
    for key in keys {
        let name = key
            .as_str()
            .ok_or_else(|| failed(format!("keys must be strings, got {key}")))?;
        bytes.extend(key_bytes(name).ok_or_else(|| failed(format!("Unknown key: {name}")))?);
    }
    if bytes.is_empty() {
        return Err(failed("Give input or keys to send"));
    }
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(failed(format!(
            "Input is limited to {MAX_INPUT_BYTES} bytes per call"
        )));
    }
    Ok(bytes)
}

fn key_bytes(name: &str) -> Option<Vec<u8>> {
    let name = name.trim().to_ascii_lowercase();
    if let Some(letter) = name
        .strip_prefix("ctrl-")
        .or_else(|| name.strip_prefix("ctrl+"))
    {
        let [byte] = letter.as_bytes() else {
            return None;
        };
        return byte.is_ascii_lowercase().then(|| vec![byte & 0x1f]);
    }
    let sequence: &[u8] = match name.as_str() {
        "enter" | "return" => b"\r",
        "tab" => b"\t",
        "esc" | "escape" => b"\x1b",
        "backspace" => b"\x7f",
        "space" => b" ",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        _ => return None,
    };
    Some(sequence.to_vec())
}

/// How a read is shaped.
struct ReadSpec {
    screen: bool,
    wait: Duration,
    cursor: Option<u64>,
    max_bytes: usize,
}

impl ReadSpec {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let screen = match text(input, "mode") {
            "" | "output" => false,
            "screen" => true,
            other => {
                return Err(failed(format!(
                    "mode must be output or screen, got {other}"
                )));
            }
        };
        let wait = number(input, "wait_ms")
            .unwrap_or(DEFAULT_WAIT_MS)
            .min(MAX_WAIT_MS);
        let max_bytes = number(input, "max_bytes")
            .unwrap_or(DEFAULT_READ_BYTES)
            .clamp(1, MAX_READ_BYTES);
        Ok(Self {
            screen,
            wait: Duration::from_millis(wait),
            cursor: number(input, "cursor"),
            max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
        })
    }
}

async fn read(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let job_id = text(&ctx.inv.input, "job_id");
    let spec = ReadSpec::parse(&ctx.inv.input)?;
    let pty = owned(ctx, job_id)?.ok_or_else(|| exited(job_id))?;
    let from = spec.cursor.unwrap_or_else(|| pty.read_cursor());
    let (bytes, end, closed) = tokio::select! {
        collected = collect(&pty, from, spec.wait) => collected,
        _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
    };
    pty.set_read_cursor(end);
    let status = if closed { "exited" } else { "running" };
    let text = if spec.screen {
        screen(&pty)
    } else {
        tail(&plain(&bytes), spec.max_bytes)
    };
    let key = if spec.screen { "screen" } else { "output" };
    Ok(json!({"job_id": job_id, "status": status, "cursor": end, key: text}).to_string())
}

/// Output from `from`: what is retained, then live output until the program has been
/// quiet for [`SETTLE`] or `wait` passes. Returns the bytes, the cursor after them and
/// whether the program exited.
async fn collect(pty: &Pty, from: u64, wait: Duration) -> (Vec<u8>, u64, bool) {
    let mut attach = pty.attach(PtyCursor::At(from));
    let mut end = attach.end();
    let mut bytes = std::mem::take(&mut attach.replay);
    if attach.closed {
        return (bytes, end, true);
    }
    let deadline = Instant::now() + wait;
    loop {
        // Before any output, wait for the first; after it, only for the quiet period.
        let quiet = if bytes.is_empty() {
            deadline
        } else {
            (Instant::now() + SETTLE).min(deadline)
        };
        match tokio::time::timeout_at(quiet, attach.live.recv()).await {
            Ok(Ok(PtyChunk::Output {
                cursor,
                bytes: chunk,
            })) => {
                let chunk_end = cursor + chunk.len() as u64;
                if chunk_end > end {
                    let skip = usize::try_from(end.saturating_sub(cursor)).unwrap_or(0);
                    bytes.extend_from_slice(&chunk[skip..]);
                    end = chunk_end;
                }
            }
            Ok(Ok(PtyChunk::Closed) | Err(_)) => return (bytes, end, true),
            Err(_elapsed) => return (bytes, end, pty.closed()),
        }
    }
}

/// What the terminal shows: the retained output replayed through a terminal emulator of
/// the PTY's size.
fn screen(pty: &Pty) -> String {
    let attach = pty.attach(PtyCursor::Replay);
    let mut parser = vt100::Parser::new(pty.rows, pty.cols, 0);
    parser.process(&attach.replay);
    parser.screen().contents()
}

/// The last `max` bytes of `text`, on a character boundary.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[…{start} earlier bytes]\n{}", &text[start..])
}

/// Terminal output as plain text: escape sequences and carriage returns removed.
fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => skip_escape(&mut chars),
            '\r' | '\x07' => {}
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Skip one escape sequence after ESC: CSI up to its final byte, OSC up to BEL or ST,
/// else the single following character.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_command_replaces_the_shell() {
        assert_eq!(script("python3 -q"), "exec python3 -q");
        assert_eq!(script("cd app && npm init"), "cd app && npm init");
        assert_eq!(script("psql $DATABASE_URL"), "psql $DATABASE_URL");
    }

    #[test]
    fn keys_map_to_terminal_bytes() {
        let bytes = keystrokes(&json!({"input": "print(1)", "keys": ["enter", "ctrl-c", "up"]}));
        assert_eq!(bytes.unwrap(), b"print(1)\r\x03\x1b[A");
        assert!(keystrokes(&json!({"keys": ["ctrl-1"]})).is_err());
        assert!(keystrokes(&json!({"keys": ["hyper"]})).is_err());
        assert!(keystrokes(&json!({})).is_err());
        assert!(keystrokes(&json!({"input": "x".repeat(MAX_INPUT_BYTES + 1)})).is_err());
    }

    #[test]
    fn plain_text_drops_escape_sequences_and_carriage_returns() {
        let raw = b"\x1b[?2004h>>> print(2+2)\r\n\x1b[1;32m4\x1b[0m\r\n\x1b]0;title\x07>>> ";
        assert_eq!(plain(raw), ">>> print(2+2)\n4\n>>> ");
        assert_eq!(plain(b"a\x1b]8;;x\x1b\\b"), "ab");
    }

    #[test]
    fn tail_keeps_the_end_on_a_character_boundary() {
        assert_eq!(tail("short", 10), "short");
        let cut = tail("ééé", 3);
        assert!(cut.ends_with('é') && !cut.ends_with("éé"), "{cut}");
    }

    #[test]
    fn read_spec_bounds_its_numbers() {
        let spec = ReadSpec::parse(&json!({"wait_ms": 10_000_000, "max_bytes": 0})).unwrap();
        assert_eq!(spec.wait, Duration::from_millis(MAX_WAIT_MS));
        assert_eq!(spec.max_bytes, 1);
        assert!(ReadSpec::parse(&json!({"mode": "pixels"})).is_err());
    }
}
