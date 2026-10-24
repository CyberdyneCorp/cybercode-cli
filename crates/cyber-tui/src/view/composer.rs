//! The composer (`tui` → Composer appearance): a `surface` block with a left accent
//! bar in the input mode's color, 1–8 text rows, and an info line with the agent,
//! model, provider and Mode, plus the running state. Pending paste, image and PDF
//! chips are drawn in `accent` on `selection`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, InputMode, Overlay};

const MAX_ROWS: u16 = 8;
/// The bar, then a space, before the text.
const INDENT: u16 = 2;
const PLACEHOLDER: &str = "Ask anything… \"/\" commands · \"@\" files · \"!\" shell";

/// Rows: a padding row, the text, a padding row and the info line.
pub fn height(app: &App) -> u16 {
    text_rows(app) + 3
}

fn text_rows(app: &App) -> u16 {
    (app.composer.lines().len() as u16).clamp(1, MAX_ROWS)
}

/// The color of the agent's configured `color`, else `accent`. An unparsable value
/// (agent files are untrusted) falls back to `accent`.
pub(super) fn agent_color(app: &App, agent: &str) -> Color {
    let t = &app.theme;
    if t.monochrome {
        return Color::Reset;
    }
    app.catalog
        .agent_color(agent)
        .and_then(crate::theme::color::parse)
        .filter(|c| *c != Color::Reset)
        .map_or(t.accent, |c| {
            crate::theme::color::reduce(c, app.appearance.depth)
        })
}

fn bar_color(app: &App) -> Color {
    let t = &app.theme;
    match app.input_mode() {
        InputMode::Shell | InputMode::Background => t.warning,
        InputMode::Command => t.info,
        InputMode::Prompt => agent_color(app, &app.session.agent),
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width <= INDENT {
        return;
    }
    let t = &app.theme;
    let surface = Style::default().fg(t.text).bg(t.surface);
    let bar = Span::styled("┃ ", Style::default().fg(bar_color(app)).bg(t.surface));
    let rows = text_rows(app);
    let (row, col) = app.composer.cursor();
    let skip = (row as u16).saturating_sub(rows - 1);
    let mut lines = vec![Line::from(bar.clone())];
    if app.composer.is_empty() {
        lines.push(Line::from(vec![
            bar.clone(),
            Span::styled(PLACEHOLDER, Style::default().fg(t.text_subtle)),
        ]));
    } else {
        let labels: Vec<String> = app.attachments.chips.iter().map(|c| c.label()).collect();
        let text = app.composer.lines();
        for line in text.iter().skip(skip as usize).take(rows as usize) {
            let mut spans = vec![bar.clone()];
            spans.extend(chip_spans(line, &labels, chip_style(app)));
            lines.push(Line::from(spans));
        }
    }
    while lines.len() < rows as usize + 2 {
        lines.push(Line::from(bar.clone()));
    }
    let mut info = vec![bar];
    info.extend(info_line(app));
    lines.push(Line::from(info));
    f.render_widget(Paragraph::new(lines).style(surface), area);
    if matches!(app.overlay, Overlay::None) && app.code_select.is_none() {
        let before: String = app.composer.lines()[row].chars().take(col).collect();
        let x = area.x + INDENT + before.width() as u16;
        f.set_cursor_position((
            x.min(area.right().saturating_sub(1)),
            area.y + 1 + row as u16 - skip,
        ));
    }
}

/// Chips stand out from typed text; under `NO_COLOR` in reverse video.
fn chip_style(app: &App) -> Style {
    let t = &app.theme;
    if t.monochrome {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(t.accent).bg(t.selection)
    }
}

/// `line` split into plain text and the chip `labels` it contains, styled `chip`.
fn chip_spans(line: &str, labels: &[String], chip: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = line;
    // The earliest label in what is left, then the text before it.
    while let Some((at, label)) = labels
        .iter()
        .filter_map(|label| Some((rest.find(label.as_str())?, label)))
        .min_by_key(|&(at, _)| at)
    {
        if at > 0 {
            spans.push(Span::raw(rest[..at].to_owned()));
        }
        spans.push(Span::styled(label.clone(), chip));
        rest = &rest[at + label.len()..];
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest.to_owned()));
    }
    spans
}

/// `<Agent> · <model> <provider> · <Mode>`, or the input mode for `!` and `/`; while a
/// Drain runs without the progress line (`tui.spinner` `off`) it ends with
/// `esc interrupt`.
fn info_line(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let s = &app.session;
    let mut spans = match app.input_mode() {
        InputMode::Shell => vec![Span::styled("shell", Style::default().fg(t.warning))],
        InputMode::Background => vec![Span::styled(
            "background shell",
            Style::default().fg(t.warning),
        )],
        InputMode::Command => vec![Span::styled("command", Style::default().fg(t.info))],
        InputMode::Prompt => {
            let (provider, variant) = provider_and_variant(&s.model);
            let mut spans = vec![
                Span::styled(
                    crate::turns::title_case(&s.agent),
                    Style::default().fg(agent_color(app, &s.agent)),
                ),
                Span::styled(" · ", muted),
                Span::styled(
                    app.catalog.model_name(&s.model),
                    Style::default().fg(t.text),
                ),
                Span::styled(format!(" {provider}"), muted),
                Span::styled(format!(" · {}", app.mode_label()), muted),
            ];
            if let Some(variant) = variant {
                spans.push(Span::styled(format!(" · {variant}"), muted));
            }
            spans
        }
    };
    if let Some(entry) = app.admissions.pending(&s.id) {
        spans.push(Span::styled(
            format!("   delegation {} · esc cancels request", entry.status),
            muted,
        ));
    } else if s.running && !super::progress::shown(app) {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("esc", Style::default().fg(t.text)));
        spans.push(Span::styled(" interrupt", muted));
    }
    spans
}

/// `provider/model#variant` → (`provider`, `Some(variant)`).
pub(super) fn provider_and_variant(reference: &str) -> (&str, Option<&str>) {
    let (base, variant) = reference
        .split_once('#')
        .map_or((reference, None), |(b, v)| (b, Some(v)));
    (base.split_once('/').map_or("", |(p, _)| p), variant)
}
