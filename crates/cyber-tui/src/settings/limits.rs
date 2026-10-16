//! Provider-reported usage windows in the Usage tab (`tui` → Usage limit meters, P2),
//! from `GET /usage/limits`: one meter per window a provider reported, with its reset
//! time and how old the reading is. `d` and `w` show a provider's daily or weekly windows
//! when it reports both. Nothing is ever estimated: no reading, no meter.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde::Deserialize;
use serde_json::Value;

use super::Loadable;
use super::insights::Window;
use crate::app::{Action, App};

/// Readings older than this show their age.
const STALE_MS: i64 = 5 * 60 * 1000;
/// The most windows shown; a provider cannot fill the tab.
const MAX_WINDOWS: usize = 16;

/// One reported window.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Reading {
    pub provider: String,
    pub window: String,
    pub label: String,
    pub period: String,
    pub used_percent: f64,
    pub resets_ms: Option<i64>,
    pub observed_ms: i64,
}

/// Parse and check `data.limits`; anything malformed refuses the whole reply.
pub fn parse(value: &Value) -> Result<Vec<Reading>, String> {
    let mut readings: Vec<Reading> = serde_json::from_value(value["limits"].clone())
        .map_err(|_| "Malformed usage limits".to_string())?;
    let ok = readings.iter().all(|r| {
        matches!(r.period.as_str(), "daily" | "weekly")
            && r.used_percent.is_finite()
            && (0.0..=100.0).contains(&r.used_percent)
            && r.observed_ms > 0
            && r.resets_ms.is_none_or(|ms| ms > 0)
    });
    if !ok {
        return Err("Invalid usage limits".into());
    }
    for r in &mut readings {
        r.provider = crate::sidebar::clean(&r.provider);
        r.label = crate::sidebar::clean(&r.label);
    }
    readings.sort_by(|a, b| {
        (&a.provider, &a.period, &a.window).cmp(&(&b.provider, &b.period, &b.window))
    });
    Ok(readings)
}

/// The last reply, and the clock and zone a snapshot fixes.
#[derive(Debug, Default)]
pub struct LimitsState {
    pub data: Loadable<Vec<Reading>>,
    /// A fixed wall clock in Unix milliseconds (snapshot tests); `None` reads the clock.
    pub clock_ms: Option<i64>,
    /// A fixed zone for reset times (snapshot tests); `None` is the local zone.
    pub zone: Option<String>,
}

impl LimitsState {
    fn wall_ms(&self) -> i64 {
        self.clock_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64)
        })
    }
}

impl App {
    pub(crate) fn load_limits(&mut self) -> Vec<Action> {
        vec![Action::LoadLimits {
            generation: self.settings.limits.data.start(),
        }]
    }
}

/// The readings of the selected view: a provider that reports both daily and weekly
/// windows shows only those of `view`; one that reports a single kind shows them all.
pub fn shown(readings: &[Reading], view: Window) -> Vec<&Reading> {
    let wanted = match view {
        Window::Day => "daily",
        Window::Week => "weekly",
    };
    let both = |provider: &str| {
        let mut periods = readings
            .iter()
            .filter(|r| r.provider == provider)
            .map(|r| r.period.as_str());
        let first = periods.next();
        periods.any(|p| Some(p) != first)
    };
    readings
        .iter()
        .filter(|r| r.period == wanted || !both(&r.provider))
        .take(MAX_WINDOWS)
        .collect()
}

/// `3m`, `2h 5m`, `4d 1h`.
fn age(ms: i64) -> String {
    let m = ms / 60_000;
    match m {
        ..60 => format!("{m}m"),
        60..1440 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d {}h", m / 1440, m / 60 % 24),
    }
}

/// One meter per shown window; nothing when no provider reported a window.
pub fn meters(app: &App, out: &mut Vec<Line<'static>>) {
    let state = &app.settings.limits;
    let Some(readings) = &state.data.data else {
        return;
    };
    let now = state.wall_ms();
    for r in shown(readings, app.settings.insights.window) {
        out.push(Line::default());
        let label = format!("{} · {}", r.provider, r.label);
        super::usage::meter_rows(app, label, r.used_percent, out);
        let mut notes = Vec::new();
        if let Some(reset) = r.resets_ms {
            notes.push(format!(
                "Resets {}",
                super::usage::reset_when(reset, now, state.zone.as_deref())
            ));
        }
        let old = now.saturating_sub(r.observed_ms);
        if old > STALE_MS {
            notes.push(format!("Reading from {} ago", age(old)));
        }
        if !notes.is_empty() {
            out.push(Line::from(Span::styled(
                notes.join(" · "),
                Style::default().fg(app.theme.text_muted),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading(provider: &str, window: &str, period: &str) -> Value {
        json!({ "provider": provider, "window": window, "label": format!("{window} limit"),
            "period": period, "used_percent": 40.0, "resets_ms": null, "observed_ms": 1 })
    }

    #[test]
    fn malformed_replies_are_refused_and_text_is_cleaned() {
        let good = json!({ "limits": [reading("a\u{1b}[2J", "5h", "daily")] });
        let parsed = parse(&good).unwrap();
        assert!(!parsed[0].provider.contains('\u{1b}'));
        for bad in [
            json!({}),
            json!({ "limits": [{ "provider": "a" }] }),
            json!({ "limits": [reading("a", "5h", "monthly")] }),
            json!({ "limits": [{ "provider": "a", "window": "5h", "label": "x",
                "period": "daily", "used_percent": 140.0, "resets_ms": null, "observed_ms": 1 }] }),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
        assert!(parse(&json!({ "limits": [] })).unwrap().is_empty());
    }

    #[test]
    fn day_and_week_split_only_providers_that_report_both() {
        let readings = parse(&json!({ "limits": [
            reading("plan", "5h", "daily"),
            reading("plan", "7d", "weekly"),
            reading("api", "rate_requests", "daily"),
            reading("week", "7d", "weekly"),
        ] }))
        .unwrap();
        let names = |view| {
            shown(&readings, view)
                .iter()
                .map(|r| format!("{}/{}", r.provider, r.window))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Window::Day),
            ["api/rate_requests", "plan/5h", "week/7d"]
        );
        assert_eq!(
            names(Window::Week),
            ["api/rate_requests", "plan/7d", "week/7d"]
        );
        assert_eq!(age(3 * 60_000), "3m");
        assert_eq!(age(125 * 60_000), "2h 5m");
        assert_eq!(age((4 * 1440 + 60) * 60_000), "4d 1h");
    }
}
