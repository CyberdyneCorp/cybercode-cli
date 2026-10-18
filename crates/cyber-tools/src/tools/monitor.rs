//! `monitor` (`background-tasks` → Monitor tool, `builtin-tools` → monitor tool): a
//! process Job that watches a command's output, a WebSocket or a file tail and queues the
//! lines matching `filter` into its Session (`source: monitor`), at most
//! `max_lines_per_minute`, until `until` matches, the timeout passes or it is stopped.
//! The Job's completion notice is the final notice.

use std::process::Stdio;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use cyber_server::runtime::{JobIo, RetrySafety, ToolDef};
use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};
use regex::Regex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::background::{SharedSink, Sink};
use super::process::{Process, ReadStream};
use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::sandboxing::{NetworkAsk, Prepared};

mod tail;

const DEFAULT_RATE: u64 = 30;
const MAX_RATE: u64 = 1000;
const DEFAULT_TIMEOUT_S: u64 = 3600;
const MAX_TIMEOUT_S: u64 = 7 * 24 * 3600;
/// The rate limit's window.
const WINDOW: Duration = Duration::from_secs(60);
/// Characters of one line kept in a queued message.
const MAX_LINE: usize = 2000;
/// How often a file source looks for new bytes, and at most how many it reads per look.
const POLL: Duration = Duration::from_millis(200);
const POLL_BYTES: u64 = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes a regex may compile to; patterns come from the model.
const REGEX_LIMIT: usize = 1 << 20;

pub(crate) struct Monitor;

impl Tool for Monitor {
    fn def(&self) -> ToolDef {
        def(
            "monitor",
            "Watch a source in the background and receive matching lines as queued messages, without blocking or polling. Give exactly one source: command (stdout and stderr of a shell command, e.g. `npm run dev`), url (a ws:// or wss:// WebSocket) or path (new lines appended to a file). filter is a regex (default: every line); at most max_lines_per_minute lines (default 30) are delivered and the rest are summarized as `N lines suppressed`. The monitor ends when until (a regex) matches a line, after timeout_seconds (default 3600) or when task_stop stops it; a final notice then arrives. Returns the job_id.",
            json!({"type": "object", "additionalProperties": false, "properties": {
                "command": {"type": "string"}, "url": {"type": "string"}, "path": {"type": "string"},
                "filter": {"type": "string"}, "until": {"type": "string"},
                "max_lines_per_minute": {"type": "integer", "minimum": 1},
                "timeout_seconds": {"type": "integer", "minimum": 1},
                "description": {"type": "string"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let spec = Spec::parse(&ctx.inv.input)?;
            let feed = open(ctx, &spec.source).await?;
            let admitted = super::background::admit(ctx, &spec.source.label(), "monitor").await;
            let admitted = match admitted {
                Ok(admitted) => admitted,
                Err(error) => {
                    feed.close().await;
                    return Err(error);
                }
            };
            let rules = spec.rules;
            let job = super::background::launch_with(admitted, move |io, sink| {
                Box::pin(watch(feed, rules, sink, io))
            })
            .await?;
            Ok(json!({
                "job_id": job.id,
                "status": job.status,
                "output_path": job.output_path,
                "message": "The monitor runs in the background. Matching lines arrive as queued messages from the monitor; do not poll for them. task_stop stops it, and a final notice arrives when it ends.",
            })
            .to_string())
        })
    }
}

/// What to watch.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Command(String),
    Url(url::Url),
    Path(String),
}

impl Source {
    /// The Job's `command`.
    fn label(&self) -> String {
        match self {
            Self::Command(command) => command.clone(),
            Self::Url(url) => url.to_string(),
            Self::Path(path) => format!("tail -F {}", shell_words::quote(path)),
        }
    }
}

/// How lines are delivered.
struct Rules {
    filter: Option<Regex>,
    until: Option<Regex>,
    rate: u64,
    timeout: Duration,
}

struct Spec {
    source: Source,
    rules: Rules,
}

impl Spec {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let source = source(input)?;
        let rules = Rules {
            filter: pattern(input, "filter")?,
            until: pattern(input, "until")?,
            rate: bounded(input, "max_lines_per_minute", DEFAULT_RATE, MAX_RATE)?,
            timeout: Duration::from_secs(bounded(
                input,
                "timeout_seconds",
                DEFAULT_TIMEOUT_S,
                MAX_TIMEOUT_S,
            )?),
        };
        Ok(Self { source, rules })
    }
}

fn source(input: &Value) -> Result<Source, ToolError> {
    let given: Vec<(&str, &str)> = ["command", "url", "path"]
        .into_iter()
        .filter_map(|key| {
            let value = text(input, key).trim();
            (!value.is_empty()).then_some((key, value))
        })
        .collect();
    let [(key, value)] = given.as_slice() else {
        return Err(failed(
            "Give exactly one of command, url or path to monitor",
        ));
    };
    match *key {
        "command" => Ok(Source::Command((*value).to_string())),
        "path" => Ok(Source::Path((*value).to_string())),
        _ => {
            let url = url::Url::parse(value).map_err(|e| failed(format!("Invalid url: {e}")))?;
            if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none() {
                return Err(failed("url must be a ws:// or wss:// WebSocket URL"));
            }
            Ok(Source::Url(url))
        }
    }
}

fn pattern(input: &Value, key: &str) -> Result<Option<Regex>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(pattern)) if pattern.is_empty() => Ok(None),
        Some(Value::String(pattern)) => regex::RegexBuilder::new(pattern)
            .size_limit(REGEX_LIMIT)
            .build()
            .map(Some)
            .map_err(|e| failed(format!("{key} is not a valid regex: {e}"))),
        Some(other) => Err(failed(format!("{key} must be a string, got {other}"))),
    }
}

fn bounded(input: &Value, key: &str, default: u64, max: u64) -> Result<u64, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| failed(format!("{key} must be an integer from 1 to {max}"))),
    }
}

/// An authorized, opened source: its lines arrive on `lines`.
struct Feed {
    lines: mpsc::Receiver<String>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// A command source's process; `prepared` keeps its sandbox proxy alive.
    process: Option<(Process, Prepared)>,
    asks: Option<mpsc::Receiver<NetworkAsk>>,
}

impl Feed {
    fn of(lines: mpsc::Receiver<String>, tasks: Vec<tokio::task::JoinHandle<()>>) -> Self {
        Self {
            lines,
            tasks,
            process: None,
            asks: None,
        }
    }

    /// Stop the source; a command's process tree gets TERM, then KILL.
    async fn close(mut self) {
        self.stop().await;
    }

    async fn stop(&mut self) {
        if let Some((process, _)) = &mut self.process {
            super::background::stop_tree(process).await;
        }
        for task in &self.tasks {
            task.abort();
        }
    }

    /// A command's exit code once its output closed, waiting a little for it.
    async fn exit_code(&mut self) -> Option<i32> {
        let (process, _) = self.process.as_mut()?;
        match tokio::time::timeout(Duration::from_secs(5), process.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => None,
        }
    }
}

/// Check the source's permission (`bash` for commands and files, `network` for URLs)
/// and open it.
async fn open(ctx: &Ctx<'_>, source: &Source) -> Result<Feed, ToolError> {
    match source {
        Source::Command(command) => open_command(ctx, command).await,
        Source::Path(path) => open_path(ctx, path, &source.label()).await,
        Source::Url(url) => open_url(ctx, url).await,
    }
}

async fn open_command(ctx: &Ctx<'_>, command: &str) -> Result<Feed, ToolError> {
    let analysis = crate::bash_analysis::analyze(command, &ctx.location);
    super::bash::authorize(ctx, command, &ctx.location, &analysis, false).await?;
    let shell = super::bash::shell(&ctx.host.opts.shell, &ctx.location)?;
    let mut prepared = crate::sandboxing::prepare_shell(ctx, &shell, command, false).await?;
    let mut process = super::bash::spawn(ctx, &prepared, &ctx.location, Stdio::piped()).await?;
    let (tx, lines) = mpsc::channel(256);
    let tasks = [process.stdout(), process.stderr()]
        .into_iter()
        .flatten()
        .map(|stream| tokio::spawn(read_lines(stream, tx.clone())))
        .collect();
    let asks = prepared.asks.take();
    Ok(Feed {
        process: Some((process, prepared)),
        asks,
        ..Feed::of(lines, tasks)
    })
}

/// A file tail: the `bash` permission for `tail -F <path>`, the external-directory check
/// and the sandbox's `deny_read`, as for `read`, for the path and the file it resolves
/// to. Its directory must exist; each open re-checks that the path names that file.
async fn open_path(ctx: &Ctx<'_>, raw: &str, label: &str) -> Result<Feed, ToolError> {
    let path = ctx.resolve(raw);
    let hidden = crate::sandboxing::hidden(ctx);
    let target = tail::Target::new(path.clone(), hidden.clone()).map_err(failed)?;
    let resolved = target.expected().to_path_buf();
    for checked in [&path, &resolved] {
        if hidden.denies(checked) {
            return Err(failed(crate::sandboxing::hidden_read(ctx, checked).await));
        }
    }
    let analysis = crate::bash_analysis::analyze(label, &ctx.location);
    super::bash::authorize(ctx, label, &ctx.location, &analysis, false).await?;
    ctx.check_external(&[path, resolved.clone()]).await?;
    let (tx, lines) = mpsc::channel(256);
    let start = std::fs::metadata(&resolved).map_or(0, |meta| meta.len());
    Ok(Feed::of(lines, vec![tokio::spawn(tail(target, start, tx))]))
}

/// A WebSocket: refused when the sandbox turns the network off or denies the domain,
/// else the `network` permission for the host, which `bypass` still asks for (stricter
/// than the proxy's allowlist, which passes allowlisted hosts at once).
async fn open_url(ctx: &Ctx<'_>, url: &url::Url) -> Result<Feed, ToolError> {
    let host = url.host_str().unwrap_or_default().to_string();
    if crate::sandboxing::network_off(ctx) || crate::sandboxing::effective(ctx).domain_denied(&host)
    {
        return Err(failed(format!("Sandbox denied: network {host}")));
    }
    if !crate::sandboxing::answer(ctx, host.clone()).await {
        return Err(failed(format!("Network access to {host} was not allowed")));
    }
    let connect = tokio_tungstenite::connect_async(url.as_str());
    let (socket, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| failed(format!("Timed out connecting to {url}")))?
        .map_err(|e| failed(format!("Could not connect to {url}: {e}")))?;
    let (tx, lines) = mpsc::channel(256);
    Ok(Feed::of(lines, vec![tokio::spawn(read_socket(socket, tx))]))
}

/// Why a monitor ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Until,
    Timeout,
    Closed,
    Stopped,
}

/// The monitor Job: read lines, log them, queue the matching ones under the rate limit.
async fn watch(
    mut feed: Feed,
    rules: Rules,
    sink: SharedSink,
    io: JobIo,
) -> Result<Option<i32>, String> {
    let io = Sink::attach(&sink, io);
    let mut gate = Gate::new(rules.rate);
    let end = follow(&mut feed, &rules, &mut gate, &sink, &io).await;
    gate.flush(&io).await;
    let command = feed.process.is_some();
    let code = match end {
        End::Closed if command => feed.exit_code().await,
        End::Stopped => None,
        _ => Some(0),
    };
    feed.stop().await;
    if end != End::Stopped {
        // Only a command has an exit code worth naming; a socket or file just closed.
        let exit = code.filter(|_| command);
        let note = format!("[monitor ended: {}]\n", reason(end, &rules, exit));
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(note.as_bytes());
    }
    Ok(code)
}

/// Handle lines until the monitor ends, summarizing suppressed lines as windows close
/// and refusing the sandbox proxy's network questions: no person can answer them.
async fn follow(
    feed: &mut Feed,
    rules: &Rules,
    gate: &mut Gate,
    sink: &SharedSink,
    io: &Arc<JobIo>,
) -> End {
    let deadline = tokio::time::sleep(rules.timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = feed.lines.recv() => {
                let Some(line) = line else { return End::Closed };
                if observe(&line, rules, gate, sink, io).await {
                    return End::Until;
                }
            }
            () = &mut deadline => return End::Timeout,
            () = io.stop.cancelled() => return End::Stopped,
            () = gate.due() => gate.flush(io).await,
            Some((_, reply)) = super::bash::next_ask(&mut feed.asks) => { let _ = reply.send(false); }
        }
    }
}

fn reason(end: End, rules: &Rules, code: Option<i32>) -> String {
    match (end, code) {
        (End::Until, _) => "until matched".into(),
        (End::Timeout, _) => format!("timed out after {} s", rules.timeout.as_secs()),
        (End::Closed, Some(code)) => format!("the command exited with code {code}"),
        _ => "the source closed".into(),
    }
}

/// Log one line and queue it when it matches; `true` when `until` matched.
async fn observe(
    line: &str,
    rules: &Rules,
    gate: &mut Gate,
    sink: &SharedSink,
    io: &Arc<JobIo>,
) -> bool {
    sink.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .write(format!("{line}\n").as_bytes());
    if rules
        .filter
        .as_ref()
        .is_none_or(|filter| filter.is_match(line))
    {
        let verdict = gate.offer(Instant::now());
        if let Some(count) = verdict.summary {
            queue(io, suppressed(io.job_id(), count, gate.rate)).await;
        }
        if verdict.send {
            queue(io, format!("Monitor {}: {line}", io.job_id())).await;
        }
    }
    rules
        .until
        .as_ref()
        .is_some_and(|until| until.is_match(line))
}

async fn queue(io: &JobIo, text: String) {
    if let Err(error) = io.queue("monitor", text).await {
        cyber_core::log::error(
            "monitor",
            &format!("Could not queue a monitor line: {error}"),
            json!({ "job_id": io.job_id() }),
        );
    }
}

fn suppressed(job_id: &str, count: u64, rate: u64) -> String {
    format!("Monitor {job_id}: {count} lines suppressed (max_lines_per_minute is {rate})")
}

/// The rate limit: at most `rate` lines per 60 s window; the rest are counted and
/// summarized when the window ends.
struct Gate {
    rate: u64,
    window: Option<Instant>,
    sent: u64,
    suppressed: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    /// Lines suppressed in the window that just ended, to summarize first.
    summary: Option<u64>,
    send: bool,
}

impl Gate {
    fn new(rate: u64) -> Self {
        Self {
            rate,
            window: None,
            sent: 0,
            suppressed: 0,
        }
    }

    fn offer(&mut self, now: Instant) -> Verdict {
        let mut summary = None;
        if self.window.is_none_or(|start| now >= start + WINDOW) {
            summary = self.take();
            self.window = Some(now);
            self.sent = 0;
        }
        let send = self.sent < self.rate;
        if send {
            self.sent += 1;
        } else {
            self.suppressed += 1;
        }
        Verdict { summary, send }
    }

    fn take(&mut self) -> Option<u64> {
        let count = std::mem::take(&mut self.suppressed);
        (count > 0).then_some(count)
    }

    /// When the current window's suppressed lines are due for their summary.
    async fn due(&self) {
        match self.window.filter(|_| self.suppressed > 0) {
            Some(start) => tokio::time::sleep_until(start + WINDOW).await,
            None => std::future::pending().await,
        }
    }

    async fn flush(&mut self, io: &JobIo) {
        if let Some(count) = self.take() {
            queue(io, suppressed(io.job_id(), count, self.rate)).await;
        }
        self.window = None;
    }
}

/// Splits bytes into lines, keeping a partial line until its newline arrives.
#[derive(Default)]
struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(chunk);
        let Some(last) = self.partial.iter().rposition(|byte| *byte == b'\n') else {
            return Vec::new();
        };
        let rest = self.partial.split_off(last + 1);
        let complete = std::mem::replace(&mut self.partial, rest);
        complete[..last]
            .split(|byte| *byte == b'\n')
            .map(clean)
            .collect()
    }

    fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.partial);
        (!rest.is_empty()).then(|| clean(&rest))
    }
}

/// One line as text: CR dropped, at most [`MAX_LINE`] characters.
fn clean(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end_matches('\r');
    if text.chars().count() <= MAX_LINE {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX_LINE).collect();
    cut.push('…');
    cut
}

async fn send_all(tx: &mpsc::Sender<String>, lines: Vec<String>) -> bool {
    for line in lines {
        if tx.send(line).await.is_err() {
            return false;
        }
    }
    true
}

async fn read_lines(mut stream: ReadStream, tx: mpsc::Sender<String>) {
    let mut lines = Lines::default();
    let mut buf = vec![0_u8; 8192];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 || !send_all(&tx, lines.push(&buf[..n])).await {
            break;
        }
    }
    if let Some(line) = lines.finish() {
        let _ = tx.send(line).await;
    }
}

/// New bytes appended to the target from `offset`, like `tail -F`: a missing file is
/// waited for and a truncated or replaced file is read from its start. The tail ends when
/// the path stops naming the validated file.
async fn tail(target: tail::Target, offset: u64, tx: mpsc::Sender<String>) {
    let target = Arc::new(target);
    let mut lines = Lines::default();
    let mut state = tail::State::from(offset);
    loop {
        tokio::time::sleep(POLL).await;
        let polled = tokio::task::spawn_blocking({
            let target = target.clone();
            move || target.poll(state, POLL_BYTES)
        })
        .await;
        let (next, bytes) = match polled {
            Ok(Ok(polled)) => polled,
            Ok(Err(reason)) => {
                cyber_core::log::warn("monitor", &reason, Value::Null);
                return;
            }
            Err(_) => return,
        };
        state = next;
        if !send_all(&tx, lines.push(&bytes)).await {
            return;
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn read_socket(mut socket: Socket, tx: mpsc::Sender<String>) {
    use tokio_tungstenite::tungstenite::Message;
    while let Some(Ok(message)) = socket.next().await {
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Ping(payload) => {
                let _ = socket.send(Message::Pong(payload)).await;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };
        let mut lines = Lines::default();
        let mut found = lines.push(text.as_bytes());
        found.extend(lines.finish());
        if !send_all(&tx, found).await {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_source_and_valid_patterns_and_bounds_are_required() {
        assert!(Spec::parse(&json!({})).is_err());
        assert!(Spec::parse(&json!({"command": "x", "path": "y"})).is_err());
        assert!(Spec::parse(&json!({"url": "http://example.com"})).is_err());
        assert!(Spec::parse(&json!({"command": "x", "filter": "("})).is_err());
        assert!(Spec::parse(&json!({"command": "x", "max_lines_per_minute": 0})).is_err());
        assert!(Spec::parse(&json!({"command": "x", "timeout_seconds": "1"})).is_err());
        let spec = Spec::parse(&json!({"url": "wss://example.com/feed"})).unwrap();
        assert!(matches!(spec.source, Source::Url(_)));
        assert_eq!(spec.rules.rate, 30);
        assert_eq!(spec.rules.timeout, Duration::from_secs(3600));
        let spec = Spec::parse(&json!({"path": "app.log", "until": "done"})).unwrap();
        assert_eq!(spec.source.label(), "tail -F app.log");
        assert!(spec.rules.until.unwrap().is_match("all done"));
    }

    #[test]
    fn gate_allows_the_rate_per_window_and_summarizes_the_rest_when_it_ends() {
        let mut gate = Gate::new(2);
        let start = Instant::now();
        let sent: Vec<bool> = (0..5).map(|_| gate.offer(start).send).collect();
        assert_eq!(sent, [true, true, false, false, false]);
        let later = start + WINDOW;
        assert_eq!(
            gate.offer(later),
            Verdict {
                summary: Some(3),
                send: true
            }
        );
        assert_eq!(gate.take(), None);
    }

    #[test]
    fn lines_keep_partial_input_and_truncate_long_lines() {
        let mut lines = Lines::default();
        assert_eq!(lines.push(b"ERROR one\r\nERR"), ["ERROR one"]);
        assert_eq!(lines.push(b"OR two\n"), ["ERROR two"]);
        assert_eq!(lines.push(b"tail"), Vec::<String>::new());
        assert_eq!(lines.finish().as_deref(), Some("tail"));
        let long = "x".repeat(MAX_LINE + 5);
        assert_eq!(clean(long.as_bytes()).chars().count(), MAX_LINE + 1);
    }
}
