//! The Usage tab's contributing factors (`tui` → Usage contributing factors), from
//! `GET /usage/insights`: one bold line and one advice line per factor of at least 5%,
//! then the Subagents table. `d` and `w` switch between the last 24 hours and 7 days.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;
use serde_json::Value;

use super::Loadable;
use crate::app::{Action, App};

/// Factors below this share are not shown.
const MIN_SHARE: f64 = 0.05;
/// The most Subagents rows.
const MAX_GROUPS: usize = 8;
/// The width of the Subagents name column.
const GROUP: usize = 24;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Window {
    #[default]
    Day,
    Week,
}

impl Window {
    pub fn query(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Last 24h",
            Self::Week => "Last 7 days",
        }
    }

    fn empty(self) -> &'static str {
        match self {
            Self::Day => "No usage in the last 24h",
            Self::Week => "No usage in the last 7 days",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Factor {
    pub factor: String,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Group {
    pub group: String,
    pub kind: String,
    pub share: f64,
}

/// The `GET /usage/insights` reply.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Insights {
    pub window: String,
    pub basis: String,
    pub total: f64,
    pub factors: Vec<Factor>,
    pub subagents: Vec<Group>,
}

impl Insights {
    /// Parse and check a reply; anything malformed refuses the whole reply.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut insights: Self = serde_json::from_value(value.clone())
            .map_err(|_| "Malformed usage insights".to_string())?;
        let share_ok = |s: f64| s.is_finite() && (0.0..=1.0).contains(&s);
        let ok = matches!(insights.basis.as_str(), "cost" | "tokens")
            && matches!(insights.window.as_str(), "24h" | "7d")
            && insights.total.is_finite()
            && insights.total >= 0.0
            && insights.factors.iter().all(|f| share_ok(f.share))
            && insights.subagents.iter().all(|g| {
                share_ok(g.share) && matches!(g.kind.as_str(), "agent" | "workflow" | "fork")
            });
        if !ok {
            return Err("Invalid usage insights".into());
        }
        for group in &mut insights.subagents {
            group.group = crate::sidebar::clean(&group.group);
        }
        insights
            .subagents
            .sort_by(|a, b| b.share.total_cmp(&a.share).then(a.group.cmp(&b.group)));
        Ok(insights)
    }
}

/// The window and the last reply.
#[derive(Debug, Default)]
pub struct InsightsState {
    pub window: Window,
    pub data: Loadable<Insights>,
}

impl App {
    pub(crate) fn load_insights(&mut self) -> Vec<Action> {
        let s = &mut self.settings.insights;
        vec![Action::LoadInsights {
            generation: s.data.start(),
            window: s.window.query(),
        }]
    }

    /// `d` and `w` in the Usage tab; `None` for any other key.
    pub(crate) fn insights_key(&mut self, c: char) -> Option<Vec<Action>> {
        let window = match c {
            'd' => Window::Day,
            'w' => Window::Week,
            _ => return None,
        };
        self.settings.insights.window = window;
        Some(self.load_insights())
    }
}

/// The factor's line and advice; `None` for a factor this TUI does not know.
fn describe(factor: &str) -> Option<(&'static str, &'static str)> {
    Some(match factor {
        "subagent_heavy" => (
            "subagent-heavy sessions",
            "Each subagent runs its own requests. Spawn them deliberately, and give simple subagents a cheaper model.",
        ),
        "long_running" => (
            "sessions active for 8+ hours",
            "These are often background or loop Sessions. Continuous usage adds up; make sure it is intended.",
        ),
        "large_context" => (
            "requests above 150k context",
            "Long contexts cost more even when cached. /compact mid-task, /new when switching tasks.",
        ),
        "workflow" => (
            "workflows",
            "A workflow fans a task out to many agents. Save workflows for big tasks.",
        ),
        _ => return None,
    })
}

/// Word-wrap trusted text to `width`, every row indented by `indent` columns.
fn words(text: &str, width: usize, indent: usize) -> Vec<String> {
    let room = width.saturating_sub(indent).max(10);
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > room {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(word);
    }
    rows.push(row);
    let pad = " ".repeat(indent);
    rows.into_iter().map(|r| format!("{pad}{r}")).collect()
}

fn percent(share: f64) -> String {
    format!("{:.0}%", (share * 100.0).round())
}

/// The section's rows, below the limit meters.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let state = &app.settings.insights;
    let mut out = vec![
        Line::default(),
        crate::settings::status::heading(app, "What's contributing to your usage?"),
        Line::from(Span::styled(
            "Approximate: Sessions recorded by this server only",
            muted,
        )),
        Line::default(),
    ];
    let Some(data) = &state.data.data else {
        out.push(match &state.data.error {
            Some(error) => crate::settings::status::failed(app, "usage insights", error),
            None => Line::from(Span::styled("Loading…", muted)),
        });
        return out;
    };
    if data.total <= 0.0 {
        out.push(Line::from(Span::styled(state.window.empty(), muted)));
        return out;
    }
    let basis = if data.basis == "cost" {
        "% of cost"
    } else {
        "% of tokens"
    };
    out.push(Line::from(Span::styled(
        format!(
            "{} · These are independent characteristics of your usage, not a breakdown · {basis}",
            state.window.label()
        ),
        muted,
    )));
    if let Some(error) = &state.data.error {
        out.push(crate::settings::status::failed(
            app,
            "usage insights",
            error,
        ));
    }
    factors(app, data, width, &mut out);
    groups(app, data, basis, &mut out);
    out
}

fn factors(app: &App, data: &Insights, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    for factor in data.factors.iter().filter(|f| f.share >= MIN_SHARE) {
        let Some((what, advice)) = describe(&factor.factor) else {
            continue;
        };
        out.push(Line::default());
        out.push(Line::from(Span::styled(
            format!("{} of your usage came from {what}", percent(factor.share)),
            bold,
        )));
        for row in words(advice, width, 2) {
            out.push(Line::from(Span::styled(
                row,
                Style::default().fg(t.text_muted),
            )));
        }
    }
}

fn groups(app: &App, data: &Insights, basis: &str, out: &mut Vec<Line<'static>>) {
    if data.subagents.is_empty() {
        return;
    }
    let t = &app.theme;
    out.push(Line::default());
    out.push(Line::from(Span::styled(
        format!("{:<GROUP$}{basis:>10}", "Subagents"),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    )));
    for group in data.subagents.iter().take(MAX_GROUPS) {
        let name = crate::view::layout::truncate_end(&group.group, GROUP - 1);
        out.push(Line::from(Span::styled(
            format!("{name:<GROUP$}{:>10}", percent(group.share)),
            Style::default().fg(t.text),
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply() -> Value {
        json!({
            "window": "24h", "basis": "cost", "total": 10.0,
            "factors": [
                { "factor": "subagent_heavy", "share": 0.62 },
                { "factor": "long_running", "share": 0.04 },
                { "factor": "large_context", "share": 0.84 },
                { "factor": "something_new", "share": 0.5 },
            ],
            "subagents": (0..10).map(|i| json!({
                "group": format!("agent{i}\u{1b}[31m"), "kind": "agent", "share": f64::from(i) / 100.0,
            })).collect::<Vec<_>>(),
        })
    }

    fn text(app: &App) -> Vec<String> {
        lines(app, 80).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn factors_above_five_percent_with_advice_and_eight_groups() {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        let generation = app.settings.insights.data.start();
        let parsed = Insights::parse(&reply());
        app.settings.insights.data.apply(generation, parsed);
        let rows = text(&app);
        assert!(rows.contains(&"62% of your usage came from subagent-heavy sessions".into()));
        assert!(rows.contains(&"84% of your usage came from requests above 150k context".into()));
        assert!(!rows.iter().any(|r| r.contains("8+ hours")), "4% is hidden");
        assert!(!rows.iter().any(|r| r.contains("something_new")));
        assert!(rows.iter().any(|r| r.starts_with("  Each subagent runs")));
        assert!(rows.iter().any(|r| r.contains("% of cost")));
        let groups: Vec<&String> = rows.iter().filter(|r| r.starts_with("agent")).collect();
        assert_eq!(groups.len(), 8, "{rows:?}");
        assert!(
            groups[0].starts_with("agent9[31m"),
            "highest first, escapes removed"
        );
        assert!(rows.iter().all(|r| !r.contains('\u{1b}')));
    }

    #[test]
    fn an_empty_window_and_malformed_replies() {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        let generation = app.settings.insights.data.start();
        let empty = json!({ "window": "24h", "basis": "cost", "total": 0.0,
            "factors": [], "subagents": [] });
        app.settings
            .insights
            .data
            .apply(generation, Insights::parse(&empty));
        assert!(text(&app).contains(&"No usage in the last 24h".into()));
        for bad in [
            json!({ "basis": "money" }),
            json!({ "window": "24h", "basis": "cost", "total": 1.0,
                "factors": [{ "factor": "large_context", "share": 1.5 }], "subagents": [] }),
            json!({ "window": "1y", "basis": "cost", "total": 1.0, "factors": [], "subagents": [] }),
        ] {
            assert!(Insights::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn d_and_w_switch_the_window() {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        let actions = app.insights_key('w').unwrap();
        assert!(matches!(
            &actions[..],
            [Action::LoadInsights { window: "7d", .. }]
        ));
        assert_eq!(app.settings.insights.window.label(), "Last 7 days");
        let actions = app.insights_key('d').unwrap();
        assert!(matches!(
            &actions[..],
            [Action::LoadInsights { window: "24h", .. }]
        ));
        assert!(app.insights_key('x').is_none());
        assert_eq!(words("a b c", 13, 2), ["  a b c"]);
    }
}
