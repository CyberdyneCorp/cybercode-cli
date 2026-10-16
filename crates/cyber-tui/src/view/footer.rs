//! The status footer (`tui` → Status line): `cwd` on the left, the other
//! `tui.statusline.fields` on the right, then the palette key hint from the binding
//! table. A toast replaces the left side for 6 seconds, and a `theme_cycle`
//! announcement for 2.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::layout::{shorten_path, truncate_end};
use crate::app::App;

const TOAST_SECS: u64 = 6;
/// Context use above this percent is shown in the warning color.
const CONTEXT_WARNING: f64 = 85.0;

/// `compact` (below 60 columns) shows only context percent and cost.
pub fn draw(f: &mut Frame, app: &App, area: Rect, compact: bool) {
    let t = &app.theme;
    if compact {
        let line = Line::from(compact_side(app)).alignment(ratatui::layout::Alignment::Right);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    let right = right_side(app);
    let right_width = right.iter().map(|s| s.content.width()).sum::<usize>() as u16;
    let room = area.width.saturating_sub(right_width + 2) as usize;
    let left = match &app.toast {
        // `theme_cycle`'s `Theme: <name>` for 2 seconds, over any toast.
        _ if app.announced().is_some() => Span::styled(
            format!(
                " {}",
                truncate_end(app.announced().unwrap_or_default(), room.saturating_sub(1))
            ),
            Style::default().fg(t.text),
        ),
        Some((msg, at)) if app.now().saturating_duration_since(*at).as_secs() < TOAST_SECS => {
            Span::styled(
                format!(" {}", truncate_end(msg, room.saturating_sub(1))),
                Style::default().fg(t.warning),
            )
        }
        _ if app.statusline_fields.iter().any(|f| f == "cwd") => Span::styled(
            format!(
                " {}",
                shorten_path(
                    &crate::sidebar::display_path(&app.session.directory),
                    room.saturating_sub(1)
                )
            ),
            Style::default().fg(t.text_muted),
        ),
        _ => Span::raw(""),
    };
    f.render_widget(Paragraph::new(Line::from(left)), area);
    let width = right_width.min(area.width);
    let r = Rect {
        x: area.right() - width,
        width,
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(right)), r);
}

/// The right-aligned fields, ` · `-separated, then `<key> commands`.
fn right_side(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let mut spans = Vec::new();
    for field in app.statusline_fields.iter().filter(|f| *f != "cwd") {
        let Some(span) = field_span(app, field) else {
            continue;
        };
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", muted));
        }
        spans.push(span);
    }
    if let Some(key) = app.keymap.label("global", "command_palette") {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(key, Style::default().fg(t.text)));
        spans.push(Span::styled(" commands", muted));
    }
    spans.push(Span::raw(" "));
    spans
}

/// `22% · $0.00`: the percent alone (tokens without a known window), then the cost.
fn compact_side(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let mut spans = Vec::new();
    if let Some(meter) = app.session.context {
        let text = match meter.percent() {
            Some(percent) => format!("{percent:.0}%"),
            None => compact(meter.used_tokens),
        };
        let warn = meter.percent().is_some_and(|p| p > CONTEXT_WARNING);
        spans.push(Span::styled(
            text,
            if warn {
                Style::default().fg(t.warning)
            } else {
                muted
            },
        ));
        spans.push(Span::styled(" · ", muted));
    }
    spans.push(Span::styled(cost(&app.session), muted));
    spans.push(Span::raw(" "));
    spans
}

/// A field's value, or `None` when there is nothing true to show.
fn field_span(app: &App, field: &str) -> Option<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let s = &app.session;
    let text = match field {
        "context" => {
            let meter = s.context?;
            let warn = meter.percent().is_some_and(|p| p > CONTEXT_WARNING);
            let style = if warn {
                Style::default().fg(t.warning)
            } else {
                muted
            };
            return Some(Span::styled(context(&meter), style));
        }
        "cost" => cost(s),
        "model" => app.catalog.model_name(&s.model),
        "agent" => crate::turns::title_case(&s.agent),
        "mode" => app.mode_label(),
        "variant" => s.model.split_once('#')?.1.to_owned(),
        "branch" => app.sidebar.vcs.ready()?.as_ref()?.branch.clone()?,
        "worktree" => app.sidebar.vcs.ready()?.as_ref()?.worktree.clone()?,
        // `goal`, `loops` and `remote` have no source yet.
        _ => return None,
    };
    Some(Span::styled(text, muted))
}

/// `29.5K (22%)`, or the tokens alone when the window is unknown.
pub fn context(meter: &crate::model::ContextMeter) -> String {
    let tokens = compact(meter.used_tokens);
    match meter.percent() {
        Some(percent) => format!("{tokens} ({percent:.0}%)"),
        None => tokens,
    }
}

/// `$0.00`; `—` when nothing was priced; `≥ $x` when part was not priced or descendant
/// history is incomplete.
pub fn cost(s: &crate::model::Session) -> String {
    let total = s.cost + s.children_cost;
    if s.unpriced_steps > 0 && total == 0.0 {
        return "—".into();
    }
    if s.unpriced_steps > 0 || s.children_incomplete {
        return format!("≥ ${total:.2}");
    }
    format!("${total:.2}")
}

/// `950`, `29.5K`, `1.2M`.
pub fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}K", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContextMeter, Session};

    #[test]
    fn values_follow_the_spec_forms() {
        let meter = ContextMeter {
            used_tokens: 29_470,
            window: Some(131_072),
            compact_at: None,
        };
        assert_eq!(context(&meter), "29.5K (22%)");
        let unknown = ContextMeter {
            window: None,
            ..meter
        };
        assert_eq!(context(&unknown), "29.5K");
        let mut s = Session::default();
        assert_eq!(cost(&s), "$0.00");
        s.unpriced_steps = 2;
        assert_eq!(cost(&s), "—");
        s.cost = 0.42;
        assert_eq!(cost(&s), "≥ $0.42");
        assert_eq!(compact(950), "950");
        assert_eq!(compact(1_200_000), "1.2M");
    }
}
