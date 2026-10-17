//! `cyber workflows runs` (`workflows` → Workflow CLI, Run monitor): the run summaries as
//! a table, and `--watch`, which keeps the table current from the instance stream.
//!
//! `--watch` patches a run from its `workflow.run.progress` events and reads the list
//! again after any other `workflow.*` event (a run started, dispatched or ended moves the
//! queue). Without a stream it reads the list every second. On a terminal it redraws the
//! table in place; otherwise it prints one line (one JSON object with `--format json`)
//! per run whose values changed. Ctrl-C exits 0 and leaves every run as it is.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::pin::Pin;
use std::time::{Duration, Instant};

use cyber_client::{Client, Event};
use futures::{Stream, StreamExt};
use serde_json::Value;

use super::{api, queue_position};
use crate::cli::GlobalArgs;
use crate::error::CliError;
use crate::output;

/// How often a watch reads the list again after events, and redraws a terminal.
const TICK: Duration = Duration::from_millis(250);
/// How often a watch without a live stream reads the list.
const POLL: Duration = Duration::from_secs(1);
/// Printed on stderr when a watch has no live stream.
const FALLBACK: &str = "live events unavailable; reading the list every second";
const HEADER: [&str; 8] = [
    "ID",
    "NAME",
    "STATUS",
    "PHASE",
    "AGENTS d/r/q/f",
    "TOKENS",
    "COST",
    "ELAPSED",
];

/// Which runs `runs` lists.
pub struct RunsQuery {
    /// Every Location instead of the command's.
    pub all: bool,
    /// Comma-separated statuses.
    pub status: Option<String>,
}

impl RunsQuery {
    fn path(&self) -> String {
        let mut query = Vec::new();
        if self.all {
            query.push("directory=all".to_owned());
        }
        if let Some(status) = &self.status {
            query.push(format!("status={}", encode(status)));
        }
        if query.is_empty() {
            "/workflows/runs".into()
        } else {
            format!("/workflows/runs?{}", query.join("&"))
        }
    }

    /// Whether a run patched by a progress event still belongs to the list.
    fn admits(&self, run: &Value) -> bool {
        self.status.as_deref().is_none_or(|statuses| {
            let status = run["status"].as_str().unwrap_or_default();
            statuses.split(',').any(|s| s.trim() == status)
        })
    }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b',' | b'-' | b'_' | b'.' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn fetch(client: &Client, query: &RunsQuery) -> Result<Vec<Value>, CliError> {
    let listed = client.get(&query.path()).await.map_err(api)?;
    Ok(listed["data"].as_array().cloned().unwrap_or_default())
}

/// `runs` and `runs --watch`.
pub async fn runs(
    client: &Client,
    global: &GlobalArgs,
    query: RunsQuery,
    watch: bool,
) -> Result<(), CliError> {
    if watch {
        return watch_runs(client, global, &query).await;
    }
    let runs = fetch(client, &query).await?;
    if output::is_json(global.format) {
        return output::json(&runs);
    }
    if runs.is_empty() {
        println!("{}", empty(&query));
    }
    print!("{}", table(&runs, Duration::ZERO));
    Ok(())
}

fn empty(query: &RunsQuery) -> &'static str {
    if query.all {
        "No Workflow Runs."
    } else {
        "No Workflow Runs in this Location."
    }
}

/// Text from the server with control characters and bidi overrides removed, so a run's
/// name or phase cannot move the cursor or reorder the line.
fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

fn field(value: &Value) -> String {
    value.as_str().map_or_else(|| "-".into(), printable)
}

fn tokens_text(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..10_000 => format!("{:.1}K", tokens as f64 / 1e3),
        10_000..1_000_000 => format!("{}K", tokens / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1e6),
    }
}

fn elapsed_text(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (hours, minutes) = (seconds / 3_600, seconds / 60 % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m{:02}s", seconds % 60)
    }
}

fn counts_text(agents: &Value) -> String {
    let count = |key: &str| agents[key].as_u64().unwrap_or(0);
    format!(
        "{}/{}/{}/{}",
        count("completed"),
        count("running"),
        count("queued"),
        count("failed")
    )
}

/// Whether a run's active time grows while it is listed.
fn active(run: &Value) -> bool {
    matches!(run["status"].as_str(), Some("running" | "waiting_input"))
}

/// One table row; `since` is the time since the list was read, added to an active run's
/// elapsed time so a redraw keeps counting.
fn row(run: &Value, since: Duration) -> [String; 8] {
    let head = [
        field(&run["id"]),
        field(&run["name"]),
        field(&run["status"]),
        field(&run["phase"]),
    ];
    let tail = match queue_position(run) {
        Some(n) => [format!("#{n} in queue"), "-".into(), "-".into(), "-".into()],
        None => {
            let extra = if active(run) { since } else { Duration::ZERO };
            let elapsed = run["elapsed_ms"].as_u64().unwrap_or(0) + extra.as_millis() as u64;
            [
                counts_text(&run["agents"]),
                tokens_text(run["tokens"].as_u64().unwrap_or(0)),
                format!("${:.2}", run["cost_usd"].as_f64().unwrap_or(0.0)),
                elapsed_text(elapsed),
            ]
        }
    };
    let [a, b, c, d] = head;
    let [e, f, g, h] = tail;
    [a, b, c, d, e, f, g, h]
}

/// The runs as an aligned table with a header, or nothing when there is no run.
fn table(runs: &[Value], since: Duration) -> String {
    if runs.is_empty() {
        return String::new();
    }
    let rows: Vec<[String; 8]> = runs.iter().map(|run| row(run, since)).collect();
    let mut widths = HEADER.map(|title| title.chars().count());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let header = HEADER.map(str::to_owned);
    std::iter::once(&header)
        .chain(&rows)
        .map(|cells| line(cells, &widths))
        .collect()
}

fn line(cells: &[String; 8], widths: &[usize; 8]) -> String {
    let mut text = String::new();
    for (cell, width) in cells.iter().zip(widths) {
        text.push_str(&format!("{cell:width$}  "));
    }
    format!("{}\n", text.trim_end())
}

/// What a change line compares: the values a progress event carries, plus the queue
/// position. Elapsed time is left out, so a running run is not reprinted every read.
fn change_key(run: &Value) -> String {
    let mut cells = row(run, Duration::ZERO);
    cells[7].clear();
    cells.join("\u{1f}")
}

/// The state of `runs --watch`.
struct Watch {
    runs: Vec<Value>,
    read_at: Instant,
    terminal: bool,
    json: bool,
    /// The last change key printed per run (non-terminal output).
    printed: HashMap<String, String>,
    /// The table last drawn on the terminal.
    drawn: String,
}

impl Watch {
    fn replace(&mut self, runs: Vec<Value>) {
        self.runs = runs;
        self.read_at = Instant::now();
    }

    /// Patch a run from a `workflow.run.progress` event; a run not in the list (or that
    /// left the filter) asks for the list again.
    fn patch(&mut self, data: &Value, query: &RunsQuery) -> bool {
        let id = data["run_id"].as_str().unwrap_or_default();
        let Some(index) = self.runs.iter().position(|run| run["id"] == id) else {
            return true;
        };
        let run = &mut self.runs[index];
        for key in ["status", "phase", "agents", "tokens", "cost_usd"] {
            run[key] = data.get(key).cloned().unwrap_or(Value::Null);
        }
        if !query.admits(run) || run["status"] == "queued" {
            return true;
        }
        false
    }

    fn show(&mut self) -> Result<(), CliError> {
        if self.terminal {
            return self.redraw();
        }
        for run in &self.runs {
            let id = run["id"].as_str().unwrap_or_default().to_owned();
            let key = change_key(run);
            if self.printed.get(&id) == Some(&key) {
                continue;
            }
            self.printed.insert(id, key);
            if self.json {
                // NDJSON: one compact object per line.
                println!("{run}");
            } else {
                println!("{}", row(run, Duration::ZERO).join("  "));
            }
        }
        Ok(())
    }

    /// Draw the table over the one drawn before.
    fn redraw(&mut self) -> Result<(), CliError> {
        let mut table = table(&self.runs, self.read_at.elapsed());
        if self.runs.is_empty() {
            table = "No Workflow Runs.\n".into();
        }
        if table == self.drawn {
            return Ok(());
        }
        let mut out = std::io::stdout().lock();
        let up = self.drawn.lines().count();
        // Auto-wrap is off while drawing, so a line wider than the terminal is cut and
        // every row stays one line to move back over.
        let mut frame = String::from("\x1b[?7l");
        if up > 0 {
            frame.push_str(&format!("\x1b[{up}F"));
        }
        frame.push_str("\x1b[J");
        frame.push_str(&table);
        frame.push_str("\x1b[?7h");
        out.write_all(frame.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|error| CliError::runtime(error.to_string()))?;
        self.drawn = table;
        Ok(())
    }
}

type Events = Pin<Box<dyn Stream<Item = Event> + Send>>;

/// The next live event; never ready without a stream.
async fn next_event(events: &mut Option<Events>) -> Option<Event> {
    match events {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

/// What a live event asks of the watch.
enum Effect {
    None,
    Show,
    Read,
}

fn effect(watch: &mut Watch, event: &Event, query: &RunsQuery) -> Effect {
    if event.kind == "workflow.run.progress" {
        if watch.patch(&event.data, query) {
            return Effect::Read;
        }
        return Effect::Show;
    }
    if event.kind.starts_with("workflow.") {
        return Effect::Read;
    }
    Effect::None
}

async fn watch_runs(
    client: &Client,
    global: &GlobalArgs,
    query: &RunsQuery,
) -> Result<(), CliError> {
    let mut watch = Watch {
        runs: Vec::new(),
        read_at: Instant::now(),
        terminal: std::io::stdout().is_terminal() && !output::is_json(global.format),
        json: output::is_json(global.format),
        printed: HashMap::new(),
        drawn: String::new(),
    };
    // One listener for the whole watch, registered (by a first poll) before the first
    // read, so an early Ctrl-C still exits 0.
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    if futures::poll!(&mut interrupt).is_ready() {
        return Ok(());
    }
    let mut events: Option<Events> = match client.events(query.all).await {
        Ok(stream) => Some(Box::pin(stream)),
        Err(_) => {
            eprintln!("{FALLBACK}");
            None
        }
    };
    watch.replace(fetch(client, query).await?);
    watch.show()?;
    let mut tick = tokio::time::interval(TICK);
    let mut stale = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut interrupt => return Ok(()),
            event = next_event(&mut events) => match event {
                Some(event) => match effect(&mut watch, &event, query) {
                    Effect::Read => stale = true,
                    Effect::Show => watch.show()?,
                    Effect::None => {}
                },
                // The stream ended: read the list every second from now on.
                None => {
                    eprintln!("{FALLBACK}");
                    events = None;
                }
            },
            _ = tick.tick() => {
                let polling = events.is_none() && watch.read_at.elapsed() >= POLL;
                if stale || polling {
                    watch.replace(fetch(client, query).await?);
                    stale = false;
                }
                watch.show()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(id: &str, status: &str) -> Value {
        json!({
            "id": id, "name": "audit", "status": status, "phase": "Verify",
            "agents": {"completed": 31, "running": 16, "queued": 53, "failed": 0},
            "tokens": 1_234_567, "cost_usd": 3.414, "elapsed_ms": 724_000,
        })
    }

    #[test]
    fn rows_show_counts_spend_and_elapsed_or_the_queue_position() {
        assert_eq!(
            row(&summary("run_a", "running"), Duration::ZERO),
            [
                "run_a",
                "audit",
                "running",
                "Verify",
                "31/16/53/0",
                "1.2M",
                "$3.41",
                "12m04s"
            ]
        );
        // A redraw keeps counting an active run's time, not an ended one's.
        assert_eq!(
            row(&summary("run_a", "running"), Duration::from_secs(2))[7],
            "12m06s"
        );
        assert_eq!(
            row(&summary("run_a", "completed"), Duration::from_secs(2))[7],
            "12m04s"
        );
        let queued =
            json!({"id": "run_q", "name": "migrate", "status": "queued", "queue_position": 1});
        assert_eq!(
            row(&queued, Duration::ZERO),
            [
                "run_q",
                "migrate",
                "queued",
                "-",
                "#1 in queue",
                "-",
                "-",
                "-"
            ]
        );
        assert_eq!(tokens_text(18_000), "18K");
        assert_eq!(tokens_text(1_500), "1.5K");
        assert_eq!(tokens_text(999), "999");
        assert_eq!(elapsed_text(3_725_000), "1h02m");
    }

    #[test]
    fn server_text_cannot_move_the_cursor_or_reorder_the_line() {
        let mut run = summary("run_a", "running");
        run["name"] = json!("evil\x1b[2J\u{202E}name\r\n");
        run["phase"] = json!("\x07Scan");
        let cells = row(&run, Duration::ZERO);
        assert_eq!(cells[1], "evil[2Jname");
        assert_eq!(cells[3], "Scan");
    }

    #[test]
    fn the_table_aligns_its_columns_under_the_header() {
        let text = table(
            &[summary("run_a", "running"), summary("run_bb", "paused")],
            Duration::ZERO,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("ID      NAME   STATUS   PHASE   AGENTS d/r/q/f"),
            "{text}"
        );
        assert!(
            lines[2].starts_with("run_bb  audit  paused   Verify  31/16/53/0"),
            "{text}"
        );
        assert!(table(&[], Duration::ZERO).is_empty());
    }

    #[test]
    fn the_query_names_the_location_scope_and_statuses() {
        let path = |all, status: Option<&str>| {
            RunsQuery {
                all,
                status: status.map(str::to_owned),
            }
            .path()
        };
        assert_eq!(path(false, None), "/workflows/runs");
        assert_eq!(
            path(true, Some("running,queued")),
            "/workflows/runs?directory=all&status=running,queued"
        );
        assert_eq!(path(false, Some("a&b")), "/workflows/runs?status=a%26b");
    }

    #[test]
    fn a_progress_event_patches_its_run_and_a_change_is_printed_once() {
        let query = RunsQuery {
            all: false,
            status: Some("running".into()),
        };
        let mut watch = Watch {
            runs: vec![summary("run_a", "running")],
            read_at: Instant::now(),
            terminal: false,
            json: true,
            printed: HashMap::new(),
            drawn: String::new(),
        };
        let first = change_key(&watch.runs[0]);
        let progress = |status: &str, running: u64| {
            json!({"run_id": "run_a", "status": status, "phase": "Verify",
                   "agents": {"completed": 32, "running": running, "queued": 52, "failed": 0},
                   "tokens": 1_300_000, "cost_usd": 3.5})
        };
        assert!(!watch.patch(&progress("running", 16), &query));
        assert_eq!(watch.runs[0]["agents"]["completed"], 32);
        assert_ne!(change_key(&watch.runs[0]), first);
        // Elapsed time alone is not a change.
        let mut later = watch.runs[0].clone();
        later["elapsed_ms"] = json!(999_999);
        assert_eq!(change_key(&later), change_key(&watch.runs[0]));
        // A run that leaves the filter, or an unknown run, asks for the list again.
        assert!(watch.patch(&progress("paused", 0), &query));
        assert!(watch.patch(&json!({"run_id": "run_z"}), &query));
    }
}
