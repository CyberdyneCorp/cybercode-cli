//! The Settings Stats tab (`tui` → Settings Stats tab): `GET /usage/stats` as an
//! Overview (heatmap, metrics and the token line) and a Models list. `p` cycles the
//! period, `v` the subtab and `s` the scope; days are local days in the TUI's zone.

use crossterm::event::{KeyCode, KeyEvent};
use jiff::civil::Date;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;
use serde_json::Value;

use super::usage::{Tokens, count, duration, money};
use super::{Loadable, heatmap};
use crate::app::{Action, App};

/// The width of a Models share bar.
const BAR: usize = 20;
/// Metrics go in two columns from this tab width.
const TWO_COLUMNS: usize = 80;
/// The width of the left metric column.
const COLUMN: usize = 34;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Period {
    #[default]
    All,
    Week,
    Month,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::All, Period::Week, Period::Month];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All time",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
        }
    }

    pub fn query(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::All => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Overview,
    Models,
}

/// One local day; only what the tab draws.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Day {
    pub date: String,
    pub total_tokens: u64,
    pub turns: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Totals {
    pub tokens: Tokens,
    pub total_tokens: u64,
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    pub steps: u64,
    pub turns: u64,
    pub sessions: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Favorite {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Longest {
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Model {
    pub provider: String,
    pub model: String,
    pub total_tokens: u64,
    pub cost: Option<f64>,
    pub unpriced_steps: u64,
    pub turns: u64,
    pub sessions: u64,
    pub share: f64,
}

/// The `GET /usage/stats` fields the tab draws.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Stats {
    pub days: Vec<Day>,
    pub totals: Totals,
    pub favorite_model: Option<Favorite>,
    pub longest_session: Option<Longest>,
    pub active_days: u32,
    pub period_days: u32,
    pub longest_streak: u32,
    pub current_streak: u32,
    pub most_active_day: Option<String>,
    pub models: Vec<Model>,
    #[serde(default)]
    pub approximate_days: bool,
    #[serde(default)]
    pub backfilling: bool,
}

impl Stats {
    /// Parse and check the response; anything malformed refuses the whole report.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let stats: Self = serde_json::from_value(value.clone())
            .map_err(|_| "Malformed usage statistics".to_string())?;
        if stats.valid() {
            Ok(stats)
        } else {
            Err("Invalid usage statistics".into())
        }
    }

    fn valid(&self) -> bool {
        let money_ok = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && v >= 0.0);
        let t = &self.totals.tokens;
        let sum = [t.input, t.output, t.reasoning, t.cache_read, t.cache_write]
            .into_iter()
            .map(u128::from)
            .sum::<u128>();
        sum == u128::from(self.totals.total_tokens)
            && money_ok(self.totals.cost)
            && self.days.iter().all(|d| d.date.parse::<Date>().is_ok())
            && self
                .most_active_day
                .as_deref()
                .is_none_or(|d| d.parse::<Date>().is_ok())
            && self.active_days <= self.period_days
            && self
                .models
                .iter()
                .all(|m| money_ok(m.cost) && m.share.is_finite() && (0.0..=1.0).contains(&m.share))
    }

    fn dated(&self) -> Vec<(Date, &Day)> {
        self.days
            .iter()
            .filter_map(|d| Some((d.date.parse().ok()?, d)))
            .collect()
    }
}

/// The tab's state: what to ask for and the last reply.
#[derive(Debug)]
pub struct StatsState {
    pub period: Period,
    pub view: View,
    /// `This project` instead of `All projects`.
    pub project: bool,
    /// The IANA zone of the local days; the server's zone when unknown here.
    pub zone: Option<String>,
    pub data: Loadable<Stats>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            period: Period::default(),
            view: View::default(),
            project: false,
            zone: jiff::tz::TimeZone::system().iana_name().map(str::to_owned),
            data: Loadable::default(),
        }
    }
}

impl App {
    pub(crate) fn load_stats(&mut self) -> Vec<Action> {
        let s = &mut self.settings.stats;
        vec![Action::LoadStats {
            generation: s.data.start(),
            period: s.period.query(),
            project: s.project,
            zone: s.zone.clone(),
        }]
    }

    /// `p`, `v` and `s`; anything else scrolls.
    pub(crate) fn stats_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Char('p') => {
                self.settings.stats.period = self.settings.stats.period.next();
                self.load_stats()
            }
            KeyCode::Char('s') => {
                self.settings.stats.project = !self.settings.stats.project;
                self.load_stats()
            }
            KeyCode::Char('v') => {
                let s = &mut self.settings.stats;
                s.view = match s.view {
                    View::Overview => View::Models,
                    View::Models => View::Overview,
                };
                self.settings.scroll = 0;
                Vec::new()
            }
            code => {
                self.scroll_key(code);
                Vec::new()
            }
        }
    }
}

fn styled(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// `[ Overview ]  Models`, the active one in the selection style.
fn subtabs(app: &App) -> Line<'static> {
    let t = &app.theme;
    let mut spans = Vec::new();
    for (view, label) in [(View::Overview, "Overview"), (View::Models, "Models")] {
        let style = if view == app.settings.stats.view {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        spans.push(styled(format!(" {label} "), style));
        spans.push(Span::raw("  "));
    }
    Line::from(spans)
}

/// `All time · Last 7 days · Last 30 days`, then the scope.
fn periods(app: &App) -> Line<'static> {
    let t = &app.theme;
    let mut spans = Vec::new();
    for (i, period) in Period::ALL.into_iter().enumerate() {
        if i > 0 {
            spans.push(styled(" · ", Style::default().fg(t.text_muted)));
        }
        let style = if period == app.settings.stats.period {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_muted)
        };
        spans.push(styled(period.label(), style));
    }
    let scope = if app.settings.stats.project {
        "This project"
    } else {
        "All projects"
    };
    spans.push(styled(format!("    {scope}"), Style::default().fg(t.text)));
    Line::from(spans)
}

/// `May 20`.
fn short_date(date: &str) -> String {
    date.parse::<Date>()
        .map(|d| format!("{} {}", heatmap::month(d), d.day()))
        .unwrap_or_default()
}

/// `—` when every call was unpriced, `≥ $x · N unpriced calls` when some were.
pub fn total_cost(totals: &Totals) -> String {
    match totals.cost {
        None => "—".into(),
        Some(cost) if totals.unpriced_steps > 0 => {
            format!(
                "≥ {} · {} unpriced calls",
                money(cost),
                totals.unpriced_steps
            )
        }
        Some(cost) => money(cost),
    }
}

/// One metric: the label, then its value parts (`(text, emphasized)`).
struct Metric {
    label: &'static str,
    parts: Vec<(String, bool)>,
}

impl Metric {
    fn new(label: &'static str, value: String) -> Self {
        Self {
            label,
            parts: vec![(value, false)],
        }
    }

    fn width(&self) -> usize {
        self.label.chars().count()
            + 2
            + self
                .parts
                .iter()
                .map(|p| p.0.chars().count())
                .sum::<usize>()
    }

    fn spans(&self, app: &App) -> Vec<Span<'static>> {
        let t = &app.theme;
        let mut spans = vec![styled(
            format!("{}: ", self.label),
            Style::default().fg(t.text),
        )];
        for (text, strong) in &self.parts {
            let mut style = Style::default().fg(t.accent);
            if *strong {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(styled(text.clone(), style));
        }
        spans
    }
}

fn streak(label: &'static str, days: u32) -> Metric {
    Metric {
        label,
        parts: vec![(days.to_string(), true), (" days".into(), false)],
    }
}

/// The metric rows, as `(left, right)` pairs in reading order.
fn metrics(s: &Stats) -> Vec<(Metric, Option<Metric>)> {
    let favorite = s
        .favorite_model
        .as_ref()
        .map_or_else(|| "—".to_owned(), |f| crate::sidebar::clean(&f.model));
    let longest = s
        .longest_session
        .as_ref()
        .map_or_else(|| "—".to_owned(), |l| duration(l.duration_ms));
    let most = s
        .most_active_day
        .as_deref()
        .map_or_else(|| "—".to_owned(), short_date);
    vec![
        (
            Metric {
                label: "Favorite model",
                parts: vec![(favorite, true)],
            },
            Some(Metric::new("Total tokens", count(s.totals.total_tokens))),
        ),
        (
            Metric::new("Sessions", s.totals.sessions.to_string()),
            Some(Metric::new("Longest session", longest)),
        ),
        (
            Metric::new(
                "Active days",
                format!("{}/{}", s.active_days, s.period_days),
            ),
            Some(streak("Longest streak", s.longest_streak)),
        ),
        (
            Metric::new("Most active day", most),
            Some(streak("Current streak", s.current_streak)),
        ),
        (Metric::new("Total cost", total_cost(&s.totals)), None),
    ]
}

fn metric_lines(app: &App, s: &Stats, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, (left, right)) in metrics(s).into_iter().enumerate() {
        if i == 1 {
            out.push(Line::default());
        }
        if width < TWO_COLUMNS {
            out.push(Line::from(left.spans(app)));
            out.extend(right.map(|r| Line::from(r.spans(app))));
            continue;
        }
        let mut spans = left.spans(app);
        if let Some(right) = right {
            spans.push(Span::raw(
                " ".repeat(COLUMN.saturating_sub(left.width()).max(2)),
            ));
            spans.extend(right.spans(app));
        }
        out.push(Line::from(spans));
    }
    out
}

/// `Input 75.7m · Output 564.9m · Reasoning 0 · Cache read 229.3b · Cache write 4.6b`.
fn token_line(app: &App, t: &Tokens) -> Line<'static> {
    let text = [
        ("Input", t.input),
        ("Output", t.output),
        ("Reasoning", t.reasoning),
        ("Cache read", t.cache_read),
        ("Cache write", t.cache_write),
    ]
    .map(|(label, n)| format!("{label} {}", count(n)))
    .join(" · ");
    Line::from(styled(text, Style::default().fg(app.theme.text_subtle)))
}

/// `Active 265 of 340 days; most active May 20; current streak 14 days`.
pub fn sentence(s: &Stats) -> String {
    let mut parts = vec![format!(
        "Active {} of {} days",
        s.active_days, s.period_days
    )];
    if let Some(day) = &s.most_active_day {
        parts.push(format!("most active {}", short_date(day)));
    }
    parts.push(format!("current streak {} days", s.current_streak));
    parts.join("; ")
}

fn overview(app: &App, s: &Stats, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    if app.screen_reader {
        out.push(Line::from(styled(sentence(s), Style::default().fg(t.text))));
    } else {
        out.extend(heatmap::lines(t, &s.dated(), width));
    }
    out.push(Line::default());
    out.push(periods(app));
    out.push(Line::default());
    out.extend(metric_lines(app, s, width));
    out.push(token_line(app, &s.totals.tokens));
}

/// The model name, with its provider when another provider has a model of that name.
fn model_name(s: &Stats, m: &Model) -> String {
    let shared = s.models.iter().filter(|o| o.model == m.model).count() > 1;
    if shared && !m.provider.is_empty() {
        crate::sidebar::clean(&format!("{}/{}", m.provider, m.model))
    } else {
        crate::sidebar::clean(&m.model)
    }
}

fn model_lines(app: &App, s: &Stats, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    out.push(periods(app));
    out.push(Line::default());
    if s.models.is_empty() {
        out.push(Line::from(styled(
            "No usage in this period",
            Style::default().fg(t.text_muted),
        )));
        return;
    }
    for m in &s.models {
        let filled = (m.share * BAR as f64).round().clamp(0.0, BAR as f64) as usize;
        let cost = match m.cost {
            None => "—".to_owned(),
            Some(cost) if m.unpriced_steps > 0 => format!("≥ {}", money(cost)),
            Some(cost) => money(cost),
        };
        out.push(Line::from(styled(
            model_name(s, m),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        )));
        out.push(Line::from(vec![
            Span::raw("  "),
            styled("█".repeat(filled), Style::default().fg(t.meter_fill)),
            styled("░".repeat(BAR - filled), Style::default().fg(t.meter_empty)),
            styled(
                format!(" {:>3.0}%", m.share * 100.0),
                Style::default().fg(t.accent),
            ),
            styled(
                format!(
                    "  {} tokens · {cost} · {} Turns · {} Sessions",
                    count(m.total_tokens),
                    m.turns,
                    m.sessions
                ),
                Style::default().fg(t.text_muted),
            ),
        ]));
    }
}

/// The tab's rows.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = vec![subtabs(app), Line::default()];
    let state = &app.settings.stats;
    let Some(stats) = &state.data.data else {
        match &state.data.error {
            Some(error) => out.push(super::status::failed(app, "stats", error)),
            None => out.push(Line::from(styled(
                "Loading…",
                Style::default().fg(t.text_muted),
            ))),
        }
        return out;
    };
    if stats.backfilling {
        out.push(Line::from(styled(
            "Indexing history…",
            Style::default().fg(t.text_muted),
        )));
    }
    match state.view {
        View::Overview => overview(app, stats, width, &mut out),
        View::Models => model_lines(app, stats, &mut out),
    }
    if stats.approximate_days {
        out.push(Line::from(styled(
            "Older days are UTC days from daily aggregates",
            Style::default().fg(t.text_muted),
        )));
    }
    if let Some(error) = &state.data.error {
        out.push(super::status::failed(app, "stats", error));
    }
    out
}
