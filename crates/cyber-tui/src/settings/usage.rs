//! The Settings Usage tab (`tui` → Settings Usage tab, Usage limit meters): the Session
//! tree's totals, durations, code changes and per-model usage, the validated token table
//! and cache hit rate, then one meter per configured Budget limit and the contributing
//! factors.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;
use serde_json::Value;

use crate::app::App;

/// The width of a Budget meter.
const METER: usize = 40;
/// Summary labels are padded to this width.
const LABEL: usize = 23;

/// One provider and model of the Session tree.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelUsage {
    pub provider: String,
    pub model: String,
    pub tokens: Tokens,
    pub total_tokens: u64,
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Limit {
    pub name: String,
    pub limit: f64,
    pub used: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Budget {
    pub scope: String,
    pub limits: Vec<Limit>,
    #[serde(default)]
    pub reset_ms: Option<i64>,
    #[serde(default)]
    pub zone: Option<String>,
}

/// The breakdown fields of `GET /usage?scope=session`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Breakdown {
    #[serde(rename = "by_model")]
    pub models: Vec<ModelUsage>,
    pub api_duration_ms: u64,
    pub wall_duration_ms: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub budgets: Vec<Budget>,
    /// Why the daily budget could not be read; the rest of the report stands.
    #[serde(default)]
    pub budgets_error: Option<String>,
    #[serde(default)]
    pub backfilling: bool,
}

impl Breakdown {
    /// Parse and check the breakdown; anything malformed refuses the whole report.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut breakdown: Self = serde_json::from_value(value.clone())
            .map_err(|_| "Malformed usage breakdown".to_string())?;
        breakdown.validate()?;
        breakdown.models.sort_by(|a, b| {
            b.cost
                .unwrap_or(0.0)
                .total_cmp(&a.cost.unwrap_or(0.0))
                .then(b.total_tokens.cmp(&a.total_tokens))
        });
        Ok(breakdown)
    }

    fn validate(&self) -> Result<(), String> {
        let money = |v: f64| v.is_finite() && v >= 0.0;
        let models_ok = self.models.iter().all(|m| {
            let t = &m.tokens;
            let sum = [t.input, t.output, t.reasoning, t.cache_read, t.cache_write]
                .into_iter()
                .map(u128::from)
                .sum::<u128>();
            sum == u128::from(m.total_tokens) && m.cost.is_none_or(money)
        });
        let budgets_ok = self.budgets.iter().all(|b| {
            matches!(b.scope.as_str(), "session" | "daily")
                && b.limits.iter().all(|l| {
                    matches!(l.name.as_str(), "max_cost_usd" | "max_tokens" | "max_turns")
                        && money(l.limit)
                        && money(l.used)
                })
        });
        if models_ok && budgets_ok {
            Ok(())
        } else {
            Err("Invalid usage breakdown".into())
        }
    }
}

/// `$125.43`, or four decimals below a dollar (`$0.0007`).
pub fn money(cost: f64) -> String {
    if cost >= 1.0 || cost == 0.0 {
        format!("${cost:.2}")
    } else {
        format!("${cost:.4}")
    }
}

/// `375`, `5.3k`, `352.7k`, `1.2m`, `317.4m`, `4.6b`.
pub fn count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    for (scale, unit) in [(1e9, "b"), (1e6, "m"), (1e3, "k")] {
        let value = n as f64 / scale;
        // 999,950 is `1.0m`, not `1000.0k`.
        if (value * 10.0).round() >= 10.0 {
            return format!("{value:.1}{unit}");
        }
    }
    n.to_string()
}

/// `3h 21m 0s`, `5d 23h 31m`, `4m 2s`, `12s`, `340ms`.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    let (d, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    match s {
        0 => format!("{ms}ms"),
        1..60 => format!("{sec}s"),
        60..3600 => format!("{m}m {sec}s"),
        3600..86_400 => format!("{h}h {m}m {sec}s"),
        _ => format!("{d}d {h}h {m}m"),
    }
}

/// One line of untrusted server text with control characters removed.
fn clean(text: &str) -> String {
    crate::sidebar::clean(text)
}

fn muted(app: &App, text: String) -> Span<'static> {
    Span::styled(text, Style::default().fg(app.theme.text_muted))
}

fn row(app: &App, label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<LABEL$}", format!("{label}:")),
            Style::default().fg(app.theme.text_muted),
        ),
        Span::styled(value, Style::default().fg(app.theme.text)),
    ])
}

/// The tab's rows: the Session section with its Budget meters, the provider-reported
/// windows, then the contributing factors.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out = session(app);
    crate::settings::limits::meters(app, &mut out);
    out.extend(crate::settings::insights::lines(app, width));
    out
}

fn session(app: &App) -> Vec<Line<'static>> {
    let mut out = vec![
        crate::settings::status::heading(app, "Session"),
        Line::default(),
    ];
    let Some(cost) = app.cost.snapshot() else {
        match app.cost.error() {
            Some(error) => out.push(crate::settings::status::failed(app, "usage", error)),
            None => out.push(Line::from(muted(app, "Loading…".into()))),
        }
        return out;
    };
    out.push(row(app, "Total cost", cost.total_cost()));
    if let Some(breakdown) = &cost.breakdown {
        summary(app, breakdown, &mut out);
    }
    out.push(Line::default());
    out.extend(
        cost.body()
            .into_iter()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(app.theme.text)))),
    );
    if let Some(error) = app.cost.error() {
        out.push(crate::settings::status::failed(app, "usage", error));
    }
    if let Some(breakdown) = &cost.breakdown {
        meters(app, breakdown, &mut out);
    }
    out
}

fn summary(app: &App, b: &Breakdown, out: &mut Vec<Line<'static>>) {
    out.push(row(
        app,
        "Total duration (API)",
        duration(b.api_duration_ms),
    ));
    if let Some(wall) = b.wall_duration_ms {
        out.push(row(app, "Total duration (wall)", duration(wall)));
    }
    if let (Some(added), Some(removed)) = (b.lines_added, b.lines_removed) {
        out.push(row(
            app,
            "Total code changes",
            format!("{added} lines added, {removed} lines removed"),
        ));
    }
    if b.models.is_empty() {
        return;
    }
    out.push(Line::from(muted(app, "Usage by model:".into())));
    let names: Vec<String> = b.models.iter().map(|m| model_name(b, m)).collect();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0) + 1;
    for (model, name) in b.models.iter().zip(names) {
        out.push(Line::from(vec![
            muted(app, format!("    {:>width$}  ", format!("{name}:"))),
            Span::styled(model_usage(model), Style::default().fg(app.theme.text)),
        ]));
    }
    if b.backfilling {
        out.push(Line::from(muted(
            app,
            "    Indexing history… per-model usage is partial".into(),
        )));
    }
}

/// The model name, with its provider when another provider has a model of that name.
fn model_name(b: &Breakdown, m: &ModelUsage) -> String {
    let shared = b
        .models
        .iter()
        .filter(|other| other.model == m.model)
        .count()
        > 1;
    if shared && !m.provider.is_empty() {
        clean(&format!("{}/{}", m.provider, m.model))
    } else {
        clean(&m.model)
    }
}

/// `5.3k input, 375 output, 0 cache read, 0 cache write ($0.0007)`.
fn model_usage(m: &ModelUsage) -> String {
    let t = &m.tokens;
    let mut parts = vec![
        format!("{} input", count(t.input)),
        format!("{} output", count(t.output)),
    ];
    if t.reasoning > 0 {
        parts.push(format!("{} reasoning", count(t.reasoning)));
    }
    parts.push(format!("{} cache read", count(t.cache_read)));
    parts.push(format!("{} cache write", count(t.cache_write)));
    let cost = match m.cost {
        None => "—".to_string(),
        Some(cost) if m.unpriced_steps > 0 => format!("≥ {}", money(cost)),
        Some(cost) => money(cost),
    };
    format!("{} ({cost})", parts.join(", "))
}

/// One meter per Budget limit; nothing when no Budget applies.
fn meters(app: &App, b: &Breakdown, out: &mut Vec<Line<'static>>) {
    if let Some(error) = &b.budgets_error {
        out.push(Line::default());
        out.push(Line::from(Span::styled(
            format!("Daily budget unavailable: {}", crate::sidebar::clean(error)),
            Style::default().fg(app.theme.warning),
        )));
    }
    for budget in &b.budgets {
        for limit in &budget.limits {
            out.push(Line::default());
            meter(app, budget, limit, out);
        }
    }
}

/// `Daily budget ($20.00)`, then the 40-column bar with `85% used`, then the reset time.
fn meter(app: &App, budget: &Budget, limit: &Limit, out: &mut Vec<Line<'static>>) {
    let scope = if budget.scope == "daily" {
        "Daily budget"
    } else {
        "Session budget"
    };
    let cap = match limit.name.as_str() {
        "max_cost_usd" => format!("${:.2}", limit.limit),
        "max_tokens" => format!("{} tokens", count(limit.limit as u64)),
        _ => format!("{} Turns", limit.limit as u64),
    };
    let percent = if limit.limit > 0.0 {
        100.0 * limit.used / limit.limit
    } else {
        100.0
    };
    meter_rows(app, format!("{scope} ({cap})"), percent, out);
    if let Some(reset) = budget.reset_ms {
        out.push(Line::from(muted(
            app,
            format!("Resets {}", reset_time(reset, budget.zone.as_deref())),
        )));
    }
}

/// The bold `label`, then the 40-column bar with `N% used` in `meter_fill`, `warning`
/// from 80% and `error` from 100%; one sentence for a screen reader.
pub(super) fn meter_rows(app: &App, label: String, percent: f64, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let color = match percent {
        p if p >= 100.0 => t.error,
        p if p >= 80.0 => t.warning,
        _ => t.meter_fill,
    };
    let used = format!("{:.0}% used", percent.floor());
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    if app.screen_reader {
        out.push(Line::from(vec![
            Span::styled(format!("{label}: "), bold),
            Span::styled(used, Style::default().fg(color)),
        ]));
        return;
    }
    out.push(Line::from(Span::styled(label, bold)));
    let filled = ((percent / 100.0) * METER as f64)
        .round()
        .clamp(0.0, METER as f64) as usize;
    out.push(Line::from(vec![
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled(
            "░".repeat(METER - filled),
            Style::default().fg(t.meter_empty),
        ),
        Span::styled(format!(" {used}"), Style::default().fg(color)),
    ]));
}

/// A plausible IANA zone name; anything else is never looked up.
fn zone_name(zone: &str) -> bool {
    !zone.is_empty()
        && zone.len() <= 64
        && !zone.contains("..")
        && zone
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

/// The named zone, or the local zone when the name is unsafe or unknown here.
fn zone(zone: Option<&str>) -> (jiff::tz::TimeZone, String) {
    let named = zone
        .filter(|z| zone_name(z))
        .and_then(|z| jiff::tz::TimeZone::get(z).ok().map(|tz| (tz, z.to_owned())));
    named.unwrap_or_else(|| {
        let tz = jiff::tz::TimeZone::system();
        let name = tz.iana_name().unwrap_or("local").to_owned();
        (tz, name)
    })
}

/// `12:00am`.
fn clock(at: &jiff::Zoned) -> String {
    let hour = at.hour();
    let (twelve, half) = match hour {
        0 => (12, "am"),
        1..12 => (hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    };
    format!("{twelve}:{:02}{half}", at.minute())
}

/// `12:00am (America/Barbados)` in the server's zone, or the local zone when the server's
/// name is unknown here.
pub fn reset_time(ms: i64, zone_name: Option<&str>) -> String {
    let (tz, name) = zone(zone_name);
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return "unknown".into();
    };
    format!("{} ({name})", clock(&at.to_zoned(tz)))
}

/// [`reset_time`], with the date (`Oct 23, 4:00am (UTC)`) when the reset is a day or more
/// after `now_ms`.
pub fn reset_when(ms: i64, now_ms: i64, zone_name: Option<&str>) -> String {
    if ms.saturating_sub(now_ms) < 86_400_000 {
        return reset_time(ms, zone_name);
    }
    let (tz, name) = zone(zone_name);
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return "unknown".into();
    };
    let at = at.to_zoned(tz);
    format!("{}, {} ({name})", at.strftime("%b %-d"), clock(&at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_use_compact_lowercase_units() {
        let shown: Vec<String> = [
            375,
            5_300,
            352_700,
            999_950,
            1_200_000,
            317_400_000,
            4_600_000_000,
        ]
        .into_iter()
        .map(count)
        .collect();
        assert_eq!(
            shown,
            ["375", "5.3k", "352.7k", "1.0m", "1.2m", "317.4m", "4.6b"]
        );
    }

    #[test]
    fn durations_use_days_hours_minutes_and_seconds() {
        assert_eq!(duration(12_060_000), "3h 21m 0s");
        assert_eq!(duration(518_000_000 + 60_000 * 20), "6d 0h 13m");
        assert_eq!(
            duration((5 * 86_400 + 23 * 3600 + 31 * 60) * 1000),
            "5d 23h 31m"
        );
        assert_eq!(duration(242_000), "4m 2s");
        assert_eq!(duration(11_300), "11s");
        assert_eq!(duration(340), "340ms");
    }

    #[test]
    fn money_keeps_cents_and_small_amounts() {
        assert_eq!(money(125.43), "$125.43");
        assert_eq!(money(0.0007), "$0.0007");
        assert_eq!(money(0.0), "$0.00");
    }

    #[test]
    fn reset_times_show_the_zone_and_refuse_unsafe_names() {
        // 2026-10-16T04:00:00Z is midnight in America/Barbados (UTC-4).
        let ms = 1_792_123_200_000;
        assert_eq!(reset_time(ms, Some("UTC")), "4:00am (UTC)");
        assert_eq!(
            reset_time(ms, Some("America/Barbados")),
            "12:00am (America/Barbados)"
        );
        assert!(!zone_name("../../etc/passwd"));
        assert!(!zone_name("UTC\u{1b}]52;"));
        assert!(zone_name("Etc/GMT+4"));
        assert_eq!(reset_when(ms, ms - 3_600_000, Some("UTC")), "4:00am (UTC)");
        assert_eq!(
            reset_when(ms + 7 * 86_400_000, ms, Some("UTC")),
            "Oct 23, 4:00am (UTC)"
        );
    }
}
