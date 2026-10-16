//! `GET /usage?scope=session` breakdown (`observability-costs` → Session usage breakdown):
//! per-model usage and API time from the usage step ledger, wall time, code changes and
//! the Budgets that apply to the Session.

use cyber_core::budget::Budget;
use cyber_llm::Usage;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::{Runtime, RuntimeError, UsageReport};

/// One provider and model of the Session tree.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ModelUsage {
    pub provider: String,
    pub model: String,
    pub tokens: Usage,
    pub total_tokens: u64,
    /// Known priced cost; null when every call of this model was unpriced.
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    /// Finished calls (provider steps and auxiliary calls).
    pub steps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    Session,
    Daily,
}

/// One configured limit of a Budget and how much of it is used.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct BudgetLimit {
    /// `max_cost_usd`, `max_tokens` or `max_turns`.
    pub name: String,
    pub limit: f64,
    pub used: f64,
}

/// A Budget that applies to the Session.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct BudgetUsage {
    pub scope: BudgetScope,
    pub limits: Vec<BudgetLimit>,
    /// The next local midnight, for the daily budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_ms: Option<i64>,
    /// The IANA zone of the server's local day, for the daily budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

/// The Session usage report with its breakdown over the Session and its descendants.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SessionUsage {
    #[serde(flatten)]
    pub report: UsageReport,
    /// Sorted by cost, then by tokens, highest first.
    pub by_model: Vec<ModelUsage>,
    /// The sum of model call durations across the tree.
    pub api_duration_ms: u64,
    /// From the first admitted prompt to the last recorded activity (or now while the
    /// Session runs); null before the first prompt.
    pub wall_duration_ms: Option<u64>,
    /// Summed over the Session's per-message diffs; null without snapshot history.
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub budgets: Vec<BudgetUsage>,
    /// Why the daily budget could not be read from the Session directory's
    /// configuration; the rest of the report is still served, without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets_error: Option<String>,
    /// True while stored history is still being indexed: `by_model` may be partial.
    pub backfilling: bool,
}

/// Ledger rows of the Session and every descendant, including deleted descendants.
const TREE: &str = "WITH RECURSIVE tree(id) AS (
    SELECT ?1
    UNION SELECT s.id FROM session s JOIN tree t ON s.parent_id = t.id
    UNION SELECT u.session_id FROM usage_step u JOIN tree t ON u.parent_id = t.id
)";

fn by_model(db: &Connection, id: &str) -> rusqlite::Result<Vec<ModelUsage>> {
    let sql = format!(
        "{TREE}, rows AS (
            SELECT provider, model, input_tokens AS i, output_tokens AS o, reasoning_tokens AS r,
                cache_read_tokens AS cr, cache_write_tokens AS cw, cost AS c,
                CASE WHEN cost IS NULL AND failed = 0 THEN 1 ELSE 0 END AS u,
                CASE WHEN failed = 0 THEN 1 ELSE 0 END AS n
            FROM usage_step WHERE session_id IN (SELECT id FROM tree)
            UNION ALL
            SELECT provider, model, input_tokens, output_tokens, reasoning_tokens,
                cache_read_tokens, cache_write_tokens, cost, unpriced_steps, steps
            FROM usage_daily WHERE root_id = ?1
        ) SELECT provider, model, SUM(i), SUM(o), SUM(r), SUM(cr), SUM(cw), TOTAL(c), SUM(u), SUM(n)
          FROM rows GROUP BY provider, model HAVING SUM(n) > 0"
    );
    let mut statement = db.prepare(&sql)?;
    let mut models = statement
        .query_map([id], |r| {
            let n = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
            let tokens = Usage {
                input: n(2)?,
                output: n(3)?,
                reasoning: n(4)?,
                cache_read: n(5)?,
                cache_write: n(6)?,
            };
            let steps = n(9)?;
            let unpriced = n(8)?;
            Ok(ModelUsage {
                provider: r.get(0)?,
                model: r.get(1)?,
                total_tokens: tokens.input
                    + tokens.output
                    + tokens.reasoning
                    + tokens.cache_read
                    + tokens.cache_write,
                tokens,
                cost: (unpriced < steps).then_some(r.get::<_, f64>(7)?),
                unpriced_steps: unpriced,
                steps,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    models.sort_by(|a, b| {
        b.cost
            .unwrap_or(0.0)
            .total_cmp(&a.cost.unwrap_or(0.0))
            .then(b.total_tokens.cmp(&a.total_tokens))
            .then(a.model.cmp(&b.model))
    });
    Ok(models)
}

fn api_duration(db: &Connection, id: &str) -> rusqlite::Result<u64> {
    let sql = format!(
        "{TREE} SELECT
            COALESCE((SELECT SUM(MAX(ended_ms - started_ms, 0)) FROM usage_step
                WHERE session_id IN (SELECT id FROM tree)), 0)
            + COALESCE((SELECT SUM(api_duration_ms) FROM usage_daily WHERE root_id = ?1), 0)"
    );
    db.query_row(&sql, [id], |r| r.get::<_, i64>(0))
        .map(|v| v.max(0) as u64)
}

/// The first admitted prompt and the last recorded activity of the tree.
fn activity(db: &Connection, id: &str) -> rusqlite::Result<(Option<i64>, Option<i64>)> {
    let sql = format!(
        "{TREE} SELECT
            (SELECT MIN(time) FROM event WHERE aggregate_id = ?1 AND type = 'session.prompt.admitted.1'),
            MAX(COALESCE((SELECT MAX(updated_at) FROM session WHERE id IN (SELECT id FROM tree)), 0),
                COALESCE((SELECT MAX(ended_ms) FROM usage_step WHERE session_id IN (SELECT id FROM tree)), 0))"
    );
    db.query_row(&sql, [id], |r| Ok((r.get(0)?, r.get(1)?)))
}

/// Cost, tokens and Turns of the Session tree, as the budget check counts them.
fn session_spend(db: &Connection, id: &str) -> rusqlite::Result<Option<(String, [f64; 3])>> {
    db.query_row(
        "SELECT b.spec, s.cost + s.children_cost,
            s.input_tokens + s.output_tokens + s.reasoning_tokens + s.cache_read_tokens
                + s.cache_write_tokens + s.children_tokens,
            (SELECT COUNT(*) FROM event WHERE aggregate_id = s.id AND type = 'session.step.ended.1')
                + COALESCE((SELECT SUM(turns) FROM session_children_charge WHERE parent_id = s.id), 0)
         FROM session s JOIN session_budget b ON b.session_id = s.id WHERE s.id = ?1",
        [id],
        |r| {
            Ok((
                r.get(0)?,
                [r.get(1)?, r.get::<_, i64>(2)? as f64, r.get::<_, i64>(3)? as f64],
            ))
        },
    )
    .optional()
}

/// Cost, tokens and Turns of every Session on this machine since `since_ms`.
fn daily_spend(db: &Connection, since_ms: i64) -> rusqlite::Result<[f64; 3]> {
    db.query_row(
        "SELECT TOTAL(cost), COALESCE(SUM(input_tokens + output_tokens + reasoning_tokens
                + cache_read_tokens + cache_write_tokens), 0),
            COALESCE(SUM(purpose = 'turn' AND failed = 0), 0)
         FROM usage_step WHERE ended_ms >= ?1",
        [since_ms],
        |r| {
            Ok([
                r.get(0)?,
                r.get::<_, i64>(1)? as f64,
                r.get::<_, i64>(2)? as f64,
            ])
        },
    )
}

/// The cost, token and Turn limits of `budget`, with the matching `[cost, tokens, turns]`.
fn limits(budget: &Budget, used: [f64; 3]) -> Vec<BudgetLimit> {
    [
        ("max_cost_usd", budget.max_cost_usd, used[0]),
        ("max_tokens", budget.max_tokens.map(|v| v as f64), used[1]),
        ("max_turns", budget.max_turns.map(|v| v as f64), used[2]),
    ]
    .into_iter()
    .filter_map(|(name, limit, used)| {
        limit.map(|limit| BudgetLimit {
            name: name.into(),
            limit,
            used,
        })
    })
    .collect()
}

/// The server's local day: its start, the next midnight and the zone name.
fn local_day(now_ms: i64) -> Option<(i64, i64, String)> {
    let zone = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::from_millisecond(now_ms)
        .ok()?
        .to_zoned(zone.clone());
    let start = now.start_of_day().ok()?;
    let next = now.tomorrow().ok()?.start_of_day().ok()?;
    let name = zone.iana_name().unwrap_or("UTC").to_owned();
    Some((
        start.timestamp().as_millisecond(),
        next.timestamp().as_millisecond(),
        name,
    ))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

struct Ledger {
    by_model: Vec<ModelUsage>,
    api_duration_ms: u64,
    first: Option<i64>,
    last: Option<i64>,
    session: Option<(String, [f64; 3])>,
    daily: Option<[f64; 3]>,
    backfilling: bool,
}

impl Runtime {
    /// The Session usage report plus its breakdown.
    pub async fn session_usage_breakdown(&self, id: &str) -> Result<SessionUsage, RuntimeError> {
        let report = self.session_usage(id)?;
        let handle = self.inner.handle(id).await?;
        let (directory, lines) = {
            let state = handle.state.lock().await;
            (state.info.directory.clone(), changed_lines(&state))
        };
        let now = now_ms();
        let day = local_day(now);
        let (daily, budgets_error) = match self.inner.tools.daily_budget(&directory) {
            Ok(daily) => (daily, None),
            Err(error) => (None, Some(error)),
        };
        let source = id.to_owned();
        let since = day.as_ref().map(|d| d.0).filter(|_| daily.is_some());
        let ledger = self.inner.store.read(move |db| {
            Ok(Ledger {
                by_model: by_model(db, &source)?,
                api_duration_ms: api_duration(db, &source)?,
                session: session_spend(db, &source)?,
                daily: since.map(|since| daily_spend(db, since)).transpose()?,
                backfilling: db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM usage_backfill)",
                    [],
                    |r| r.get(0),
                )?,
                first: None,
                last: None,
            }
            .with_activity(activity(db, &source)?))
        })?;
        let running = self.is_running(id);
        let wall = ledger.first.map(|first| {
            let last = if running {
                now
            } else {
                ledger.last.unwrap_or(first)
            };
            last.saturating_sub(first).max(0) as u64
        });
        let mut budgets = Vec::new();
        if let Some((spec, used)) = &ledger.session
            && let Ok(budget) = serde_json::from_str::<Budget>(spec)
        {
            budgets.push(BudgetUsage {
                scope: BudgetScope::Session,
                limits: limits(&budget, *used),
                reset_ms: None,
                zone: None,
            });
        }
        if let (Some(budget), Some(used), Some((_, reset, zone))) = (&daily, ledger.daily, day) {
            budgets.push(BudgetUsage {
                scope: BudgetScope::Daily,
                limits: limits(budget, used),
                reset_ms: Some(reset),
                zone: Some(zone),
            });
        }
        budgets.retain(|b| !b.limits.is_empty());
        Ok(SessionUsage {
            report,
            by_model: ledger.by_model,
            api_duration_ms: ledger.api_duration_ms,
            wall_duration_ms: wall,
            lines_added: lines.map(|l| l.0),
            lines_removed: lines.map(|l| l.1),
            budgets,
            budgets_error,
            backfilling: ledger.backfilling,
        })
    }
}

impl Ledger {
    fn with_activity(mut self, (first, last): (Option<i64>, Option<i64>)) -> Self {
        self.first = first;
        self.last = last;
        self
    }
}

/// Lines added and removed over the per-message diffs; `None` when the Session ran steps
/// but no snapshot was ever recorded.
fn changed_lines(state: &super::SessionState) -> Option<(u64, u64)> {
    if !state.steps.is_empty() && state.steps.iter().all(|s| s.pre.is_none()) {
        return None;
    }
    let files = state.diffs.values().flatten();
    Some(files.fold((0, 0), |(added, removed), d| {
        (
            added + u64::from(d.additions),
            removed + u64::from(d.deletions),
        )
    }))
}
