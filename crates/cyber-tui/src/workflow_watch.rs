//! Workflow run indicators and notifications (`tui` → Workflow run indicators, Workflow
//! run notifications): the unsettled runs of the Session's Location, read from
//! `GET /workflows/runs` whatever the screen shows, and the one-line notice each
//! attempt's end gets.
//!
//! The list is patched by `workflow.run.progress` and read again on a run's lifecycle
//! events and when the event stream reconnects. Runs whose events asked for a read but
//! that the read does not list belong to another Location, as in the monitor, and their
//! progress asks for no further read. Whether an end notifies the Location goes by the
//! run's own directory: the read lists only active runs, so a run that ended before it
//! is not listed either.
//!
//! An attempt's end is noticed from its durable event, or, for a run that was active at
//! the previous read and is no longer, from that read (a run that ended or was paused by
//! a restart while the stream was down). Each end is keyed by run, status and end time,
//! so a reconnect's resync never shows a notice twice; a restart's pause is keyed until
//! the run is resumed.

use std::collections::HashSet;

use serde_json::Value;

use crate::app::{Action, App};
use crate::settings::Loadable;
use crate::workflows::{Request, Summary, printable};

/// Sidebar rows before `+N more`.
pub const SIDEBAR_RUNS: usize = 5;
/// Transcript notices kept per TUI.
const MAX_NOTICES: usize = 100;

/// `tui.workflow_notifications`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Runs of the current Session and runs this TUI acted on.
    #[default]
    Own,
    /// Every run of the Location.
    Location,
    Off,
}

impl Scope {
    pub const VALUES: &'static [&'static str] = &["own", "location", "off"];

    pub fn parse(value: &str) -> Self {
        match value {
            "location" => Self::Location,
            "off" => Self::Off,
            _ => Self::Own,
        }
    }
}

/// A notice added to its owner Session's transcript, after the item that was last
/// when it came.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub session_id: String,
    /// `None` before the first item.
    pub after: Option<String>,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Watch {
    pub runs: Loadable<Vec<Summary>>,
    /// The Location the runs are of.
    directory: String,
    /// A read is due once the one in flight answers.
    again: bool,
    unknown: HashSet<String>,
    foreign: HashSet<String>,
    /// Runs some read listed in this Location.
    seen: HashSet<String>,
    /// Runs this TUI paused, resumed or stopped.
    acted: HashSet<String>,
    /// Attempt ends already noticed: `<run>:<status>:<ended_ms>`.
    notified: HashSet<String>,
    pub notices: Vec<Notice>,
    /// The progress line's value the runs last gave.
    pub(crate) fed: Option<crate::progress::WorkflowProgress>,
}

impl Summary {
    /// Shown by the indicators: running, queued, or paused with agents still settling.
    pub fn indicated(&self) -> bool {
        matches!(self.status.as_str(), "running" | "waiting_input" | "queued")
            || self.settling() > 0
    }

    /// An attempt's end that gets a notice: a terminal status, or a restart's pause.
    fn ended(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "budget_exceeded" | "stopped"
        ) || (self.status == "paused" && self.interrupted)
    }

    /// Who noticed this end: one key per attempt.
    fn end_key(&self) -> String {
        if self.status == "paused" {
            return format!("{}:interrupted", self.id);
        }
        format!("{}:{}:{}", self.id, self.status, self.ended_ms.unwrap_or(0))
    }
}

impl Watch {
    pub fn runs(&self) -> &[Summary] {
        self.runs.data.as_deref().unwrap_or_default()
    }

    /// The runs the indicators show.
    pub fn indicated(&self) -> impl Iterator<Item = &Summary> {
        self.runs().iter().filter(|run| run.indicated())
    }

    pub fn acted(&mut self, run_id: &str) {
        self.acted.insert(run_id.to_owned());
    }

    fn list(&mut self) -> Vec<Action> {
        if self.runs.loading {
            self.again = true;
            return Vec::new();
        }
        vec![Action::Workflow(Request::Watch {
            generation: self.runs.start(),
            directory: self.directory.clone(),
        })]
    }

    /// Read the runs of `directory` again, forgetting another Location's.
    pub(crate) fn reload(&mut self, directory: &str) -> Vec<Action> {
        if self.directory != directory {
            self.directory = directory.to_owned();
            self.runs.forget();
            self.again = false;
            self.unknown.clear();
            self.foreign.clear();
            self.seen.clear();
        }
        self.list()
    }

    fn listed(&self, run_id: &str) -> bool {
        self.runs().iter().any(|run| run.id == run_id)
    }

    /// A run the list does not show: read it again unless it belongs to another
    /// Location; a started run is always looked for.
    fn unlisted(&mut self, run_id: &str, started: bool) -> Vec<Action> {
        if self.foreign.contains(run_id) && !started {
            return Vec::new();
        }
        self.foreign.remove(run_id);
        self.unknown.insert(run_id.to_owned());
        self.list()
    }

    fn progress(&mut self, data: &Value) -> Vec<Action> {
        let Some(run_id) = data["run_id"].as_str() else {
            return Vec::new();
        };
        let patched = self
            .runs
            .data
            .as_mut()
            .and_then(|runs| runs.iter_mut().find(|run| run.id == run_id))
            .map(|run| run.patch(data));
        match patched {
            Some(false) => Vec::new(),
            Some(true) => self.list(),
            None if matches!(
                data["status"].as_str(),
                Some("queued" | "running" | "waiting_input")
            ) =>
            {
                self.unlisted(run_id, false)
            }
            None => Vec::new(),
        }
    }

    /// A lifecycle event of `run_id` reads the list again.
    fn lifecycle(&mut self, kind: &str, run_id: &str) -> Vec<Action> {
        if kind.starts_with("workflow.run.resumed") || kind.starts_with("workflow.run.dispatched") {
            self.notified.remove(&format!("{run_id}:interrupted"));
        }
        // A run that starts an attempt is looked for even when an earlier read did not
        // list it: it may have ended then.
        let starting = ["started", "resumed", "dispatched"]
            .iter()
            .any(|verb| kind.starts_with(&format!("workflow.run.{verb}")));
        if self.listed(run_id) {
            self.list()
        } else {
            self.unlisted(run_id, starting)
        }
    }

    /// The runs that were indicated before a read and that it no longer shows active:
    /// those it lists ended are noticed at once, the others are read one by one.
    fn left(&self, before: &[Summary]) -> (Vec<Summary>, Vec<Summary>) {
        let mut ended = Vec::new();
        let mut gone = Vec::new();
        for run in before.iter().filter(|run| run.indicated()) {
            match self.runs().iter().find(|now| now.id == run.id) {
                Some(now) if now.ended() => ended.push(now.clone()),
                Some(_) => {}
                None => gone.push(run.clone()),
            }
        }
        (ended, gone)
    }
}

/// `21m`, `41s`, `1h 05m`.
fn elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

/// The notice of an ended attempt.
pub fn notice_text(run: &Summary) -> String {
    let name = &run.name;
    match run.status.as_str() {
        "completed" => {
            let n = run.agents.total;
            let agents = if n == 1 { "agent" } else { "agents" };
            format!(
                "✓ {name} completed · {n} {agents} · ${:.2} · {}",
                run.cost_usd,
                elapsed(run.elapsed_ms)
            )
        }
        "failed" => {
            let why = run.error.as_ref().or(run.reason.as_ref());
            match why {
                Some(why) => format!("✗ {name} failed: {why}"),
                None => format!("✗ {name} failed"),
            }
        }
        "budget_exceeded" => {
            let limit = run
                .reason
                .as_deref()
                .map(|r| r.strip_prefix("BudgetExceeded: ").unwrap_or(r))
                .or(run.error.as_deref());
            match limit {
                Some(limit) => format!("$ {name} budget exceeded: {limit}"),
                None => format!("$ {name} budget exceeded"),
            }
        }
        "stopped" => format!("■ {name} stopped"),
        _ => format!("⏸ {name} paused by restart"),
    }
}

impl App {
    /// `tui.workflow_notifications`.
    fn notice_scope(&self) -> Scope {
        Scope::parse(self.prefs.get("workflow_notifications"))
    }

    /// Whether `run`'s end notifies here.
    fn notifies(&self, run: &Summary) -> bool {
        let w = &self.watch;
        match self.notice_scope() {
            Scope::Off => false,
            Scope::Own => run.session_id == self.session.id || w.acted.contains(&run.id),
            Scope::Location => {
                w.seen.contains(&run.id)
                    || run.session_id == self.session.id
                    || (!run.directory.is_empty() && run.directory == w.directory)
            }
        }
    }

    /// Read the indicators' runs again: the stream reconnected, or the Session or its
    /// Location changed.
    pub(crate) fn watch_reload(&mut self) -> Vec<Action> {
        let directory = self.session.directory.clone();
        self.watch.reload(&directory)
    }

    /// A `workflow.*` event, whatever the screen shows.
    pub(crate) fn watch_event(&mut self, event: &cyber_client::Event) -> Vec<Action> {
        if event.kind == "workflow.run.progress" {
            return self.watch.progress(&event.data);
        }
        let Some(run_id) = event.session_id.as_deref().filter(|_| event.seq.is_some()) else {
            return Vec::new();
        };
        if !event.kind.starts_with("workflow.run.") {
            return Vec::new();
        }
        let mut actions = self.watch.lifecycle(&event.kind, run_id);
        let mut run = Summary::parse(&event.data);
        if let Some(run) = run.as_mut() {
            run.id = run_id.to_owned();
            actions.extend(self.end_seen(run.clone()));
        }
        actions
    }

    /// An attempt may have ended: read the run for its notice, unless it does not
    /// notify here or its end was noticed already.
    fn end_seen(&mut self, run: Summary) -> Vec<Action> {
        if !run.ended() || self.watch.notified.contains(&run.end_key()) || !self.notifies(&run) {
            return Vec::new();
        }
        vec![Action::Workflow(Request::Ended {
            directory: self.watch.directory.clone(),
            run_id: run.id.clone(),
            fallback: Box::new(run),
        })]
    }

    pub(crate) fn watch_listed(
        &mut self,
        generation: u64,
        directory: &str,
        result: Result<Vec<Summary>, String>,
    ) -> Vec<Action> {
        let w = &mut self.watch;
        if directory != w.directory {
            return Vec::new();
        }
        let before = w.runs.data.clone().unwrap_or_default();
        let ok = result.is_ok();
        w.runs.apply(generation, result);
        let mut actions = Vec::new();
        if !w.runs.loading && ok {
            let unknown = std::mem::take(&mut w.unknown);
            let missing: Vec<String> = unknown.into_iter().filter(|id| !w.listed(id)).collect();
            w.foreign.extend(missing);
            let ids: Vec<String> = w.runs().iter().map(|run| run.id.clone()).collect();
            w.seen.extend(ids);
            let (ended, gone) = w.left(&before);
            for run in ended {
                self.watch_ended(run);
            }
            for run in gone {
                actions.extend(self.end_seen_gone(run));
            }
        }
        if std::mem::take(&mut self.watch.again) {
            actions.extend(self.watch.list());
        }
        actions
    }

    /// A run that was active and the list no longer shows: read how it ended.
    fn end_seen_gone(&mut self, run: Summary) -> Vec<Action> {
        if self.notice_scope() == Scope::Off {
            return Vec::new();
        }
        vec![Action::Workflow(Request::Ended {
            directory: self.watch.directory.clone(),
            run_id: run.id.clone(),
            fallback: Box::new(run),
        })]
    }

    /// The run as read after its end: its notice, once per attempt.
    pub(crate) fn watch_ended(&mut self, run: Summary) {
        if !run.ended() || !self.notifies(&run) || !self.watch.notified.insert(run.end_key()) {
            return;
        }
        let mut run = run;
        run.name = printable(&run.name);
        self.workflow_notice(&run, notice_text(&run));
    }

    /// Into the owner's transcript when it is open, else a toast; and a desktop
    /// notification while the terminal is unfocused.
    fn workflow_notice(&mut self, run: &Summary, text: String) {
        if run.session_id == self.session.id {
            let notices = &mut self.watch.notices;
            if notices.len() == MAX_NOTICES {
                notices.remove(0);
            }
            notices.push(Notice {
                session_id: run.session_id.clone(),
                after: self.items.last().map(|item| item.id().to_owned()),
                text: text.clone(),
            });
        } else {
            self.toast(text.clone());
        }
        if !self.focused {
            self.attention(&text);
        }
    }

    /// Indicators from the runs, before every frame: the sidebar rows and the progress
    /// line's known progress.
    pub(crate) fn sync_workflow_indicators(&mut self) {
        let session = self.session.id.as_str();
        let (own, other): (Vec<&Summary>, Vec<&Summary>) = self
            .watch
            .indicated()
            .partition(|run| run.session_id == session);
        let ordered: Vec<&Summary> = own.into_iter().chain(other).collect();
        self.sidebar.workflows = ordered
            .iter()
            .take(SIDEBAR_RUNS)
            .map(|run| crate::sidebar::WorkflowRow {
                id: run.id.clone(),
                name: run.name.clone(),
                done: run.agents.completed,
                total: run.agents.total,
                queue_position: run.queue_position,
            })
            .collect();
        self.sidebar.workflows_more = ordered.len().saturating_sub(SIDEBAR_RUNS);
        if let Some(focus) = self.sidebar.focus {
            self.sidebar.focus = Some(focus.min(self.sidebar.rows().len().saturating_sub(1)));
        }
        let running: Vec<&Summary> = ordered
            .iter()
            .copied()
            .filter(|run| run.session_id == session && run.active())
            .collect();
        let progress = match running.as_slice() {
            [] => None,
            [run] => Some(crate::progress::WorkflowProgress {
                label: run.name.clone(),
                done: run.agents.completed,
                total: run.agents.total,
            }),
            runs => Some(crate::progress::WorkflowProgress {
                label: format!("{} workflows", runs.len()),
                done: runs.iter().map(|run| run.agents.completed).sum(),
                total: runs.iter().map(|run| run.agents.total).sum(),
            }),
        };
        // Set only when the runs change it, so another source's value stands until then.
        if progress != self.watch.fed {
            self.watch.fed.clone_from(&progress);
            self.progress.set_workflow(progress);
        }
    }

    /// The `workflows` status line field: `⚙ 2 workflows · 1 queued`.
    pub fn workflows_field(&self) -> Option<String> {
        let runs: Vec<&Summary> = self.watch.indicated().collect();
        let n = runs.len();
        if n == 0 {
            return None;
        }
        let queued = runs.iter().filter(|run| run.status == "queued").count();
        let noun = if n == 1 { "workflow" } else { "workflows" };
        let mut text = format!("⚙ {n} {noun}");
        if queued > 0 {
            text.push_str(&format!(" · {queued} queued"));
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(id: &str, status: &str, done: u64, total: u64) -> Summary {
        Summary::parse(
            &json!({"id": id, "name": id, "status": status, "session_id": "ses_1",
            "directory": "/w", "agents": {"total": total, "completed": done,
            "running": total - done}, "cost_usd": 4.1, "elapsed_ms": 1_260_000,
            "ended_ms": 5}),
        )
        .unwrap()
    }

    #[test]
    fn notices_follow_the_spec_forms() {
        let mut done = run("audit", "completed", 100, 100);
        assert_eq!(
            notice_text(&done),
            "✓ audit completed · 100 agents · $4.10 · 21m"
        );
        done.status = "stopped".into();
        assert_eq!(notice_text(&done), "■ audit stopped");
        let failed = Summary::parse(&json!({"id": "r", "name": "review", "status": "failed",
            "error": {"kind": "AgentFailed", "message": "agent agt_1 failed"}}))
        .unwrap();
        assert_eq!(
            notice_text(&failed),
            "✗ review failed: AgentFailed: agent agt_1 failed"
        );
        let over = Summary::parse(
            &json!({"id": "r", "name": "scan", "status": "budget_exceeded",
            "reason": "BudgetExceeded: max_cost_usd 10"}),
        )
        .unwrap();
        assert_eq!(
            notice_text(&over),
            "$ scan budget exceeded: max_cost_usd 10"
        );
        let paused = Summary::parse(&json!({"id": "r", "name": "docs", "status": "paused",
            "interrupted": true}))
        .unwrap();
        assert!(paused.ended());
        assert_eq!(notice_text(&paused), "⏸ docs paused by restart");
        assert_eq!(elapsed(41_000), "41s");
        assert_eq!(elapsed(3_900_000), "1h 05m");
    }

    #[test]
    fn a_user_pause_is_not_an_end_and_each_attempt_has_its_own_key() {
        let mut paused = run("a", "paused", 1, 2);
        assert!(!paused.ended());
        paused.interrupted = true;
        assert_eq!(paused.end_key(), "a:interrupted");
        let mut first = run("a", "failed", 1, 2);
        let key = first.end_key();
        first.ended_ms = Some(9);
        assert_ne!(first.end_key(), key, "a later attempt's end notifies again");
    }

    #[test]
    fn the_field_counts_indicated_runs_and_queued_ones() {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        assert_eq!(app.workflows_field(), None);
        let mut runs: Vec<Summary> = (0..8)
            .map(|i| run(&format!("r{i}"), "running", 0, 1))
            .collect();
        runs.push(run("q1", "queued", 0, 0));
        runs.push(run("q2", "queued", 0, 0));
        let mut settled = run("p", "paused", 1, 1);
        settled.agents.running = 0;
        runs.push(settled);
        runs.push(run("one", "running", 0, 1));
        let generation = app.watch.runs.start();
        app.watch.runs.apply(generation, Ok(runs[..10].to_vec()));
        assert_eq!(
            app.workflows_field().as_deref(),
            Some("⚙ 10 workflows · 2 queued")
        );
        let generation = app.watch.runs.start();
        app.watch.runs.apply(generation, Ok(runs[10..].to_vec()));
        assert_eq!(app.workflows_field().as_deref(), Some("⚙ 1 workflow"));
    }

    #[test]
    fn the_progress_line_sums_the_sessions_running_runs() {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        app.session.id = "ses_1".into();
        let mut other = run("theirs", "running", 1, 9);
        other.session_id = "ses_2".into();
        let generation = app.watch.runs.start();
        app.watch.runs.apply(
            generation,
            Ok(vec![
                other,
                run("audit", "running", 30, 100),
                run("review", "running", 12, 30),
                run("later", "queued", 0, 0),
            ]),
        );
        app.sync_workflow_indicators();
        let rows: Vec<&str> = app
            .sidebar
            .workflows
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(rows, ["audit", "review", "later", "theirs"], "own first");
        match app.activity() {
            Some(crate::progress::Activity::Workflow(w)) => {
                assert_eq!(
                    (w.label.as_str(), w.done, w.total),
                    ("2 workflows", 42, 130)
                );
            }
            other => panic!("{other:?}"),
        }
        let generation = app.watch.runs.start();
        app.watch.runs.apply(generation, Ok(Vec::new()));
        app.sync_workflow_indicators();
        assert!(app.activity().is_none() && app.sidebar.workflows.is_empty());
    }
}
