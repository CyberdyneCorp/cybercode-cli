//! The pure part of the statistics: ledger groups in, the response out. Local days,
//! streaks, the favorite model and the most active day are decided here, once.

use std::collections::{BTreeMap, BTreeSet};

use cyber_llm::Usage;
use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use super::{
    FavoriteModel, LongestSession, MAX_PERIOD_DAYS, Period, StatsDay, StatsModel, StatsOrigin,
    StatsTotals, UsageStats, local_date,
};

/// When a ledger group happened: a bucket start (ms), or a folded UTC day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum When {
    Bucket(i64),
    Day(Date),
}

/// One ledger group.
#[derive(Debug, Clone)]
pub(super) struct Row {
    pub when: When,
    pub root: String,
    pub provider: String,
    pub model: String,
    pub directory: String,
    pub steps: u64,
    pub turns: u64,
    pub unpriced: u64,
    pub tokens: Usage,
    pub cost: f64,
    /// `user` for rows recorded before origins existed.
    pub origin: String,
}

/// The first and last call of a root tree in one directory.
#[derive(Debug, Clone)]
pub(super) struct Span {
    pub root: String,
    pub directory: String,
    pub first: i64,
    pub last: i64,
}

/// The local days of a request.
pub(super) struct Window {
    zone: TimeZone,
    today: Date,
    /// The first day of an `Nd` period; `None` for `all`.
    first: Option<Date>,
}

impl Window {
    pub fn new(zone: TimeZone, period: Period, now_ms: i64) -> Option<Self> {
        let today = local_date(&zone, now_ms)?;
        let first = match period {
            Period::All => None,
            Period::Days(n) => Some(today.checked_sub((i64::from(n) - 1).days()).ok()?),
        };
        Some(Self { zone, today, first })
    }

    pub fn first_day(&self) -> Option<Date> {
        self.first
    }

    /// The first local midnight of an `Nd` period.
    pub fn since_ms(&self) -> Option<i64> {
        let first = self.first?;
        Some(
            first
                .to_zoned(self.zone.clone())
                .ok()?
                .timestamp()
                .as_millisecond(),
        )
    }

    /// The local day of a group; a clock-skewed future call counts as today.
    fn date(&self, when: When) -> Option<Date> {
        let date = match when {
            When::Bucket(ms) => local_date(&self.zone, ms)?,
            When::Day(day) => day,
        };
        Some(date.min(self.today))
    }
}

fn total(u: &Usage) -> u64 {
    u.input + u.output + u.reasoning + u.cache_read + u.cache_write
}

/// Sums of one day, one model or the whole period.
#[derive(Debug, Default)]
struct Acc {
    tokens: Usage,
    cost: f64,
    steps: u64,
    unpriced: u64,
    turns: u64,
    roots: BTreeSet<String>,
}

impl Acc {
    fn add(&mut self, row: &Row) {
        self.tokens.add(&row.tokens);
        self.cost += row.cost;
        self.steps += row.steps;
        self.unpriced += row.unpriced;
        self.turns += row.turns;
        self.roots.insert(row.root.clone());
    }

    /// Known cost, or null when every call was unpriced.
    fn known_cost(&self) -> Option<f64> {
        (self.unpriced < self.steps).then_some(self.cost)
    }
}

struct Sums {
    days: BTreeMap<Date, Acc>,
    models: BTreeMap<(String, String), Acc>,
    origins: BTreeMap<String, Acc>,
    totals: Acc,
    approximate: bool,
}

fn sum(rows: &[Row], window: &Window) -> Sums {
    let mut sums = Sums {
        days: BTreeMap::new(),
        models: BTreeMap::new(),
        origins: BTreeMap::new(),
        totals: Acc::default(),
        approximate: false,
    };
    for row in rows {
        let Some(date) = window.date(row.when) else {
            continue;
        };
        sums.approximate |= matches!(row.when, When::Day(_));
        sums.days.entry(date).or_default().add(row);
        sums.models
            .entry((row.provider.clone(), row.model.clone()))
            .or_default()
            .add(row);
        sums.origins.entry(row.origin.clone()).or_default().add(row);
        sums.totals.add(row);
    }
    sums
}

/// Every local day of the period, oldest first. `all` starts at the first day with usage.
fn period_days(window: &Window, sums: &Sums) -> Vec<Date> {
    let first = window
        .first
        .or_else(|| sums.days.keys().next().copied())
        .unwrap_or(window.today)
        .min(window.today);
    first
        .series(1.day())
        .take_while(|d| *d <= window.today)
        .collect()
}

/// `(longest, current)`: the current streak may end yesterday while today has no Turn.
pub(super) fn streaks(active: &[bool]) -> (u32, u32) {
    let mut longest = 0;
    let mut run = 0;
    for &day in active {
        run = if day { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let skip = usize::from(active.last() == Some(&false));
    let current = active
        .iter()
        .rev()
        .skip(skip)
        .take_while(|day| **day)
        .count() as u32;
    (longest, current)
}

fn day_entry(date: Date, acc: Option<&Acc>) -> StatsDay {
    let empty = Acc::default();
    let acc = acc.unwrap_or(&empty);
    StatsDay {
        date: date.to_string(),
        tokens: acc.tokens,
        total_tokens: total(&acc.tokens),
        cost: acc.cost,
        turns: acc.turns,
        sessions: acc.roots.len() as u64,
    }
}

fn models(sums: &Sums) -> Vec<StatsModel> {
    let all = total(&sums.totals.tokens);
    let mut models: Vec<StatsModel> = sums
        .models
        .iter()
        .filter(|(_, acc)| acc.steps > 0)
        .map(|((provider, model), acc)| {
            let tokens = total(&acc.tokens);
            StatsModel {
                provider: provider.clone(),
                model: model.clone(),
                tokens: acc.tokens,
                total_tokens: tokens,
                cost: acc.known_cost(),
                unpriced_steps: acc.unpriced,
                steps: acc.steps,
                turns: acc.turns,
                sessions: acc.roots.len() as u64,
                share: if all == 0 {
                    0.0
                } else {
                    tokens as f64 / all as f64
                },
            }
        })
        .collect();
    models.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.model.cmp(&b.model))
    });
    models
}

fn origins(sums: &Sums) -> Vec<StatsOrigin> {
    let mut origins: Vec<StatsOrigin> = sums
        .origins
        .iter()
        .filter(|(_, acc)| acc.steps > 0)
        .map(|(origin, acc)| StatsOrigin {
            origin: origin.clone(),
            tokens: acc.tokens,
            total_tokens: total(&acc.tokens),
            cost: acc.known_cost(),
            unpriced_steps: acc.unpriced,
            steps: acc.steps,
            turns: acc.turns,
            sessions: acc.roots.len() as u64,
        })
        .collect();
    origins.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.origin.cmp(&b.origin))
    });
    origins
}

/// The root tree with the longest span from its first to its last call.
fn longest(spans: &[Span]) -> Option<LongestSession> {
    let mut trees: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    for span in spans {
        let entry = trees.entry(&span.root).or_insert((span.first, span.last));
        entry.0 = entry.0.min(span.first);
        entry.1 = entry.1.max(span.last);
    }
    let mut best: Option<(&str, u64)> = None;
    for (root, (first, last)) in trees {
        let duration = last.saturating_sub(first).max(0) as u64;
        if best.is_none_or(|(_, d)| duration > d) {
            best = Some((root, duration));
        }
    }
    best.map(|(id, duration_ms)| LongestSession {
        id: id.to_owned(),
        title: None,
        duration_ms,
    })
}

/// The active day with the most tokens; the earliest wins a tie.
fn most_active(days: &[StatsDay]) -> Option<String> {
    let mut best: Option<&StatsDay> = None;
    for day in days.iter().filter(|d| d.turns > 0) {
        if best.is_none_or(|b| day.total_tokens > b.total_tokens) {
            best = Some(day);
        }
    }
    best.map(|d| d.date.clone())
}

pub(super) fn aggregate(rows: &[Row], spans: &[Span], window: &Window) -> UsageStats {
    let sums = sum(rows, window);
    let days: Vec<StatsDay> = period_days(window, &sums)
        .into_iter()
        .map(|date| day_entry(date, sums.days.get(&date)))
        .collect();
    let active: Vec<bool> = days.iter().map(|d| d.turns > 0).collect();
    let (longest_streak, current_streak) = streaks(&active);
    let models = models(&sums);
    let favorite_model = models.first().map(|m| FavoriteModel {
        provider: m.provider.clone(),
        model: m.model.clone(),
        total_tokens: m.total_tokens,
    });
    let totals = &sums.totals;
    let keep = days.len().saturating_sub(usize::from(MAX_PERIOD_DAYS));
    UsageStats {
        period: String::new(),
        zone: String::new(),
        since_ms: window.since_ms(),
        totals: StatsTotals {
            tokens: totals.tokens,
            total_tokens: total(&totals.tokens),
            cost: totals.known_cost(),
            unpriced_steps: totals.unpriced,
            steps: totals.steps,
            turns: totals.turns,
            sessions: totals.roots.len() as u64,
        },
        favorite_model,
        longest_session: longest(spans),
        active_days: active.iter().filter(|a| **a).count() as u32,
        period_days: days.len() as u32,
        longest_streak,
        current_streak,
        most_active_day: most_active(&days),
        models,
        origins: origins(&sums),
        approximate_days: sums.approximate,
        backfilling: false,
        days: days.into_iter().skip(keep).collect(),
    }
}
