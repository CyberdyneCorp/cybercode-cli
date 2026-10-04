//! The message list: Markdown-lite text, collapsible reasoning and tool cards.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::wrap::wrap;
use crate::app::App;
use crate::model::{Item, Tool};
use crate::theme::Theme;

const COLLAPSE_OVER: usize = 30;
const COLLAPSED_TAIL: usize = 10;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(1) as usize;
    let rows: Vec<Line<'static>> = lines(app)
        .into_iter()
        .flat_map(|l| wrap(l, width))
        .collect();
    let height = area.height as usize;
    let max_scroll = rows.len().saturating_sub(height);
    let scroll = (app.scroll as usize).min(max_scroll);
    let start = rows.len().saturating_sub(height + scroll);
    let visible: Vec<Line> = rows.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(visible), area);
}

/// Every line of the conversation, unwrapped.
pub fn lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = Vec::new();
    if app.items.is_empty() && app.streaming.is_empty() {
        out.push(Line::from(Span::styled(
            format!(
                "  {} — ask anything. /help for commands.",
                app.session.directory
            ),
            Style::default().fg(t.muted),
        )));
    }
    for item in &app.items {
        item_lines(app, item, &mut out);
        out.push(Line::default());
    }
    for (id, text) in &app.streaming {
        if !app.items.iter().any(|i| matches!(i, Item::Assistant { id: aid, text: done, .. } if aid == id && !done.is_empty())) {
            out.extend(markdown(text, t));
            out.push(Line::default());
        }
    }
    out
}

fn item_lines(app: &App, item: &Item, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    match item {
        Item::User { text, .. } => {
            for (i, line) in text.lines().enumerate() {
                let prefix = if i == 0 { "› " } else { "  " };
                out.push(Line::from(vec![
                    Span::styled(
                        prefix,
                        Style::default().fg(t.user).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(line.to_string(), Style::default().fg(t.user)),
                ]));
            }
        }
        Item::System { text, .. } => {
            let first = text
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('<'))
                .unwrap_or("context updated");
            out.push(Line::from(Span::styled(
                format!("  ⚙ {first}"),
                Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
            )));
        }
        Item::Assistant {
            id,
            text,
            reasoning,
            tools,
            error,
        } => {
            assistant(app, id, text, reasoning, tools, error.as_deref(), out);
        }
    }
}

fn assistant(
    app: &App,
    id: &str,
    text: &str,
    reasoning: &str,
    tools: &[Tool],
    error: Option<&str>,
    out: &mut Vec<Line<'static>>,
) {
    let t = &app.theme;
    if !reasoning.is_empty() {
        if app.show_reasoning {
            for line in reasoning.lines() {
                out.push(Line::from(Span::styled(
                    format!("  ∴ {line}"),
                    Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
                )));
            }
        } else {
            out.push(Line::from(Span::styled(
                "  ∴ thinking (Ctrl+T to show)",
                Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
            )));
        }
    }
    let shown = if text.is_empty() {
        app.streaming
            .get(id)
            .map(String::as_str)
            .unwrap_or_default()
    } else {
        text
    };
    out.extend(markdown(shown, t));
    for tool in tools {
        tool_card(app, tool, out);
    }
    if let Some(e) = error {
        out.push(Line::from(Span::styled(
            format!("  ✗ {e}"),
            Style::default().fg(t.error),
        )));
    }
}

fn status_style(t: &Theme, status: &str) -> Style {
    let color = match status {
        "completed" => t.success,
        "failed" | "unknown" => t.error,
        "interrupted" => t.warning,
        _ => t.accent,
    };
    Style::default().fg(color)
}

fn tool_card(app: &App, tool: &Tool, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let status = tool.display_status();
    out.push(Line::from(vec![
        Span::styled("  ⏺ ", status_style(t, status)),
        Span::styled(
            tool.name.clone(),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", truncate(&tool.summary(), 80)),
            Style::default().fg(t.muted),
        ),
        Span::styled(format!("  {status}"), status_style(t, status)),
    ]));
    let lines: Vec<&str> = tool.output.lines().collect();
    let collapsed = !app.expand_tools && lines.len() > COLLAPSE_OVER;
    let shown = if collapsed {
        &lines[lines.len() - COLLAPSED_TAIL..]
    } else {
        &lines[..]
    };
    if collapsed {
        out.push(Line::from(Span::styled(
            format!(
                "    … {} lines hidden (Ctrl+O to expand)",
                lines.len() - COLLAPSED_TAIL
            ),
            Style::default().fg(t.muted),
        )));
    }
    for line in shown {
        out.push(Line::from(vec![
            Span::styled("    │ ", Style::default().fg(t.border)),
            Span::styled(line.to_string(), diff_style(t, line)),
        ]));
    }
}

/// Unified-diff lines are colored; other output is muted.
fn diff_style(t: &Theme, line: &str) -> Style {
    if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(t.success)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(t.error)
    } else if line.starts_with("@@") {
        Style::default().fg(t.accent)
    } else {
        Style::default().fg(t.muted)
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Headings, bullets, fenced code and inline `code`/**bold**.
pub fn markdown(text: &str, t: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            let lang = raw.trim_start().trim_start_matches('`');
            if in_code && !lang.is_empty() {
                out.push(Line::from(Span::styled(
                    format!("  ┌ {lang}"),
                    Style::default().fg(t.muted),
                )));
            }
            continue;
        }
        if in_code {
            out.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(t.border)),
                Span::styled(raw.to_string(), Style::default().fg(t.code)),
            ]));
            continue;
        }
        out.push(prose(raw, t));
    }
    out
}

fn prose(raw: &str, t: &Theme) -> Line<'static> {
    let trimmed = raw.trim_start();
    if let Some(h) = trimmed.strip_prefix('#') {
        let title = h.trim_start_matches('#').trim().to_string();
        return Line::from(Span::styled(
            format!("  {title}"),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    }
    let indent = raw.len() - trimmed.len();
    let (bullet, body) = match trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        Some(rest) => (format!("{}• ", " ".repeat(indent)), rest),
        None => (" ".repeat(indent), trimmed),
    };
    let mut spans = vec![Span::raw(format!("  {bullet}"))];
    spans.extend(inline(body, t));
    Line::from(spans)
}

/// Alternate plain and `code` segments; `**bold**` inside plain text.
fn inline(text: &str, t: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, segment) in text.split('`').enumerate() {
        if i % 2 == 1 {
            spans.push(Span::styled(
                segment.to_string(),
                Style::default().fg(t.code),
            ));
            continue;
        }
        for (j, part) in segment.split("**").enumerate() {
            let style = if j % 2 == 1 {
                Style::default().fg(t.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            if !part.is_empty() {
                spans.push(Span::styled(part.to_string(), style));
            }
        }
    }
    spans
}
