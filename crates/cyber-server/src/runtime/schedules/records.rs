//! Durable schedule records (`loops-scheduling` → Persistent local schedules, Loop events
//! and hooks): the `schedule.*` events of each schedule's own stream, validated on
//! append, and their projector into `schedule` and `schedule_run`, which the sweep, the
//! run lifecycle and the routes read.

use cyber_core::budget::Budget;
use cyber_core::loops::{Missed, Notify};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use cyber_store::{EventRegistry, StoredEvent};

pub const CREATED: &str = "schedule.created.1";
pub const UPDATED: &str = "schedule.updated.1";
pub const RUN: &str = "schedule.run.1";

/// The projection tables, children first, in the order a rebuild clears them.
pub const TABLES: [&str; 2] = ["schedule_run", "schedule"];
/// The event type prefix every schedule record shares.
pub const PREFIX: &str = "schedule.";
/// Runs `show` lists, newest first.
pub const RECENT_RUNS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    Active,
    Paused,
    BudgetExceeded,
    Deleted,
}

impl ScheduleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::BudgetExceeded => "budget_exceeded",
            Self::Deleted => "deleted",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "budget_exceeded" => Self::BudgetExceeded,
            _ => Self::Deleted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Admitted to its Session; it runs when the Session is free.
    Started,
    /// The run's `schedule_wakeup` (`noop` and `reason`).
    Reported,
    Ended,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Completed,
    Failed,
    Interrupted,
    /// The gate refused it: untrusted or missing directory, `bypass` not allowed, a
    /// Session it cannot use.
    Refused,
    /// Its time came while the previous run (or the target Session) was still busy.
    SkippedOverlap,
    /// Its time passed while the service was not running (missed-run policy `skip`).
    SkippedMissed,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Refused => "refused",
            Self::SkippedOverlap => "skipped_overlap",
            Self::SkippedMissed => "skipped_missed",
        }
    }
}

/// `schedule.created.1`: everything a run needs, fixed for the schedule's life.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Created {
    pub schedule_id: String,
    pub name: String,
    pub cron: String,
    pub directory: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub mode: String,
    pub model: String,
    /// Each run resumes this Session; `None` creates a new Session per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub notify: Notify,
    pub missed: Missed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// The fixed offset added to every occurrence (`loops.jitter_seconds`).
    pub jitter_ms: i64,
    pub created_ms: i64,
    pub next_due_ms: i64,
}

/// `schedule.updated.1`: pause, resume, budget stop or removal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Updated {
    pub schedule_id: String,
    pub status: ScheduleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_ms: Option<i64>,
}

/// `schedule.run.1`: a run's start, its reported `noop`, or its end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub schedule_id: String,
    pub run_id: String,
    pub n: u64,
    pub phase: Phase,
    pub due_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    #[serde(default)]
    pub noop: bool,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub turns: u64,
    /// Session totals at admission, so the end reports a delta.
    #[serde(default)]
    pub cost_before: f64,
    #[serde(default)]
    pub tokens_before: u64,
    #[serde(default)]
    pub turns_before: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the next run is due; `None` keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_ms: Option<i64>,
}

impl Run {
    /// A run record of `phase` with nothing measured yet.
    pub fn new(schedule_id: &str, run_id: &str, n: u64, phase: Phase, due_ms: i64) -> Self {
        Self {
            schedule_id: schedule_id.to_owned(),
            run_id: run_id.to_owned(),
            n,
            phase,
            due_ms,
            session_id: None,
            message_id: None,
            outcome: None,
            noop: false,
            duration_ms: 0,
            cost: 0.0,
            tokens: 0,
            turns: 0,
            cost_before: 0.0,
            tokens_before: 0,
            turns_before: 0,
            reason: None,
            next_due_ms: None,
        }
    }
}

/// A schedule as the routes and the CLI show it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub cron: String,
    pub directory: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub mode: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub notify: Notify,
    pub missed: Missed,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    pub jitter_ms: i64,
    pub status: ScheduleStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The next run, jitter included; `None` while paused or ended.
    pub next_due_ms: Option<i64>,
    /// The run in flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflight: Option<String>,
    pub runs: u64,
    pub cost_usd: f64,
    pub tokens: u64,
    pub turns: u64,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// The most recent runs, newest first (`show` only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<RunRecord>,
}

/// One run as `show` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RunRecord {
    pub run_id: String,
    pub n: u64,
    pub due_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// A [`RunOutcome`], or `running`.
    pub outcome: String,
    pub noop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub cost: f64,
    pub tokens: u64,
    pub turns: u64,
    #[serde(skip)]
    pub cost_before: f64,
    #[serde(skip)]
    pub tokens_before: u64,
    #[serde(skip)]
    pub turns_before: u64,
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CREATED, validate_created)
        .and_then(|r| r.register_with(UPDATED, |v| decode::<Updated>(v).map(drop)))
        .and_then(|r| r.register_with(RUN, validate_run))
        .expect("valid schedule events");
}

fn validate_created(data: &Value) -> Result<(), String> {
    let c: Created = decode(data)?;
    if !cyber_core::loops::is_cron_id(&c.schedule_id) {
        return Err(format!("schedule ID {:?} is not a cron_ ID", c.schedule_id));
    }
    cyber_core::cron::Cron::parse(&c.cron)?;
    if c.prompt.trim().is_empty() || c.directory.is_empty() || c.mode.is_empty() {
        return Err("a schedule has a prompt, a directory and a Mode".into());
    }
    if c.jitter_ms < 0 {
        return Err("a schedule's jitter is not negative".into());
    }
    Ok(())
}

fn validate_run(data: &Value) -> Result<(), String> {
    let run: Run = decode(data)?;
    match (run.phase, run.outcome) {
        (Phase::Ended, None) => Err("an ended run has an outcome".into()),
        (Phase::Started | Phase::Reported, Some(_)) => {
            Err("only an ended run has an outcome".into())
        }
        (Phase::Started, None) if run.session_id.is_none() || run.message_id.is_none() => {
            Err("a started run names its Session and message".into())
        }
        _ => Ok(()),
    }
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|error| error.to_string())
}

/// Write `schedule` and `schedule_run` in the event transaction.
pub(crate) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    let result = match e.kind.as_str() {
        CREATED => project_created(tx, &decode(&e.data)?, e.time_ms),
        UPDATED => {
            let u: Updated = decode(&e.data)?;
            tx.execute(
                "UPDATE schedule SET status = ?2, reason = ?3,
                   next_due_ms = COALESCE(?4, next_due_ms), updated_ms = ?5
                 WHERE id = ?1",
                params![
                    u.schedule_id,
                    u.status.as_str(),
                    u.reason,
                    u.next_due_ms,
                    e.time_ms
                ],
            )
            .map(drop)
        }
        RUN => project_run(tx, &decode(&e.data)?, e.time_ms),
        _ => return Ok(()),
    };
    result.map_err(|error| error.to_string())
}

fn project_created(tx: &Transaction<'_>, c: &Created, time_ms: i64) -> rusqlite::Result<()> {
    let spec = serde_json::to_string(c).expect("a schedule serializes");
    tx.execute(
        "INSERT INTO schedule (id, name, cron, directory, spec, status, next_due_ms,
           created_ms, updated_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?8)",
        params![
            c.schedule_id,
            c.name,
            c.cron,
            c.directory,
            spec,
            c.next_due_ms,
            c.created_ms,
            time_ms
        ],
    )
    .map(drop)
}

fn project_run(tx: &Transaction<'_>, r: &Run, time_ms: i64) -> rusqlite::Result<()> {
    match r.phase {
        Phase::Started => {
            tx.execute(
                "INSERT INTO schedule_run (id, schedule_id, n, due_ms, started_ms, session_id,
                   message_id, outcome, cost_before, tokens_before, turns_before)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9, ?10)",
                params![
                    r.run_id,
                    r.schedule_id,
                    int(r.n),
                    r.due_ms,
                    time_ms,
                    r.session_id,
                    r.message_id,
                    r.cost_before,
                    int(r.tokens_before),
                    int(r.turns_before)
                ],
            )?;
            tx.execute(
                "UPDATE schedule SET inflight = ?2, runs = ?3,
                   next_due_ms = COALESCE(?4, next_due_ms), updated_ms = ?5
                 WHERE id = ?1",
                params![r.schedule_id, r.run_id, int(r.n), r.next_due_ms, time_ms],
            )
            .map(drop)
        }
        Phase::Reported => tx
            .execute(
                "UPDATE schedule_run SET noop = ?2, reason = ?3 WHERE id = ?1",
                params![r.run_id, r.noop, r.reason],
            )
            .map(drop),
        Phase::Ended => project_ended(tx, r, time_ms),
    }
}

fn project_ended(tx: &Transaction<'_>, r: &Run, time_ms: i64) -> rusqlite::Result<()> {
    let outcome = r.outcome.map_or("failed", RunOutcome::as_str);
    let updated = tx.execute(
        "UPDATE schedule_run SET outcome = ?2, ended_ms = ?3, noop = ?4, reason = ?5,
           cost_usd = ?6, tokens = ?7, turns = ?8
         WHERE id = ?1",
        params![
            r.run_id,
            outcome,
            time_ms,
            r.noop,
            r.reason,
            r.cost,
            int(r.tokens),
            int(r.turns)
        ],
    )?;
    if updated == 0 {
        // A run that never started: skipped or refused.
        tx.execute(
            "INSERT INTO schedule_run (id, schedule_id, n, due_ms, ended_ms, session_id,
               outcome, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                r.run_id,
                r.schedule_id,
                int(r.n),
                r.due_ms,
                time_ms,
                r.session_id,
                outcome,
                r.reason
            ],
        )?;
    }
    tx.execute(
        "UPDATE schedule SET
           inflight = CASE WHEN inflight = ?2 THEN NULL ELSE inflight END,
           next_due_ms = COALESCE(?3, next_due_ms),
           cost_usd = cost_usd + ?4, tokens = tokens + ?5, turns = turns + ?6, updated_ms = ?7
         WHERE id = ?1",
        params![
            r.schedule_id,
            r.run_id,
            r.next_due_ms,
            r.cost,
            int(r.tokens),
            int(r.turns),
            time_ms
        ],
    )
    .map(drop)
}

/// A count as SQLite stores it.
fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn count(row: &rusqlite::Row<'_>, column: &str) -> rusqlite::Result<u64> {
    Ok(u64::try_from(row.get::<_, i64>(column)?).unwrap_or(0))
}

fn schedule_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    let spec: String = row.get("spec")?;
    let c: Created = serde_json::from_str(&spec).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let status: String = row.get("status")?;
    let status = ScheduleStatus::parse(&status);
    Ok(Schedule {
        id: c.schedule_id,
        name: c.name,
        cron: c.cron,
        directory: c.directory,
        prompt: c.prompt,
        agent: c.agent,
        mode: c.mode,
        model: c.model,
        session_id: c.session_id,
        notify: c.notify,
        missed: c.missed,
        budget: c.budget,
        jitter_ms: c.jitter_ms,
        status,
        reason: row.get("reason")?,
        next_due_ms: if status == ScheduleStatus::Active {
            row.get("next_due_ms")?
        } else {
            None
        },
        inflight: row.get("inflight")?,
        runs: count(row, "runs")?,
        cost_usd: row.get("cost_usd")?,
        tokens: count(row, "tokens")?,
        turns: count(row, "turns")?,
        created_ms: row.get("created_ms")?,
        updated_ms: row.get("updated_ms")?,
        recent: Vec::new(),
    })
}

fn run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        run_id: row.get("id")?,
        n: count(row, "n")?,
        due_ms: row.get("due_ms")?,
        started_ms: row.get("started_ms")?,
        ended_ms: row.get("ended_ms")?,
        session_id: row.get("session_id")?,
        message_id: row.get("message_id")?,
        outcome: row.get("outcome")?,
        noop: row.get("noop")?,
        reason: row.get("reason")?,
        cost: row.get("cost_usd")?,
        tokens: count(row, "tokens")?,
        turns: count(row, "turns")?,
        cost_before: row.get("cost_before")?,
        tokens_before: count(row, "tokens_before")?,
        turns_before: count(row, "turns_before")?,
    })
}

/// One schedule, deleted ones included.
pub(crate) fn load(db: &Connection, id: &str) -> rusqlite::Result<Option<Schedule>> {
    db.query_row("SELECT * FROM schedule WHERE id = ?1", [id], schedule_row)
        .optional()
}

/// Every schedule not removed, oldest first.
pub(crate) fn list(db: &Connection) -> rusqlite::Result<Vec<Schedule>> {
    let mut stmt =
        db.prepare("SELECT * FROM schedule WHERE status != 'deleted' ORDER BY created_ms, id")?;
    stmt.query_map([], schedule_row)?.collect()
}

/// A schedule's runs, newest first.
pub(crate) fn runs(db: &Connection, id: &str, limit: usize) -> rusqlite::Result<Vec<RunRecord>> {
    let mut stmt = db.prepare(
        "SELECT * FROM schedule_run WHERE schedule_id = ?1 ORDER BY n DESC, rowid DESC LIMIT ?2",
    )?;
    stmt.query_map(params![id, limit as i64], run_row)?
        .collect()
}

/// One run.
pub(crate) fn run(db: &Connection, run_id: &str) -> rusqlite::Result<Option<RunRecord>> {
    db.query_row(
        "SELECT * FROM schedule_run WHERE id = ?1",
        [run_id],
        run_row,
    )
    .optional()
}

/// Active schedules due at `now_ms`, oldest due first.
pub(crate) fn due(db: &Connection, now_ms: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT id FROM schedule
         WHERE status = 'active' AND next_due_ms IS NOT NULL AND next_due_ms <= ?1
         ORDER BY next_due_ms, id",
    )?;
    stmt.query_map([now_ms], |r| r.get(0))?.collect()
}

/// Runs in flight in `session_id`: `(schedule, run)`.
pub(crate) fn running_in(
    db: &Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = db.prepare(
        "SELECT schedule_id, id FROM schedule_run
         WHERE session_id = ?1 AND outcome = 'running' ORDER BY n",
    )?;
    stmt.query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// Schedules the restore visits: active ones and any with a run in flight.
pub(crate) fn restorable(db: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT id FROM schedule WHERE status = 'active' OR inflight IS NOT NULL ORDER BY id",
    )?;
    stmt.query_map([], |r| r.get(0))?.collect()
}
