//! The Workflow monitor's state (`tui` → Workflow monitor): the run summaries of the
//! Location (or of every Location), the selected run's agents, the log, refusals and the
//! pending confirmation, and the child transcript opened from an agent.
//!
//! The list comes from `GET /workflows/runs` and is patched by `workflow.run.progress`.
//! Any other `workflow.*` event of a listed run, a run started anywhere, or a stream
//! that reconnected reads it again; a read already in flight is followed by one more.
//! Every action goes through the public `workflow` routes and acts on one run.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::settings::Loadable;

/// Untrusted text from the server on one line, without control characters or bidi
/// overrides that could move the cursor or reorder a row.
pub fn printable(text: &str) -> String {
    crate::sidebar::clean(text)
        .chars()
        .filter(|c| !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .collect()
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(printable).filter(|s| !s.is_empty())
}

/// `AgentFailed: the agent failed`, from a run's `error`.
fn diagnostic(error: &Value) -> Option<String> {
    let message = text(&error["message"]);
    match (text(&error["kind"]), message) {
        (Some(kind), Some(message)) => Some(format!("{kind}: {message}")),
        (kind, message) => kind.or(message),
    }
}

fn number(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

/// A run's agents by status; `queued` are issued calls waiting for a slot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: u64,
    pub completed: u64,
    pub running: u64,
    pub queued: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub interrupted: u64,
}

impl Counts {
    fn parse(value: &Value) -> Self {
        Self {
            total: number(&value["total"]),
            completed: number(&value["completed"]),
            running: number(&value["running"]),
            queued: number(&value["queued"]),
            failed: number(&value["failed"]),
            cancelled: number(&value["cancelled"]),
            interrupted: number(&value["interrupted"]),
        }
    }
}

/// One `GET /workflows/runs` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub id: String,
    pub name: String,
    /// The owner Session.
    pub session_id: String,
    pub directory: String,
    pub status: String,
    pub phase: Option<String>,
    /// Declared phases, then titles first seen in `phase()`.
    pub phases: Vec<String>,
    pub revision: u64,
    pub agents: Counts,
    pub tokens: u64,
    pub cost_usd: f64,
    /// Active time when the list was read.
    pub elapsed_ms: u64,
    pub queue_position: Option<u64>,
    /// `budget.max_cost_usd`.
    pub budget_usd: Option<f64>,
    /// The inline script's `script_file`, else the saved module's path.
    pub script: Option<String>,
    /// Why the run is paused, stopped or ended early.
    pub reason: Option<String>,
    /// A failed run's diagnostic: `<kind>: <message>`.
    pub error: Option<String>,
    /// When the attempt ended; each attempt's end has its own.
    pub ended_ms: Option<i64>,
    /// Paused by a server restart rather than by a user.
    pub interrupted: bool,
}

impl Summary {
    pub fn parse(value: &Value) -> Option<Self> {
        let id = text(&value["id"])?;
        Some(Self {
            name: text(&value["name"]).unwrap_or_else(|| id.clone()),
            id,
            session_id: value["session_id"].as_str().unwrap_or_default().into(),
            directory: value["directory"].as_str().unwrap_or_default().into(),
            status: text(&value["status"]).unwrap_or_else(|| "unknown".into()),
            phase: text(&value["phase"]),
            phases: value["phases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(text)
                .collect(),
            revision: value["revision"].as_u64().unwrap_or(1),
            agents: Counts::parse(&value["agents"]),
            tokens: number(&value["tokens"]),
            cost_usd: value["cost_usd"].as_f64().unwrap_or(0.0),
            elapsed_ms: number(&value["elapsed_ms"]),
            queue_position: value["queue_position"].as_u64(),
            budget_usd: value["budget"]["max_cost_usd"].as_f64(),
            script: value["script_file"]
                .as_str()
                .or_else(|| value["origin"]["path"].as_str())
                .map(str::to_owned),
            reason: text(&value["reason"]),
            error: diagnostic(&value["error"]),
            ended_ms: value["ended_ms"].as_i64(),
            interrupted: value["interrupted"] == true,
        })
    }

    /// Apply a `workflow.run.progress` event; true when the status changed.
    pub(crate) fn patch(&mut self, progress: &Value) -> bool {
        let status = text(&progress["status"]).unwrap_or_else(|| self.status.clone());
        let changed = status != self.status;
        self.status = status;
        self.phase = text(&progress["phase"]);
        self.agents = Counts::parse(&progress["agents"]);
        self.tokens = number(&progress["tokens"]);
        self.cost_usd = progress["cost_usd"].as_f64().unwrap_or(self.cost_usd);
        if self.status != "queued" {
            self.queue_position = None;
        }
        changed
    }

    /// The run's active time grows while it is listed.
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "running" | "waiting_input")
    }

    /// Agents of a paused run still finishing.
    pub fn settling(&self) -> u64 {
        if self.status == "paused" {
            self.agents.running
        } else {
            0
        }
    }

    /// `paused (6 settling)`, or the status as recorded.
    pub fn status_text(&self) -> String {
        match self.settling() {
            0 => self.status.replace('_', " "),
            n => format!("paused ({n} settling)"),
        }
    }
}

/// One agent of the run view (`agents_detail`).
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub id: String,
    /// The child Session, once created.
    pub session_id: Option<String>,
    pub phase: Option<String>,
    pub label: Option<String>,
    /// The tier the call named, else its model.
    pub tier: Option<String>,
    pub branch: Option<String>,
    pub status: String,
    pub tokens: u64,
    pub cost: f64,
    /// From start to settlement, or to when the run was read for a running agent.
    pub duration_ms: Option<u64>,
    /// Compactions of the agent's Session.
    pub compactions: u64,
}

impl Agent {
    fn parse(value: &Value, now_ms: i64) -> Option<Self> {
        let started = value["started_ms"].as_i64();
        let ended = value["ended_ms"].as_i64().or(Some(now_ms));
        Some(Self {
            id: text(&value["id"])?,
            session_id: value["session_id"].as_str().map(str::to_owned),
            phase: text(&value["phase"]),
            label: text(&value["label"]),
            tier: text(&value["tier"]).or_else(|| text(&value["model"])),
            branch: text(&value["branch"]),
            status: text(&value["status"]).unwrap_or_else(|| "unknown".into()),
            tokens: number(&value["tokens"]),
            cost: value["cost"].as_f64().unwrap_or(0.0),
            duration_ms: started
                .zip(ended)
                .map(|(start, end)| end.saturating_sub(start).max(0) as u64),
            compactions: number(&value["compactions"]),
        })
    }
}

/// `GET /workflows/runs/{id}`: the summary and every agent.
#[derive(Debug, Clone, PartialEq)]
pub struct RunDetail {
    pub summary: Summary,
    pub agents: Vec<Agent>,
}

impl RunDetail {
    pub fn parse(value: &Value, now_ms: i64) -> Option<Self> {
        Some(Self {
            summary: Summary::parse(value)?,
            agents: value["agents_detail"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|agent| Agent::parse(agent, now_ms))
                .collect(),
        })
    }

    /// The agents grouped by phase, in the run's phase order; agents without a phase
    /// come last. The run's queued calls belong to its current phase.
    pub fn groups(&self) -> Vec<Group> {
        let mut titles: Vec<Option<String>> =
            self.summary.phases.iter().cloned().map(Some).collect();
        for agent in &self.agents {
            if !titles.contains(&agent.phase) && agent.phase.is_some() {
                titles.push(agent.phase.clone());
            }
        }
        if self.agents.iter().any(|a| a.phase.is_none()) {
            titles.push(None);
        }
        let current = self
            .summary
            .phase
            .clone()
            .or_else(|| titles.last().cloned().flatten());
        titles
            .into_iter()
            .map(|title| self.group(title, current.as_deref()))
            .collect()
    }

    fn group(&self, title: Option<String>, current: Option<&str>) -> Group {
        let agents: Vec<usize> = (0..self.agents.len())
            .filter(|&i| self.agents[i].phase == title)
            .collect();
        let count = |status: &str| {
            agents
                .iter()
                .filter(|&&i| self.agents[i].status == status)
                .count()
        };
        let queued = if title.as_deref() == current {
            self.summary.agents.queued
        } else {
            0
        };
        Group {
            done: count("completed"),
            running: count("running"),
            total: agents.len() + queued as usize,
            queued,
            title,
            agents,
        }
    }

    /// Agent indexes in the order the run view lists them.
    pub fn order(&self) -> Vec<usize> {
        self.groups().into_iter().flat_map(|g| g.agents).collect()
    }
}

/// One phase of the run view.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// `None` for agents outside any phase.
    pub title: Option<String>,
    pub done: usize,
    pub total: usize,
    pub running: usize,
    pub queued: u64,
    /// Indexes into [`RunDetail::agents`].
    pub agents: Vec<usize>,
}

/// The run view of one run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunView {
    pub run_id: String,
    /// The selected row of [`RunDetail::order`].
    pub selected: usize,
    pub detail: Loadable<RunDetail>,
    again: bool,
}

impl RunView {
    /// The selected agent; it stays selected when the run is read again.
    pub fn agent(&self) -> Option<&Agent> {
        let detail = self.detail.data.as_ref()?;
        detail
            .order()
            .get(self.selected)
            .map(|&index| &detail.agents[index])
    }
}

/// The selected run's `log()` lines.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogView {
    pub run_id: String,
    pub name: String,
    pub lines: Loadable<Vec<String>>,
    pub scroll: u16,
}

/// An action that waits for `y`.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    Stop {
        run_id: String,
        name: String,
    },
    /// The script or configuration changed: relaunch as this revision.
    Revision {
        run_id: String,
        revision: u64,
    },
}

/// A child transcript opened from the run view, and the Session to come back to.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub owner: String,
    pub child: String,
    pub agent_id: String,
}

/// A run operation over the public routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pause,
    /// `revision: true` accepts a changed script as a new revision.
    Resume {
        revision: bool,
    },
    Stop,
    /// Every run a restart interrupted (`resume_interrupted`).
    ResumeInterrupted,
    /// Save an inline run's script into `.cyber/workflows/` (`force` replaces a file).
    Save {
        force: bool,
    },
}

/// Monitor reads and writes, performed by [`perform`].
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    List {
        generation: u64,
        directory: String,
        all: bool,
    },
    Detail {
        generation: u64,
        directory: String,
        run_id: String,
    },
    Op {
        directory: String,
        /// `None` for [`Op::ResumeInterrupted`].
        run_id: Option<String>,
        op: Op,
        all: bool,
    },
    Log {
        directory: String,
        run_id: String,
    },
    /// The child Session of a running agent, which its record does not name yet: the
    /// child of the run's owner Session (`parent`) whose subagent name is the agent ID.
    /// `owner` is the Session the TUI shows, where Esc returns.
    Thread {
        owner: String,
        parent: String,
        agent_id: String,
    },
    /// The unsettled runs of a Location, for the indicators.
    Watch {
        generation: u64,
        directory: String,
    },
    /// One run whose attempt may have ended, for its notice; `fallback` is what its
    /// event said, used when the read fails.
    Ended {
        directory: String,
        run_id: String,
        fallback: Box<Summary>,
    },
    /// The Location's Workflow library, for the Library tab.
    Library {
        generation: u64,
        directory: String,
    },
    /// Start a library Workflow for the open Session.
    StartRun {
        directory: String,
        session: String,
        name: String,
        args: Value,
    },
}

/// Why the server refused an operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub message: String,
    /// The error's `reason`, such as `WorkflowReplayMismatch`.
    pub reason: Option<String>,
}

/// A [`Request`]'s outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    List {
        generation: u64,
        result: Result<Vec<Summary>, String>,
    },
    Detail {
        generation: u64,
        run_id: String,
        result: Result<Box<RunDetail>, String>,
    },
    Op {
        run_id: Option<String>,
        op: Op,
        result: Result<Value, Refusal>,
    },
    Log {
        run_id: String,
        result: Result<Vec<String>, String>,
    },
    Thread {
        owner: String,
        agent_id: String,
        result: Result<String, String>,
    },
    Watch {
        generation: u64,
        directory: String,
        result: Result<Vec<Summary>, String>,
    },
    Ended {
        result: Result<Box<Summary>, String>,
        fallback: Box<Summary>,
    },
    Library {
        generation: u64,
        /// The entries and the number of modules that failed to load.
        result: Result<(Vec<crate::workflow_library::Entry>, usize), String>,
    },
    Started {
        name: String,
        result: Result<Value, Refusal>,
    },
}

#[derive(Debug, Default)]
pub struct WorkflowsState {
    pub runs: Loadable<Vec<Summary>>,
    /// When the list was read; active runs' elapsed time counts on from it.
    pub read_at: Option<Instant>,
    pub selected: usize,
    /// Every Location instead of [`Self::directory`].
    pub all: bool,
    /// The Location the monitor lists: the Session's when it opened.
    pub directory: String,
    pub run: Option<RunView>,
    pub log: Option<LogView>,
    /// The last refusal of each run, shown on its row.
    pub refusals: BTreeMap<String, String>,
    pub confirm: Option<Confirm>,
    pub thread: Option<Thread>,
    /// The Library tab is shown instead of the runs.
    pub library_tab: bool,
    pub library: crate::workflow_library::Library,
    /// A list read is due once the one in flight answers.
    again: bool,
    /// Runs whose events asked for a read; those still missing after it belong to
    /// another Location and are ignored.
    unknown: HashSet<String>,
    foreign: HashSet<String>,
}

impl WorkflowsState {
    pub fn summaries(&self) -> &[Summary] {
        self.runs.data.as_deref().unwrap_or_default()
    }

    pub fn current(&self) -> Option<&Summary> {
        self.summaries().get(self.selected)
    }

    /// The run whose bar animates: the selected one, while it runs.
    pub fn animated(&self) -> Option<&Summary> {
        if self.log.is_some() {
            return None;
        }
        let run = match &self.run {
            Some(view) => view.detail.data.as_ref().map(|d| &d.summary),
            None => self.current(),
        };
        run.filter(|run| run.active())
    }

    /// Time since the list was read, for active runs' elapsed time.
    pub fn since(&self, now: Instant) -> Duration {
        self.read_at
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
    }

    fn list(&mut self) -> Vec<Action> {
        if self.runs.loading {
            self.again = true;
            return Vec::new();
        }
        vec![Action::Workflow(Request::List {
            generation: self.runs.start(),
            directory: self.directory.clone(),
            all: self.all,
        })]
    }

    fn detail(&mut self) -> Vec<Action> {
        let directory = self.directory.clone();
        let Some(view) = self.run.as_mut() else {
            return Vec::new();
        };
        if view.detail.loading {
            view.again = true;
            return Vec::new();
        }
        vec![Action::Workflow(Request::Detail {
            generation: view.detail.start(),
            directory,
            run_id: view.run_id.clone(),
        })]
    }

    /// Read the list and the open run again.
    pub(crate) fn reload(&mut self) -> Vec<Action> {
        let mut actions = self.list();
        actions.extend(self.detail());
        actions
    }

    fn apply_list(&mut self, generation: u64, result: Result<Vec<Summary>, String>, now: Instant) {
        let selected = self.current().map(|run| run.id.clone());
        let ok = result.is_ok();
        self.runs.apply(generation, result);
        if self.runs.loading || !ok {
            return;
        }
        self.read_at = Some(now);
        let unknown = std::mem::take(&mut self.unknown);
        let listed: HashSet<&str> = self.summaries().iter().map(|run| run.id.as_str()).collect();
        let missing: Vec<String> = unknown
            .into_iter()
            .filter(|id| !listed.contains(id.as_str()))
            .collect();
        self.foreign.extend(missing);
        let index = selected.and_then(|id| self.summaries().iter().position(|r| r.id == id));
        let last = self.summaries().len().saturating_sub(1);
        self.selected = index.unwrap_or(self.selected).min(last);
    }

    fn apply_detail(&mut self, generation: u64, run_id: &str, result: Result<RunDetail, String>) {
        let Some(view) = self.run.as_mut().filter(|view| view.run_id == run_id) else {
            return;
        };
        let selected = view.agent().map(|agent| agent.id.clone());
        view.detail.apply(generation, result);
        let Some(detail) = view.detail.data.as_ref() else {
            return;
        };
        let order = detail.order();
        let index = selected.and_then(|id| order.iter().position(|&i| detail.agents[i].id == id));
        view.selected = index
            .unwrap_or(view.selected)
            .min(order.len().saturating_sub(1));
    }

    /// A progress event: patch the listed run; a status change or an unknown run reads
    /// the list again.
    fn progress(&mut self, data: &Value) -> Vec<Action> {
        let Some(run_id) = data["run_id"].as_str() else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if let Some(view) = self.run.as_mut().filter(|view| view.run_id == run_id)
            && let Some(detail) = view.detail.data.as_mut()
        {
            detail.summary.patch(data);
            actions.extend(self.detail());
        }
        let patched = self
            .runs
            .data
            .as_mut()
            .and_then(|runs| runs.iter_mut().find(|run| run.id == run_id))
            .map(|run| run.patch(data));
        match patched {
            Some(false) => {}
            Some(true) => actions.extend(self.list()),
            None => actions.extend(self.unlisted(run_id, false)),
        }
        actions
    }

    /// An event of a run the list does not show: read it again unless the run belongs
    /// to another Location; a started run is always looked for.
    fn unlisted(&mut self, run_id: &str, started: bool) -> Vec<Action> {
        if self.foreign.contains(run_id) && !started {
            return Vec::new();
        }
        self.foreign.remove(run_id);
        self.unknown.insert(run_id.to_owned());
        self.list()
    }

    /// A durable `workflow.*` event of `run_id`.
    fn durable(&mut self, kind: &str, run_id: &str) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.run.as_ref().is_some_and(|view| view.run_id == run_id) {
            actions.extend(self.detail());
        }
        let listed = self.summaries().iter().any(|run| run.id == run_id);
        if listed {
            actions.extend(self.list());
        } else {
            actions.extend(self.unlisted(run_id, kind.starts_with("workflow.run.started")));
        }
        actions
    }
}

impl App {
    /// `/workflows`: the runs of the Session's Location.
    pub(crate) fn open_workflows(&mut self) -> Vec<Action> {
        self.completion = None;
        let w = &mut self.workflows;
        if w.directory != self.session.directory {
            w.directory = self.session.directory.clone();
            w.runs.forget();
            w.selected = 0;
            w.foreign.clear();
        }
        w.run = None;
        w.log = None;
        w.confirm = None;
        w.thread = None;
        w.library_tab = false;
        w.library.form = None;
        w.library.source = None;
        self.overlay = Overlay::Workflows;
        self.workflows.reload()
    }

    /// The monitor follows events while it is shown.
    fn workflows_live(&self) -> bool {
        matches!(self.overlay, Overlay::Workflows)
    }

    /// A `workflow.*` event from the instance stream.
    pub(crate) fn workflow_event(&mut self, event: &cyber_client::Event) -> Vec<Action> {
        let mut actions = self.watch_event(event);
        if !self.workflows_live() {
            return actions;
        }
        if event.kind == "workflow.run.progress" {
            actions.extend(self.workflows.progress(&event.data));
            return actions;
        }
        if let Some(run_id) = event.session_id.as_deref().filter(|_| event.seq.is_some()) {
            actions.extend(self.workflows.durable(&event.kind, run_id));
        }
        actions
    }

    /// `/workflows` on one run, from the sidebar's Workflows section.
    pub(crate) fn open_workflow_run(&mut self, run_id: String) -> Vec<Action> {
        let mut actions = self.open_workflows();
        actions.extend(self.open_run(run_id));
        actions
    }

    /// The event stream (re)connected: the monitor reads everything again.
    pub(crate) fn workflows_connected(&mut self) -> Vec<Action> {
        if self.workflows_live() {
            self.workflows.reload()
        } else {
            Vec::new()
        }
    }

    pub(crate) fn workflow_reply(&mut self, reply: Reply) -> Vec<Action> {
        let now = self.now();
        let w = &mut self.workflows;
        match reply {
            Reply::List { generation, result } => {
                w.apply_list(generation, result, now);
                if std::mem::take(&mut w.again) {
                    return w.list();
                }
            }
            Reply::Detail {
                generation,
                run_id,
                result,
            } => {
                w.apply_detail(generation, &run_id, result.map(|detail| *detail));
                if let Some(view) = w.run.as_mut()
                    && std::mem::take(&mut view.again)
                {
                    return w.detail();
                }
            }
            Reply::Op { run_id, op, result } => return self.op_done(run_id, op, result),
            Reply::Watch {
                generation,
                directory,
                result,
            } => return self.watch_listed(generation, &directory, result),
            Reply::Ended { result, fallback } => {
                self.watch_ended(result.map_or(*fallback, |summary| *summary));
            }
            Reply::Thread {
                owner,
                agent_id,
                result,
            } => return self.thread_found(owner, agent_id, result),
            Reply::Library { generation, result } => self.library_listed(generation, result),
            Reply::Started { name, result } => return self.library_started(&name, result),
            Reply::Log { run_id, result } => {
                if let Some(log) = w.log.as_mut().filter(|log| log.run_id == run_id) {
                    let generation = log.lines.start();
                    log.lines.apply(generation, result);
                }
            }
        }
        Vec::new()
    }

    fn op_done(
        &mut self,
        run_id: Option<String>,
        op: Op,
        result: Result<Value, Refusal>,
    ) -> Vec<Action> {
        let w = &mut self.workflows;
        match (result, &run_id) {
            (Ok(report), None) => self.interrupted_done(&report),
            (Ok(saved), Some(id)) if matches!(op, Op::Save { .. }) => {
                w.refusals.remove(id);
                let path = saved["path"].as_str().unwrap_or_default();
                self.toast(format!("Saved {}", printable(path)));
            }
            (Ok(_), Some(id)) => {
                w.refusals.remove(id);
            }
            (Err(refusal), Some(id))
                if op == (Op::Resume { revision: false })
                    && refusal.reason.as_deref() == Some("WorkflowReplayMismatch") =>
            {
                let revision = w
                    .summaries()
                    .iter()
                    .chain(
                        w.run
                            .iter()
                            .filter_map(|v| v.detail.data.as_ref().map(|d| &d.summary)),
                    )
                    .find(|run| &run.id == id)
                    .map_or(1, |run| run.revision);
                w.confirm = Some(Confirm::Revision {
                    run_id: id.clone(),
                    revision: revision + 1,
                });
            }
            (Err(refusal), Some(id)) => {
                w.refusals.insert(id.clone(), printable(&refusal.message));
            }
            (Err(refusal), None) => {
                self.toast(format!("workflows: {}", printable(&refusal.message)))
            }
        }
        self.workflows.reload()
    }

    /// `R`'s report: refused runs show why on their rows.
    fn interrupted_done(&mut self, report: &Value) {
        let resumed = report["resumed"].as_array().map_or(0, Vec::len);
        for id in report["resumed"].as_array().into_iter().flatten() {
            if let Some(id) = id.as_str() {
                self.watch.acted(id);
            }
        }
        for refused in report["refused"].as_array().into_iter().flatten() {
            if let (Some(id), Some(reason)) =
                (refused["run_id"].as_str(), refused["reason"].as_str())
            {
                self.workflows
                    .refusals
                    .insert(id.to_owned(), printable(reason));
            }
        }
        self.toast(match resumed {
            0 => "No interrupted run to resume".to_owned(),
            1 => "Resumed 1 interrupted run".to_owned(),
            n => format!("Resumed {n} interrupted runs"),
        });
    }

    pub(crate) fn workflows_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.workflows.confirm.is_some() {
            return self.confirm_key(key);
        }
        if self.workflows.log.is_some() {
            self.log_key(key);
            return Vec::new();
        }
        if self.workflows.library_tab {
            return self.library_key(key);
        }
        let action = self.keymap.action("workflows", &key, false);
        if self.workflows.run.is_some() {
            return self.run_key(action, key);
        }
        if key.code == KeyCode::Tab {
            return self.toggle_library();
        }
        self.list_key(action, key)
    }

    fn confirm_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(confirm) = self.workflows.confirm.take() else {
            return Vec::new();
        };
        if !matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
            return Vec::new();
        }
        match confirm {
            Confirm::Stop { run_id, .. } => self.run_op(run_id, Op::Stop),
            Confirm::Revision { run_id, .. } => self.run_op(run_id, Op::Resume { revision: true }),
        }
    }

    fn log_key(&mut self, key: KeyEvent) {
        let Some(log) = self.workflows.log.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up => log.scroll = log.scroll.saturating_sub(1),
            KeyCode::Down => log.scroll = log.scroll.saturating_add(1),
            KeyCode::PageUp => log.scroll = log.scroll.saturating_sub(10),
            KeyCode::PageDown => log.scroll = log.scroll.saturating_add(10),
            KeyCode::Esc | KeyCode::Char('q') => self.workflows.log = None,
            _ => {}
        }
    }

    fn list_key(&mut self, action: Option<&str>, key: KeyEvent) -> Vec<Action> {
        let Some(action) = action else {
            self.move_run(key.code);
            if key.code == KeyCode::Esc {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        };
        if action == "workflows_scope" {
            let w = &mut self.workflows;
            w.all = !w.all;
            w.selected = 0;
            w.foreign.clear();
            return w.list();
        }
        if action == "workflows_resume_interrupted" {
            return self.resume_interrupted();
        }
        match self.workflows.current().map(|run| run.id.clone()) {
            Some(run_id) if action == "workflows_open" => self.open_run(run_id),
            Some(run_id) => self.run_action(action, run_id),
            None => Vec::new(),
        }
    }

    fn move_run(&mut self, code: KeyCode) {
        let w = &mut self.workflows;
        let last = w.summaries().len().saturating_sub(1);
        w.selected = step(w.selected, last, code);
    }

    fn open_run(&mut self, run_id: String) -> Vec<Action> {
        self.workflows.run = Some(RunView {
            run_id,
            ..Default::default()
        });
        self.workflows.detail()
    }

    fn run_key(&mut self, action: Option<&str>, key: KeyEvent) -> Vec<Action> {
        let Some(run_id) = self.workflows.run.as_ref().map(|v| v.run_id.clone()) else {
            return Vec::new();
        };
        match (action, key.code) {
            (Some("workflows_open"), _) | (None, KeyCode::Char('t')) => self.open_thread(),
            (Some("workflows_resume_interrupted"), _) => self.resume_interrupted(),
            (Some("workflows_scope"), _) => Vec::new(),
            (Some(action), _) => self.run_action(action, run_id),
            (None, KeyCode::Esc) => {
                self.workflows.run = None;
                self.workflows.list()
            }
            (None, code) => {
                if let Some(view) = self.workflows.run.as_mut() {
                    let last = view
                        .detail
                        .data
                        .as_ref()
                        .map_or(0, |d| d.agents.len().saturating_sub(1));
                    view.selected = step(view.selected, last, code);
                }
                Vec::new()
            }
        }
    }

    /// The summary of `run_id` from the run view or the list.
    fn summary_of(&self, run_id: &str) -> Option<&Summary> {
        let w = &self.workflows;
        w.run
            .iter()
            .filter_map(|view| view.detail.data.as_ref().map(|d| &d.summary))
            .chain(w.summaries())
            .find(|run| run.id == run_id)
    }

    fn run_action(&mut self, action: &str, run_id: String) -> Vec<Action> {
        let Some(run) = self.summary_of(&run_id).cloned() else {
            return Vec::new();
        };
        match action {
            "workflows_pause" if run.status == "paused" => {
                self.run_op(run_id, Op::Resume { revision: false })
            }
            "workflows_pause" => self.run_op(run_id, Op::Pause),
            "workflows_resume" => self.run_op(run_id, Op::Resume { revision: false }),
            "workflows_stop" => {
                self.workflows.confirm = Some(Confirm::Stop {
                    run_id,
                    name: run.name,
                });
                Vec::new()
            }
            "workflows_script" => self.open_script(&run),
            "workflows_save" => self.run_op(run_id, Op::Save { force: false }),
            "workflows_log" => self.open_log(run),
            _ => Vec::new(),
        }
    }

    fn run_op(&mut self, run_id: String, op: Op) -> Vec<Action> {
        self.workflows.refusals.remove(&run_id);
        self.watch.acted(&run_id);
        vec![Action::Workflow(Request::Op {
            directory: self.workflows.directory.clone(),
            run_id: Some(run_id),
            op,
            all: self.workflows.all,
        })]
    }

    fn resume_interrupted(&mut self) -> Vec<Action> {
        vec![Action::Workflow(Request::Op {
            directory: self.workflows.directory.clone(),
            run_id: None,
            op: Op::ResumeInterrupted,
            all: self.workflows.all,
        })]
    }

    fn open_script(&mut self, run: &Summary) -> Vec<Action> {
        match &run.script {
            Some(path) => vec![Action::EditFile(path.clone())],
            None => {
                let refusal = "This run has no script file to open".to_owned();
                self.workflows.refusals.insert(run.id.clone(), refusal);
                Vec::new()
            }
        }
    }

    fn open_log(&mut self, run: Summary) -> Vec<Action> {
        let mut log = LogView {
            run_id: run.id.clone(),
            name: run.name,
            ..Default::default()
        };
        log.lines.start();
        self.workflows.log = Some(log);
        vec![Action::Workflow(Request::Log {
            directory: self.workflows.directory.clone(),
            run_id: run.id,
        })]
    }

    /// Enter or `t` on an agent: its child transcript, live, with Esc back to the run.
    fn open_thread(&mut self) -> Vec<Action> {
        let Some(view) = self.workflows.run.as_ref() else {
            return Vec::new();
        };
        let Some(agent) = view.agent() else {
            return Vec::new();
        };
        let (owner, agent_id) = (self.session.id.clone(), agent.id.clone());
        // The agent is a child of the run's owner, which need not be this Session.
        let parent = view
            .detail
            .data
            .as_ref()
            .map(|d| d.summary.session_id.clone());
        match (agent.session_id.clone(), parent) {
            (Some(child), _) => self.thread_found(owner, agent_id, Ok(child)),
            (None, Some(parent)) => vec![Action::Workflow(Request::Thread {
                owner,
                parent,
                agent_id,
            })],
            (None, None) => Vec::new(),
        }
    }

    fn thread_found(
        &mut self,
        owner: String,
        agent_id: String,
        child: Result<String, String>,
    ) -> Vec<Action> {
        let Some(view) = self
            .workflows
            .run
            .as_ref()
            .filter(|_| self.session.id == owner && matches!(self.overlay, Overlay::Workflows))
        else {
            return Vec::new();
        };
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                let refusal = format!("Agent {agent_id}: {}", printable(&error));
                self.workflows.refusals.insert(view.run_id.clone(), refusal);
                return Vec::new();
            }
        };
        self.toast(format!("Agent {agent_id} · esc returns to the run"));
        self.workflows.thread = Some(Thread {
            owner,
            child: child.clone(),
            agent_id,
        });
        self.overlay = Overlay::None;
        vec![Action::Open(child)]
    }

    /// Esc in an agent's transcript opened from the monitor, unless a completion menu
    /// takes it; `None` lets the key through.
    pub(crate) fn thread_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        if key.code != KeyCode::Esc || self.completion.is_some() || self.workflows.thread.is_none()
        {
            return None;
        }
        self.leave_thread()
    }

    /// Back from an agent's transcript to the owner and the run view, the agent still
    /// selected. `None` when the transcript shown is not the one the monitor opened.
    fn leave_thread(&mut self) -> Option<Vec<Action>> {
        let thread = self.workflows.thread.take()?;
        if thread.child != self.session.id {
            return None;
        }
        self.overlay = Overlay::Workflows;
        let mut actions = vec![Action::Open(thread.owner)];
        actions.extend(self.workflows.reload());
        Some(actions)
    }

    /// Another Session was opened: a transcript left another way forgets its way back.
    pub(crate) fn thread_session(&mut self, session_id: &str) {
        if self
            .workflows
            .thread
            .as_ref()
            .is_some_and(|t| t.child != session_id && t.owner != session_id)
        {
            self.workflows.thread = None;
        }
    }
}

/// Up, Down, Home and End over `0..=last`.
pub(crate) fn step(selected: usize, last: usize, code: KeyCode) -> usize {
    match code {
        KeyCode::Up => selected.saturating_sub(1),
        KeyCode::Down => (selected + 1).min(last),
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => selected,
    }
}

pub(crate) fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b',' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub(crate) fn refusal(error: cyber_client::ClientError) -> Refusal {
    let reason = match &error {
        cyber_client::ClientError::Api { body, .. } => body["reason"].as_str().map(str::to_owned),
        _ => None,
    };
    let message = match &error {
        cyber_client::ClientError::Api { message, .. } if !message.is_empty() => message.clone(),
        _ => error.to_string(),
    };
    Refusal { message, reason }
}

/// One `log()` call as a line: its values, strings as written and others as JSON.
fn log_line(entry: &Value) -> String {
    let values: Vec<String> = entry["values"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect();
    values.join(" ")
}

fn log_lines(page: &Value) -> Vec<String> {
    page["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            log_line(entry)
                .lines()
                .map(crate::sidebar::clean_patch)
                .map(|line| printable_keep_blank(&line))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn printable_keep_blank(line: &str) -> String {
    line.chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

fn op_path(run_id: Option<&str>, op: Op, all: bool) -> (String, Value) {
    let path = |verb: &str| {
        format!(
            "/workflows/runs/{}/{verb}",
            encode(run_id.unwrap_or_default())
        )
    };
    match op {
        Op::Pause => (path("pause"), json!({})),
        Op::Stop => (path("stop"), json!({})),
        Op::Resume { revision } => (path("resume"), json!({ "revision": revision })),
        Op::ResumeInterrupted if all => (
            "/workflows/runs/resume_interrupted?directory=all".into(),
            json!({}),
        ),
        Op::ResumeInterrupted => ("/workflows/runs/resume_interrupted".into(), json!({})),
        Op::Save { force } => (path("save"), json!({ "force": force })),
    }
}

/// The run owner's child Session whose subagent name is `agent_id`, page by page.
async fn child_of(
    client: &cyber_client::Client,
    owner: &str,
    agent_id: &str,
) -> Result<String, String> {
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let after = cursor
            .as_deref()
            .map_or(String::new(), |c| format!("&cursor={}", encode(c)));
        let page = client
            .get(&format!(
                "/sessions/{}/children?limit=200{after}",
                encode(owner)
            ))
            .await
            .map_err(crate::perform::err)?;
        let page = &page["data"];
        let found = page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|thread| {
                thread["session"]["subagent_name"] == agent_id
                    && thread["session"]["parent_id"] == owner
            });
        if let Some(id) = found.and_then(|t| t["session"]["id"].as_str()) {
            return Ok(id.to_owned());
        }
        match page["cursor"]["next"].as_str() {
            Some(next) if Some(next) != cursor.as_deref() => cursor = Some(next.to_owned()),
            _ => break,
        }
    }
    Err("no transcript yet".into())
}

/// Perform a monitor request in its Location.
pub(crate) async fn perform(client: &cyber_client::Client, request: Request) -> Reply {
    use crate::perform::err;
    match request {
        Request::Library {
            generation,
            directory,
        } => crate::workflow_library::list(client, generation, &directory).await,
        Request::StartRun {
            directory,
            session,
            name,
            args,
        } => crate::workflow_library::start(client, &directory, &session, name, args).await,
        Request::List {
            generation,
            directory,
            all,
        } => {
            let path = if all {
                "/workflows/runs?directory=all&limit=200"
            } else {
                "/workflows/runs?limit=200"
            };
            let result = client.at(&directory).get(path).await.map_err(err).map(|v| {
                v["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Summary::parse)
                    .collect()
            });
            Reply::List { generation, result }
        }
        Request::Detail {
            generation,
            directory,
            run_id,
        } => {
            let path = format!("/workflows/runs/{}", encode(&run_id));
            let result = client
                .at(&directory)
                .get(&path)
                .await
                .map_err(err)
                .and_then(|v| {
                    RunDetail::parse(&v["data"], wall_ms())
                        .map(Box::new)
                        .ok_or_else(|| "Invalid run".to_owned())
                });
            Reply::Detail {
                generation,
                run_id,
                result,
            }
        }
        Request::Op {
            directory,
            run_id,
            op,
            all,
        } => {
            let (path, body) = op_path(run_id.as_deref(), op, all);
            let result = client
                .at(&directory)
                .post(&path, body)
                .await
                .map(|mut v| v["data"].take())
                .map_err(refusal);
            Reply::Op { run_id, op, result }
        }
        Request::Thread {
            owner,
            parent,
            agent_id,
        } => Reply::Thread {
            result: child_of(client, &parent, &agent_id).await,
            owner,
            agent_id,
        },
        Request::Watch {
            generation,
            directory,
        } => {
            let path = "/workflows/runs?status=queued,running,waiting_input,paused&limit=200";
            let result = client.at(&directory).get(path).await.map_err(err).map(|v| {
                v["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Summary::parse)
                    .collect()
            });
            Reply::Watch {
                generation,
                directory,
                result,
            }
        }
        Request::Ended {
            directory,
            run_id,
            fallback,
        } => {
            let path = format!("/workflows/runs/{}", encode(&run_id));
            let result = client
                .at(&directory)
                .get(&path)
                .await
                .map_err(err)
                .and_then(|v| {
                    Summary::parse(&v["data"])
                        .map(Box::new)
                        .ok_or_else(|| "Invalid run".to_owned())
                });
            Reply::Ended { result, fallback }
        }
        Request::Log { directory, run_id } => {
            let path = format!("/workflows/runs/{}/logs", encode(&run_id));
            let result = client
                .at(&directory)
                .get(&path)
                .await
                .map_err(err)
                .map(|page| log_lines(&page));
            Reply::Log { run_id, result }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, status: &str) -> Value {
        json!({"id": id, "name": id, "status": status, "session_id": "ses_1",
            "directory": "/w", "phases": ["Scan"], "revision": 1,
            "agents": {"total": 3, "completed": 1, "running": 2, "queued": 0,
                "failed": 0, "cancelled": 0, "interrupted": 0},
            "tokens": 10, "cost_usd": 0.5, "elapsed_ms": 1000})
    }

    #[test]
    fn untrusted_text_loses_controls_and_bidi_overrides() {
        let run = Summary::parse(&json!({"id": "run_1",
            "name": "au\u{1b}[2Jdit\u{202E}evil\nsecond", "status": "running"}))
        .unwrap();
        assert_eq!(run.name, "au[2Jditevil");
    }

    #[test]
    fn paused_runs_show_their_settling_agents() {
        let mut run = Summary::parse(&summary("run_1", "paused")).unwrap();
        assert_eq!(run.status_text(), "paused (2 settling)");
        run.agents.running = 0;
        assert_eq!(run.status_text(), "paused");
        run.status = "budget_exceeded".into();
        assert_eq!(run.status_text(), "budget exceeded");
    }

    #[test]
    fn progress_patches_a_listed_run_and_an_unknown_one_reads_the_list_once_per_location() {
        let mut w = WorkflowsState::default();
        let generation = w.runs.start();
        w.apply_list(
            generation,
            Ok(vec![Summary::parse(&summary("run_a", "running")).unwrap()]),
            Instant::now(),
        );
        let progress = json!({"run_id": "run_a", "status": "running", "phase": "Scan",
            "agents": {"total": 3, "completed": 2, "running": 1}, "tokens": 20, "cost_usd": 1.0});
        assert!(w.progress(&progress).is_empty(), "same status: no read");
        assert_eq!(w.summaries()[0].agents.completed, 2);
        let foreign = json!({"run_id": "run_x", "status": "running", "agents": {}});
        let actions = w.progress(&foreign);
        assert!(matches!(
            actions[..],
            [Action::Workflow(Request::List { .. })]
        ));
        assert!(w.progress(&foreign).is_empty(), "a read is in flight");
        let generation = match &actions[0] {
            Action::Workflow(Request::List { generation, .. }) => *generation,
            _ => unreachable!(),
        };
        w.apply_list(
            generation,
            Ok(vec![Summary::parse(&summary("run_a", "running")).unwrap()]),
            Instant::now(),
        );
        assert!(
            w.progress(&foreign).is_empty(),
            "another Location's run is ignored"
        );
        assert_eq!(w.durable("workflow.run.started.1", "run_x").len(), 1);
    }

    #[test]
    fn agents_group_by_phase_and_queued_calls_join_the_current_phase() {
        let detail = RunDetail::parse(
            &json!({"id": "run_1", "status": "running", "phases": ["Scan", "Verify", "Report"],
            "phase": "Verify", "agents": {"queued": 5},
            "agents_detail": [
                {"id": "agt_2", "phase": "Verify", "status": "running", "started_ms": 1000},
                {"id": "agt_1", "phase": "Scan", "status": "completed", "compactions": 2,
                    "started_ms": 0, "ended_ms": 41000},
                {"id": "agt_3", "status": "failed"},
            ]}),
            5000,
        )
        .unwrap();
        let groups = detail.groups();
        let titles: Vec<_> = groups.iter().map(|g| g.title.clone()).collect();
        assert_eq!(
            titles,
            [
                Some("Scan".into()),
                Some("Verify".into()),
                Some("Report".into()),
                None
            ]
        );
        assert_eq!(
            (
                groups[1].done,
                groups[1].running,
                groups[1].queued,
                groups[1].total
            ),
            (0, 1, 5, 6)
        );
        assert_eq!(detail.order(), [1, 0, 2]);
        assert_eq!(detail.agents[1].duration_ms, Some(41_000));
        assert_eq!(detail.agents[0].duration_ms, Some(4_000));
        assert_eq!(detail.agents[2].duration_ms, None);
        assert_eq!(detail.agents[1].compactions, 2);
    }

    #[test]
    fn operations_use_the_public_routes() {
        assert_eq!(
            op_path(Some("run_1"), Op::Resume { revision: true }, false),
            (
                "/workflows/runs/run_1/resume".into(),
                json!({"revision": true})
            )
        );
        assert_eq!(
            op_path(None, Op::ResumeInterrupted, true).0,
            "/workflows/runs/resume_interrupted?directory=all"
        );
        assert_eq!(
            op_path(Some("a/b"), Op::Stop, false).0,
            "/workflows/runs/a%2Fb/stop"
        );
        assert_eq!(
            op_path(Some("run_1"), Op::Save { force: false }, false),
            ("/workflows/runs/run_1/save".into(), json!({"force": false}))
        );
    }

    #[test]
    fn log_lines_are_split_and_cleaned() {
        let page = json!({"data": [
            {"seq": 1, "time_ms": 0, "values": ["found", 3, {"a": 1}]},
            {"seq": 2, "time_ms": 0, "values": ["two\nlines\u{1b}[2J"]},
        ]});
        assert_eq!(log_lines(&page), ["found 3 {\"a\":1}", "two", "lines[2J"]);
    }
}
