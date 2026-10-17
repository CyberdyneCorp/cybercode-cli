//! Run summaries for the monitor (`workflows` → Run monitor): a run with its agent
//! counts, spend and active time, listed with filters and a cursor.
//!
//! Agent counts come from one `GROUP BY status` over `workflow_agent` plus the
//! scheduler's queued calls; tokens and cost from the run's budget scope; `elapsed_ms`
//! from its active time, so time spent `queued` or `paused` is excluded.

use std::collections::HashMap;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::budget::{RunSpend, run_scope};
use super::workflow_recovery::location_key;
use super::workflows::{WorkflowRun, WorkflowRunStatus};
use super::{Runtime, RuntimeError};

/// `limit` when a list names none.
pub const DEFAULT_RUN_PAGE: u32 = 50;
/// The largest `limit` of one page.
pub const MAX_RUN_PAGE: u32 = 200;

/// The agents of a run by status. `queued` counts issued `agent()` calls waiting for a
/// slot of the server-wide limit; `total` is every recorded agent plus those calls.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct AgentCounts {
    pub total: u64,
    pub completed: u64,
    pub running: u64,
    pub queued: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub interrupted: u64,
}

impl AgentCounts {
    /// Count `count` recorded agents of `status`; an unknown status counts in the total
    /// only.
    fn add(&mut self, status: &str, count: u64) {
        self.total += count;
        let slot = match status {
            "completed" => &mut self.completed,
            "running" => &mut self.running,
            "failed" => &mut self.failed,
            "cancelled" => &mut self.cancelled,
            "interrupted" => &mut self.interrupted,
            _ => return,
        };
        *slot += count;
    }
}

/// A run as the monitor lists it: the run, plus its agent counts, spend and active time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowRunSummary {
    #[serde(flatten)]
    pub run: WorkflowRun,
    pub agents: AgentCounts,
    /// Tokens of the run's agents.
    pub tokens: u64,
    /// Cost of the run's agents.
    pub cost_usd: f64,
    /// Active time: time spent `queued` or `paused` is excluded.
    pub elapsed_ms: u64,
    /// A paused run whose pause a server restart recorded (`workflows` → Resume after
    /// restart); absent otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

/// The data of a `workflow.run.progress` live event: nothing a list does not return.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowRunProgress {
    pub run_id: String,
    pub status: WorkflowRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub agents: AgentCounts,
    pub tokens: u64,
    pub cost_usd: f64,
}

impl From<&WorkflowRunSummary> for WorkflowRunProgress {
    fn from(summary: &WorkflowRunSummary) -> Self {
        Self {
            run_id: summary.run.id.clone(),
            status: summary.run.status,
            phase: summary.run.phase.clone(),
            agents: summary.agents,
            tokens: summary.tokens,
            cost_usd: summary.cost_usd,
        }
    }
}

/// Which runs a list returns.
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    /// Only these statuses; every status when empty.
    pub statuses: Vec<WorkflowRunStatus>,
    pub session_id: Option<String>,
    /// One Location's directory, in any spelling: a run matches when its recorded
    /// directory is the same Location canonically. Every Location when `None`.
    pub directory: Option<String>,
    /// At most this many runs (1–200, default 50).
    pub limit: Option<u32>,
    /// The `next` cursor of the previous page.
    pub cursor: Option<String>,
}

/// One page of runs: active runs first, then the newest.
#[derive(Debug, Clone)]
pub struct RunPage {
    pub runs: Vec<WorkflowRunSummary>,
    /// The cursor of the next page, when there is one.
    pub next: Option<String>,
}

/// Where a page starts: after the run of this sort key.
struct Cursor {
    rank: i64,
    started_ms: i64,
    id: String,
}

impl Cursor {
    fn parse(text: &str) -> Result<Self, RuntimeError> {
        let invalid = || RuntimeError::Invalid("InvalidCursorError: malformed cursor".into());
        let mut parts = text.splitn(3, ':');
        let mut number = || -> Result<i64, RuntimeError> {
            parts
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or_else(invalid)
        };
        let (rank, started_ms) = (number()?, number()?);
        let id = parts
            .next()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        Ok(Self {
            rank,
            started_ms,
            id: id.to_owned(),
        })
    }

    fn of(run: &WorkflowRun) -> String {
        let rank = i64::from(!run.status.is_active());
        format!("{rank}:{}:{}", run.started_ms, run.id)
    }
}

/// Runs of the filter after the cursor, active first and then the newest; one more than
/// a page, to tell whether another follows.
const PAGE_SQL: &str = "SELECT data FROM (
    SELECT data, id, started_ms,
           CASE WHEN status IN ('queued', 'running', 'waiting_input') THEN 0 ELSE 1 END AS rank
    FROM workflow_run
    WHERE (?1 IS NULL OR status IN (SELECT value FROM json_each(?1)))
      AND (?2 IS NULL OR session_id = ?2)
      AND (?3 IS NULL OR json_extract(data, '$.directory') IN (SELECT value FROM json_each(?3))))
  WHERE ?4 IS NULL OR rank > ?4 OR (rank = ?4 AND (started_ms < ?5 OR (started_ms = ?5 AND id < ?6)))
  ORDER BY rank, started_ms DESC, id DESC
  LIMIT ?7";

/// The directories recorded on runs that are the Location `key` (a [`location_key`]),
/// as a JSON array: runs keep their owner's spelling, which a symlink or a `\\?\` prefix
/// may make differ from the canonical one.
fn location_spellings(db: &rusqlite::Connection, key: &str) -> rusqlite::Result<String> {
    let mut statement = db.prepare(
        "SELECT DISTINCT json_extract(data, '$.directory') FROM workflow_run
         WHERE json_extract(data, '$.directory') IS NOT NULL",
    )?;
    let mut spellings = vec![key.to_owned()];
    for directory in statement.query_map([], |row| row.get::<_, String>(0))? {
        let directory = directory?;
        if directory != key && location_key(&directory) == key {
            spellings.push(directory);
        }
    }
    Ok(serde_json::to_string(&spellings).expect("strings serialize"))
}

impl Runtime {
    /// One page of run summaries; a bad cursor or limit is `Invalid`.
    pub fn workflow_run_page(&self, filter: &RunFilter) -> Result<RunPage, RuntimeError> {
        let limit = filter.limit.unwrap_or(DEFAULT_RUN_PAGE);
        if !(1..=MAX_RUN_PAGE).contains(&limit) {
            return Err(RuntimeError::Invalid(format!(
                "limit must be between 1 and {MAX_RUN_PAGE}"
            )));
        }
        let cursor = filter.cursor.as_deref().map(Cursor::parse).transpose()?;
        let statuses = (!filter.statuses.is_empty())
            .then(|| serde_json::to_string(&filter.statuses).expect("statuses serialize"));
        let session = filter.session_id.clone();
        let directory = filter.directory.as_deref().map(location_key);
        let texts = self.inner.store.read(move |db| {
            let directory = directory
                .map(|key| location_spellings(db, &key))
                .transpose()?;
            let mut statement = db.prepare(PAGE_SQL)?;
            let (rank, started, id) = cursor.map_or((None, None, None), |c| {
                (Some(c.rank), Some(c.started_ms), Some(c.id))
            });
            let texts = statement
                .query_map(
                    params![
                        statuses,
                        session,
                        directory,
                        rank,
                        started,
                        id,
                        i64::from(limit) + 1
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(texts)
        })?;
        let mut runs = texts
            .iter()
            .map(|text| {
                serde_json::from_str(text).map_err(|e| RuntimeError::Corrupt(e.to_string()))
            })
            .collect::<Result<Vec<WorkflowRun>, _>>()?;
        let next = (runs.len() > limit as usize).then(|| {
            runs.truncate(limit as usize);
            Cursor::of(runs.last().expect("a full page"))
        });
        Ok(RunPage {
            runs: self.summarize(runs)?,
            next,
        })
    }

    /// The summary of one run.
    pub fn workflow_run_summary(
        &self,
        id: &str,
    ) -> Result<Option<WorkflowRunSummary>, RuntimeError> {
        let Some(run) = self.workflow_run(id)? else {
            return Ok(None);
        };
        Ok(self.summarize(vec![run])?.pop())
    }

    /// Summaries of `runs`, in their order: one `GROUP BY` for the recorded agents, one
    /// budget read per run, and the scheduler's queued calls.
    fn summarize(&self, runs: Vec<WorkflowRun>) -> Result<Vec<WorkflowRunSummary>, RuntimeError> {
        let runs = self.with_queue_positions(runs)?;
        let ids: Vec<String> = runs.iter().map(|run| run.id.clone()).collect();
        let now = chrono::Utc::now().timestamp_millis();
        let (mut counts, spends) = self.inner.store.read(move |db| {
            let mut counts: HashMap<String, AgentCounts> = HashMap::new();
            let mut statement = db.prepare(
                "SELECT run_id, status, COUNT(*) FROM workflow_agent
                 WHERE run_id IN (SELECT value FROM json_each(?1)) GROUP BY run_id, status",
            )?;
            let rows = statement.query_map(
                params![serde_json::to_string(&ids).expect("IDs serialize")],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?;
            for row in rows {
                let (run, status, count) = row?;
                counts
                    .entry(run)
                    .or_default()
                    .add(&status, u64::try_from(count).unwrap_or_default());
            }
            let mut spends = HashMap::new();
            for id in &ids {
                if let Some(scope) = run_scope(db, id, now)? {
                    spends.insert(id.clone(), RunSpend::of(&scope, now));
                }
            }
            Ok((counts, spends))
        })?;
        let mut interrupted = std::collections::HashSet::new();
        for run in runs
            .iter()
            .filter(|r| r.status == WorkflowRunStatus::Paused)
        {
            if self.workflow_run_interrupted(&run.id)? {
                interrupted.insert(run.id.clone());
            }
        }
        let scheduler = self.workflow_scheduler();
        Ok(runs
            .into_iter()
            .map(|mut run| {
                let mut agents = counts.remove(&run.id).unwrap_or_default();
                let queued = scheduler.snapshot(&run.id).queued as u64;
                agents.queued = queued;
                agents.total += queued;
                let spend = spends.get(&run.id).cloned().unwrap_or_default();
                run.share = Some(self.workflow_share(run.share));
                WorkflowRunSummary {
                    interrupted: interrupted.contains(&run.id),
                    run,
                    agents,
                    tokens: spend.tokens,
                    cost_usd: spend.cost,
                    elapsed_ms: (spend.elapsed_seconds * 1000.0).round() as u64,
                }
            })
            .collect())
    }

    /// The progress of one run and the Location its event streams on.
    pub(super) fn workflow_run_progress(
        &self,
        id: &str,
    ) -> Result<Option<(String, WorkflowRunProgress)>, RuntimeError> {
        Ok(self
            .workflow_run_summary(id)?
            .map(|summary| (summary.run.directory.clone(), (&summary).into())))
    }

    /// IDs of the runs that may be executing or settling: queued, running, waiting for
    /// input, or paused.
    pub(super) fn unsettled_workflow_runs(&self) -> Result<Vec<String>, RuntimeError> {
        Ok(self.inner.store.read(|db| {
            let mut statement = db.prepare(
                "SELECT id FROM workflow_run
                 WHERE status IN ('queued', 'running', 'waiting_input', 'paused')",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run recorded before runs kept canonical directories still matches its Location
    /// in any spelling, and another Location's run does not.
    #[cfg(unix)]
    #[test]
    fn recorded_spellings_of_a_location_match_canonically() {
        use cyber_store::{Store, StoredEvent};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (target, link, other) = (root.join("target"), root.join("link"), root.join("other"));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let store = Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            Runtime::registry(),
        ))
        .unwrap();
        let runs = [
            ("run_link", &link),
            ("run_target", &target),
            ("run_other", &other),
        ];
        let events: Vec<StoredEvent> = runs
            .iter()
            .map(|(id, directory)| StoredEvent {
                id: format!("evt_{id}"),
                aggregate_id: (*id).into(),
                seq: 0,
                kind: super::super::workflows::RUN_STARTED.into(),
                data: json!({"session_id": "ses_owner", "name": "n", "status": "running",
                    "directory": directory.display().to_string(), "started_ms": 0,
                    "revision": 1, "digest": "sha256:0"}),
                time_ms: 0,
                causation_id: None,
            })
            .collect();
        let key = location_key(&link.display().to_string());
        let spellings = store
            .transaction(move |tx| {
                for event in &events {
                    super::super::workflows::project(tx, event).unwrap();
                }
                Ok(location_spellings(tx, &key)?)
            })
            .unwrap();
        let mut spellings: Vec<String> = serde_json::from_str(&spellings).unwrap();
        spellings.sort();
        let mut expected = vec![link.display().to_string(), target.display().to_string()];
        expected.sort();
        assert_eq!(spellings, expected);
    }

    #[test]
    fn counts_add_every_status_to_the_total() {
        let mut counts = AgentCounts::default();
        for (status, n) in [
            ("completed", 3),
            ("running", 2),
            ("failed", 1),
            ("other", 4),
        ] {
            counts.add(status, n);
        }
        assert_eq!(
            counts,
            AgentCounts {
                total: 10,
                completed: 3,
                running: 2,
                failed: 1,
                ..AgentCounts::default()
            }
        );
    }

    #[test]
    fn cursors_round_trip_and_malformed_ones_are_invalid() {
        let cursor = Cursor::parse("1:1700:run_a:b").unwrap();
        assert_eq!(
            (cursor.rank, cursor.started_ms, cursor.id.as_str()),
            (1, 1700, "run_a:b")
        );
        for bad in ["", "1", "1:2", "1:2:", "x:2:run", "1:y:run"] {
            assert!(
                matches!(Cursor::parse(bad), Err(RuntimeError::Invalid(_))),
                "{bad}"
            );
        }
    }
}
