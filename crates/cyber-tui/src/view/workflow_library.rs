//! The `/workflows` Library tab: one row per library Workflow with its origin badge, the
//! args form of the selected one, or its source.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::layout::truncate_end;
use super::workflows::{frame, key_line, location, muted, pad, split};
use crate::app::App;
use crate::workflow_library::{Entry, Form, Library, SourceView};

/// The name column, before its padding.
const NAME: usize = 14;
/// The origin badge column: `[bundled]`.
const ORIGIN: usize = 10;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let library = &app.workflows.library;
    if let Some(view) = &library.source {
        return draw_source(f, app, view, area);
    }
    let right = match library.issues {
        0 => format!("{} Workflows", library.listed().len()),
        n => format!("{} Workflows · {n} failed to load", library.listed().len()),
    };
    let block = frame(
        app,
        area.width,
        format!("Workflows · Library · {}", location(app)),
        right,
    );
    let (body, keys) = split(&block, area);
    f.render_widget(block, area);
    let inner = body.width as usize;
    let mut lines = list_lines(app, library, inner);
    if let Some(form) = &library.form {
        lines.push(Line::default());
        lines.extend(form_lines(app, form, inner));
    }
    let skip = lines.len().saturating_sub(body.height as usize);
    f.render_widget(Paragraph::new(lines).scroll((skip as u16, 0)), body);
    let hints = match &library.form {
        Some(form) if form.posting => vec!["starting…".to_owned()],
        Some(_) => vec!["enter start".into(), "ctrl+u clear".into(), "esc cancel".into()],
        None => vec![
            "enter run".into(),
            "o source".into(),
            "tab runs".into(),
            "esc".into(),
        ],
    };
    f.render_widget(Paragraph::new(key_line(app, inner, hints)), keys);
}

fn list_lines(app: &App, library: &Library, inner: usize) -> Vec<Line<'static>> {
    let entries = &library.entries;
    match &entries.data {
        _ if entries.data.is_none() && entries.error.is_some() => {
            vec![muted(app, format!(" {}", entries.error.clone().unwrap_or_default()))]
        }
        None => vec![muted(app, " Loading…")],
        Some(listed) if listed.is_empty() => vec![muted(
            app,
            " No Workflows. Add modules to .cyber/workflows/ or ~/.config/cyber/workflows/.",
        )],
        Some(listed) => listed
            .iter()
            .enumerate()
            .map(|(index, entry)| row(app, entry, index == library.selected, inner))
            .collect(),
    }
}

fn row(app: &App, entry: &Entry, selected: bool, inner: usize) -> Line<'static> {
    let t = &app.theme;
    let marker = if selected { " ▸ " } else { "   " };
    let badge = format!("[{}]", entry.origin);
    let used = 3 + NAME + 1 + ORIGIN + 1;
    let mut description = entry.description.clone();
    if !entry.shadowed.is_empty() {
        description.push_str(&format!(" (shadows {})", entry.shadowed.join(", ")));
    }
    let name_style = if selected {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.text)
    };
    let badge_color = match entry.origin.as_str() {
        "bundled" => t.text_muted,
        "project" => t.success,
        _ => t.info,
    };
    Line::from(vec![
        Span::styled(marker.to_owned(), Style::default().fg(t.accent)),
        Span::styled(pad(&entry.name, NAME), name_style),
        Span::raw(" "),
        Span::styled(pad(&badge, ORIGIN), Style::default().fg(badge_color)),
        Span::raw(" "),
        Span::styled(
            truncate_end(&description, inner.saturating_sub(used)),
            Style::default().fg(t.text_muted),
        ),
    ])
}

fn form_lines(app: &App, form: &Form, inner: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = vec![Line::from(Span::styled(
        format!(" Run {}", form.name),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    ))];
    if let Some(estimate) = &form.estimate {
        lines.push(muted(app, truncate_end(&format!(" agents: {estimate}"), inner)));
    }
    lines.push(muted(app, " args (JSON):"));
    // The end of the text stays visible: that is where typing happens.
    let room = inner.saturating_sub(4);
    let chars: Vec<char> = form.text.chars().collect();
    let shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
    lines.push(Line::from(vec![
        Span::styled(" › ".to_owned(), Style::default().fg(t.accent)),
        Span::styled(shown, Style::default().fg(t.text)),
        Span::styled("▏".to_owned(), Style::default().fg(t.accent)),
    ]));
    if let Some(error) = &form.error {
        lines.push(Line::from(Span::styled(
            truncate_end(&format!(" {error}"), inner),
            Style::default().fg(t.error),
        )));
    }
    lines
}

fn draw_source(f: &mut Frame, app: &App, view: &SourceView, area: Rect) {
    let block = frame(
        app,
        area.width,
        format!("Workflows · {} · source", view.name),
        "read-only".into(),
    );
    let (body, keys) = split(&block, area);
    f.render_widget(block, area);
    let inner = body.width as usize;
    let lines: Vec<Line<'static>> = view
        .lines
        .iter()
        .map(|line| Line::from(truncate_end(line, inner)))
        .collect();
    let most = (lines.len() as u16).saturating_sub(body.height);
    f.render_widget(Paragraph::new(lines).scroll((view.scroll.min(most), 0)), body);
    let hints = vec!["↑↓ scroll".into(), "esc back".into()];
    f.render_widget(Paragraph::new(key_line(app, inner, hints)), keys);
}
