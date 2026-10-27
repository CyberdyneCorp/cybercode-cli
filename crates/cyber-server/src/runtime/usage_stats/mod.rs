//! `GET /usage/stats` (`observability-costs` → Usage statistics API): activity per local
//! day, streaks, the favorite model and per-model totals over a period, from the usage
//! step ledger and its folded daily rows. `cyber stats` and the TUI Stats tab both render
//! this response, so they cannot disagree.

mod aggregate;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use cyber_llm::Usage;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::{Runtime, RuntimeError};
use aggregate::{Row, Span, When, Window};

/// The longest `Nd` period, and the most days `all` returns.
pub const MAX_PERIOD_DAYS: u16 = 371;
/// Ledger rows are grouped by 15-minute bucket: every zone offset in use is a multiple
/// of 15 minutes, so a bucket never straddles a local midnight.
const BUCKET_MS: i64 = 15 * 60 * 1000;

/// `all`, or the last N local days including today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    All,
    Days(u16),
}

impl Period {
    /// `all` or `1d` to `371d`; anything else is refused.
    pub fn parse(value: &str) -> Option<Self> {
        if value == "all" {
            return Some(Self::All);
        }
        let digits = value.strip_suffix('d')?;
        if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let days: u16 = digits.parse().ok()?;
        (1..=MAX_PERIOD_DAYS)
            .contains(&days)
            .then_some(Self::Days(days))
    }

    pub fn label(self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Days(n) => format!("{n}d"),
        }
    }
}

/// What `GET /usage/stats` asks for.
#[derive(Debug, Clone)]
pub struct StatsRequest {
    pub period: Period,
    /// Only Sessions whose directory is inside this project root.
    pub project: Option<PathBuf>,
    /// The IANA zone of the local days; the server's zone when absent or unknown.
    pub zone: Option<String>,
}

/// One local day of the period.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct StatsDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub tokens: Usage,
    pub total_tokens: u64,
    /// Known priced cost.
    pub cost: f64,
    pub turns: u64,
    /// Root Sessions with usage on this day.
    pub sessions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct StatsTotals {
    pub tokens: Usage,
    pub total_tokens: u64,
    /// Known priced cost; null when every call was unpriced (or nothing ran).
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    /// Finished calls (provider steps and auxiliary calls).
    pub steps: u64,
    pub turns: u64,
    /// Root Sessions with usage in the period.
    pub sessions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct StatsModel {
    pub provider: String,
    pub model: String,
    pub tokens: Usage,
    pub total_tokens: u64,
    /// Known priced cost; null when every call of this model was unpriced.
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    pub steps: u64,
    pub turns: u64,
    pub sessions: u64,
    /// This model's share (0–1) of the period's tokens.
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct FavoriteModel {
    pub provider: String,
    pub model: String,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct LongestSession {
    pub id: String,
    /// Null once the Session was deleted.
    pub title: Option<String>,
    /// From the tree's first call to its last one in the period.
    pub duration_ms: u64,
}

/// Usage of one origin (`cross-session-messaging` → Usage attribution): `user`,
/// `session`, `client`, `job`, `mcp`, or the admission source (`workflow`, `hook`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct StatsOrigin {
    pub origin: String,
    pub tokens: Usage,
    pub total_tokens: u64,
    /// Known priced cost; null when every call of this origin was unpriced.
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    pub steps: u64,
    pub turns: u64,
    pub sessions: u64,
}

/// The `GET /usage/stats` response.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageStats {
    /// `all` or `Nd`.
    pub period: String,
    /// The IANA zone of the local days.
    pub zone: String,
    /// The start of the period's first local day; null for `all`.
    pub since_ms: Option<i64>,
    /// One entry per local day, oldest first; at most the last 371 for `all`.
    pub days: Vec<StatsDay>,
    pub totals: StatsTotals,
    /// The model with the most tokens.
    pub favorite_model: Option<FavoriteModel>,
    pub longest_session: Option<LongestSession>,
    /// Days with at least one Turn.
    pub active_days: u32,
    /// Local days in the period (for `all`, since the first day with usage).
    pub period_days: u32,
    pub longest_streak: u32,
    /// Consecutive active days up to today; today counts once it has a Turn, and a
    /// streak that reached yesterday is still current.
    pub current_streak: u32,
    /// The active day with the most tokens, `YYYY-MM-DD`.
    pub most_active_day: Option<String>,
    /// Sorted by tokens, highest first.
    pub models: Vec<StatsModel>,
    /// One entry per usage origin, sorted by tokens, highest first.
    pub origins: Vec<StatsOrigin>,
    /// True when part of the period was served from daily aggregates (UTC days).
    pub approximate_days: bool,
    /// True while stored history is still being indexed.
    pub backfilling: bool,
}

/// An IANA name the zone database may look up: no path syntax and no control bytes.
fn plausible_zone(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'))
}

/// The requested zone, or the server's own when the name is absent or unknown.
pub(crate) fn resolve_zone(name: Option<&str>) -> (TimeZone, String) {
    if let Some(name) = name.filter(|n| plausible_zone(n))
        && let Ok(zone) = TimeZone::get(name)
    {
        return (zone, name.to_owned());
    }
    let zone = TimeZone::system();
    let name = zone.iana_name().unwrap_or("UTC").to_owned();
    (zone, name)
}

fn usage(r: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Usage> {
    let n = |i: usize| r.get::<_, i64>(first + i).map(|v| v.max(0) as u64);
    Ok(Usage {
        input: n(0)?,
        output: n(1)?,
        reasoning: n(2)?,
        cache_read: n(3)?,
        cache_write: n(4)?,
    })
}

fn row(r: &rusqlite::Row<'_>, when: When) -> rusqlite::Result<Row> {
    let n = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
    Ok(Row {
        when,
        root: r.get(1)?,
        provider: r.get(2)?,
        model: r.get(3)?,
        directory: r.get(4)?,
        steps: n(5)?,
        turns: n(6)?,
        unpriced: n(7)?,
        tokens: usage(r, 8)?,
        cost: r.get(13)?,
        origin: r.get(14)?,
    })
}

/// Ledger rows since `since_ms`, grouped by bucket, root, model and directory.
fn buckets(db: &Connection, since_ms: i64) -> rusqlite::Result<Vec<Row>> {
    let finished = super::usage_ledger::FINISHED_CALL;
    let unpriced = super::usage_ledger::unpriced_call();
    let turns = super::usage_ledger::RECORDED_TURN;
    let mut statement = db.prepare(&format!(
        "SELECT (started_ms / ?2) * ?2, root_id, provider, model, directory,
            SUM({finished}), SUM({turns}), SUM({unpriced}),
            SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens), SUM(cache_read_tokens),
            SUM(cache_write_tokens), TOTAL(cost), origin
         FROM usage_step u WHERE started_ms >= ?1 GROUP BY 1, 2, 3, 4, 5, 15"
    ))?;
    statement
        .query_map([since_ms, BUCKET_MS], |r| row(r, When::Bucket(r.get(0)?)))?
        .collect()
}

/// Folded daily rows from the UTC day `since` on.
fn folded(db: &Connection, since: &str) -> rusqlite::Result<Vec<Row>> {
    let mut statement = db.prepare(
        "SELECT day, root_id, provider, model, directory, steps, turns, unpriced_steps,
            input_tokens, output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens, cost,
            origin
         FROM usage_daily WHERE day >= ?1",
    )?;
    let rows = statement
        .query_map([since], |r| {
            Ok((r.get::<_, String>(0)?, row(r, When::Bucket(0))?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(day, mut row)| {
            row.when = When::Day(day.parse().ok()?);
            Some(row)
        })
        .collect())
}

/// The first and last call of each root tree and directory since `since_ms`.
fn spans(db: &Connection, since_ms: i64) -> rusqlite::Result<Vec<Span>> {
    let mut statement = db.prepare(
        "SELECT root_id, directory, MIN(started_ms), MAX(ended_ms) FROM usage_step
         WHERE started_ms >= ?1 AND failed = 0 GROUP BY root_id, directory",
    )?;
    statement
        .query_map([since_ms], |r| {
            Ok(Span {
                root: r.get(0)?,
                directory: r.get(1)?,
                first: r.get(2)?,
                last: r.get(3)?,
            })
        })?
        .collect()
}

fn title(db: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT title FROM session WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .optional()
}

/// Whether a Session directory is inside the project root (component-wise).
pub(super) fn in_project(directory: &str, project: Option<&Path>) -> bool {
    project.is_none_or(|root| Path::new(directory).starts_with(root))
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The ledger input of one request.
struct Ledger {
    rows: Vec<Row>,
    spans: Vec<Span>,
    backfilling: bool,
}

fn ledger(db: &Connection, since_ms: i64, since_day: &str) -> rusqlite::Result<Ledger> {
    let mut rows = buckets(db, since_ms)?;
    rows.extend(folded(db, since_day)?);
    Ok(Ledger {
        rows,
        spans: spans(db, since_ms)?,
        backfilling: db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backfill)", [], |r| {
            r.get(0)
        })?,
    })
}

impl Runtime {
    /// Usage statistics over the requested period, local days and project.
    pub fn usage_stats(&self, request: &StatsRequest) -> Result<UsageStats, RuntimeError> {
        self.usage_stats_at(request, now_ms())
    }

    /// [`Runtime::usage_stats`] as of `now_ms`.
    pub fn usage_stats_at(
        &self,
        request: &StatsRequest,
        now_ms: i64,
    ) -> Result<UsageStats, RuntimeError> {
        let (zone, zone_name) = resolve_zone(request.zone.as_deref());
        let window = Window::new(zone, request.period, now_ms)
            .ok_or_else(|| RuntimeError::Invalid("The server clock is out of range".into()))?;
        let since_ms = window.since_ms();
        let since_day = window
            .first_day()
            .map_or_else(String::new, |d| d.to_string());
        let Ledger {
            mut rows,
            mut spans,
            backfilling,
        } = self
            .inner
            .store
            .read(move |db| Ok(ledger(db, since_ms.unwrap_or(i64::MIN), &since_day)?))?;
        let project = request.project.as_deref();
        rows.retain(|r| in_project(&r.directory, project));
        spans.retain(|s| in_project(&s.directory, project));
        let mut stats = aggregate::aggregate(&rows, &spans, &window);
        stats.period = request.period.label();
        stats.zone = zone_name;
        stats.backfilling = backfilling;
        if let Some(longest) = stats.longest_session.as_mut() {
            let id = longest.id.clone();
            longest.title = self.inner.store.read(move |db| Ok(title(db, &id)?))?;
        }
        Ok(stats)
    }
}

/// The local date of `ms` in `zone`.
fn local_date(zone: &TimeZone, ms: i64) -> Option<Date> {
    Some(
        jiff::Timestamp::from_millisecond(ms)
            .ok()?
            .to_zoned(zone.clone())
            .date(),
    )
}
