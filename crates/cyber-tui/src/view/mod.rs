//! Drawing (`tui` → Message and part rendering, Permission prompt, Question prompt).

mod messages;
mod overlays;
mod wrap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Overlay};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let composer_height = (app.composer.lines().len() as u16 + 2).clamp(3, 10);
    let queued_height = (app.queued.len() as u16).min(4);
    let [header, body, queued, status, composer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(queued_height),
        Constraint::Length(1),
        Constraint::Length(composer_height),
    ])
    .areas(area);
    draw_header(f, app, header);
    messages::draw(f, app, body);
    draw_queued(f, app, queued);
    draw_status(f, app, status);
    draw_composer(f, app, composer);
    if let Some(c) = &app.completion {
        overlays::completion(f, app, c, composer);
    }
    match &app.overlay {
        Overlay::None => {}
        Overlay::Picker(p) => overlays::picker(f, app, p, area),
        Overlay::Permission(step) => overlays::permission(f, app, step, area),
        Overlay::Question(form) => overlays::question(f, app, form, area),
        Overlay::Help => overlays::help(f, app, area),
        Overlay::ConfirmStopTasks => overlays::confirm_stop_tasks(f, app, area),
        Overlay::ConfirmBypass => overlays::confirm_bypass(f, app, area),
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let s = &app.session;
    let status = if s.running { "running" } else { "idle" };
    let left = Line::from(vec![
        Span::styled(
            " cyber ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("· {} ", s.title), Style::default().fg(t.text)),
        Span::styled(
            format!("· {} · {} · {status}", s.model, app.mode_label()),
            Style::default().fg(t.muted),
        ),
    ]);
    let right = format!(
        "{} in / {} out · ${:.4} ",
        s.input_tokens, s.output_tokens, s.cost
    );
    let width = area.width.saturating_sub(right.len() as u16);
    f.render_widget(Paragraph::new(left), Rect { width, ..area });
    let r = Rect {
        x: area.x + width,
        width: area.width - width,
        ..area
    };
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(t.muted))),
        r,
    );
}

fn draw_queued(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app
        .queued
        .iter()
        .take(area.height as usize)
        .map(|q| {
            let first = q.text.lines().next().unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    format!(" ⏳ {} ", q.delivery),
                    Style::default().fg(app.theme.warning),
                ),
                Span::raw(first.to_string()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let text = match &app.toast {
        Some((msg, at)) if at.elapsed().as_secs() < 6 => {
            Span::styled(format!(" {msg}"), Style::default().fg(t.warning))
        }
        _ if app.session.running => Span::styled(
            " working… Esc to interrupt · Enter steers · Tab queues",
            Style::default().fg(t.muted),
        ),
        _ => Span::styled(
            " Enter send · Shift+Enter newline · / commands · @ files · ! shell · Ctrl+X h help",
            Style::default().fg(t.muted),
        ),
    };
    f.render_widget(Paragraph::new(text), area);
}

fn draw_composer(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let title = format!(" {} ", app.mode_label());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(title, Style::default().fg(t.accent)));
    let inner = block.inner(area);
    let lines: Vec<Line> = app.composer.lines().into_iter().map(Line::from).collect();
    let (row, col) = app.composer.cursor();
    let skip = (row as u16).saturating_sub(inner.height.saturating_sub(1));
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((skip, 0))
            .style(Style::default().fg(t.text)),
        area,
    );
    if matches!(app.overlay, Overlay::None) {
        let before: String = app.composer.lines()[row].chars().take(col).collect();
        let x = inner.x + unicode_width::UnicodeWidthStr::width(before.as_str()) as u16;
        f.set_cursor_position((
            x.min(inner.right().saturating_sub(1)),
            inner.y + row as u16 - skip,
        ));
    }
}
