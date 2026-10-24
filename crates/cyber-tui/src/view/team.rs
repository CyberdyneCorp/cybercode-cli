//! The team view (`agent-teams` → Team view; design section 16): a members table (name,
//! role, status badge, current task, cost, last message) over the task board, whose
//! `todo`, `in_progress`, `blocked` and `done` columns show each task with its owner
//! (`cancelled` behind `c`); and the sidebar's Team section with member status dots.
//! Every status is also a word, so NO_COLOR and screen readers lose nothing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::layout::truncate_end;
use super::wrap::wrap;
use crate::app::App;
use crate::mouse::{List, Target};
use crate::team::{COLUMNS, Member, TeamView};
use crate::theme::Tokens;

const NAME: usize = 10;
const ROLE: usize = 9;
const STATUS: usize = 15;
const COST: usize = 7;
/// Columns the members table needs besides the task and last message.
const FIXED: usize = 2 + NAME + 1 + ROLE + 1 + STATUS + 1 + COST + 1;
/// The task and last-message columns share what is left once it is this wide.
const MESSAGE_FROM: usize = 40;

/// A member status's dot and color.
fn badge(t: &Tokens, status: &str) -> (&'static str, Color) {
    match status {
        "working" => ("●", t.success),
        "waiting_input" => ("◐", t.warning),
        "stopped" => ("■", t.text_subtle),
        _ => ("○", t.text_muted),
    }
}

fn cost(usd: f64) -> String {
    format!("${usd:.2}")
}

/// `/team`, full screen over the transcript.
pub(super) fn view(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    if area.width < 30 || area.height < 8 {
        return;
    }
    let team = app.sidebar.team.ready();
    let title = team.map_or_else(
        || " Team ".to_owned(),
        |team| format!(" Team {} · {} ", team.name, team.status_label()),
    );
    let summary = team.map_or_else(String::new, |team| {
        format!(
            " {} members · {}/{} done · {} ",
            team.members.len(),
            team.done(),
            team.tasks.len(),
            cost(team.cost_usd)
        )
    });
    let room = (area.width as usize).saturating_sub(summary.width() + 4);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            truncate_end(&title, room),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(summary, Style::default().fg(t.text_muted))).right_aligned(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let lines = match team {
        Some(team) => body_lines(app, team, body),
        None => vec![Line::from(Span::styled(
            "  Loading the team…",
            Style::default().fg(t.text_muted),
        ))],
    };
    f.render_widget(Paragraph::new(lines), body);
    let keys = Rect {
        y: inner.bottom().saturating_sub(2),
        height: 2,
        ..inner
    };
    f.render_widget(
        Paragraph::new(key_lines(app, width)).style(Style::default().fg(t.text_muted)),
        keys,
    );
}

/// The members table, then the board in the rows left; each member's row is a click
/// target over `body`.
fn body_lines(app: &App, team: &TeamView, body: Rect) -> Vec<Line<'static>> {
    let t = &app.theme;
    let (width, height) = (body.width as usize, body.height as usize);
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(t.text_muted);
    let rest = width.saturating_sub(FIXED);
    let (task, message) = if rest >= MESSAGE_FROM {
        let task = rest * 2 / 5;
        (task, rest - task - 1)
    } else {
        (rest, 0)
    };
    let mut lines = vec![Line::from(Span::styled(" Members", bold))];
    let mut head = format!(
        "  {:<NAME$} {:<ROLE$} {:<STATUS$} {:<task$} {:>COST$}",
        "member", "role", "status", "task", "cost"
    );
    if message > 0 {
        head.push_str(&format!(" {}", "last message"));
    }
    lines.push(Line::from(Span::styled(truncate_end(&head, width), muted)));
    for (i, member) in team.members.iter().enumerate() {
        let line = lines.len();
        super::push_lines(app, body, line..line + 1, 0, Target::row(List::Team, i));
        lines.push(member_line(
            app,
            member,
            i == app.team.selected,
            (task, message),
            width,
        ));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(" Board", bold)));
    let room = height.saturating_sub(lines.len());
    lines.extend(board(app, team, width, room));
    lines
}

/// `  api        teammate  ● working       Build the API …   $0.00 → lead: Done`.
fn member_line(
    app: &App,
    member: &Member,
    selected: bool,
    (task, message): (usize, usize),
    width: usize,
) -> Line<'static> {
    let t = &app.theme;
    let (dot, color) = badge(t, &member.status);
    let marker = if selected { "› " } else { "  " };
    let left = format!(
        "{marker}{:<NAME$} {:<ROLE$} ",
        truncate_end(&member.name, NAME),
        truncate_end(&member.role, ROLE)
    );
    let status = format!("{dot} {:<w$}", member.status, w = STATUS - 2);
    let current = member.task.as_deref().unwrap_or("—");
    let mut right = format!(
        " {:<task$} {:>COST$}",
        truncate_end(current, task),
        cost(member.cost_usd)
    );
    if message > 0 {
        let last = member.last_message.as_deref().unwrap_or("");
        right.push_str(&format!(" {}", truncate_end(last, message)));
    }
    if selected {
        let text = truncate_end(&format!("{left}{status}{right}"), width);
        let pad = width.saturating_sub(text.width());
        return Line::from(Span::styled(
            format!("{text}{}", " ".repeat(pad)),
            t.selected(),
        ));
    }
    Line::from(vec![
        Span::styled(left, Style::default().fg(t.text)),
        Span::styled(status, Style::default().fg(color)),
        Span::styled(right, Style::default().fg(t.text_muted)),
    ])
}

/// The board's columns side by side: a `todo (2)` header, then each task's title and
/// owner, cut to `height` rows with `+N more`.
fn board(app: &App, team: &TeamView, width: usize, height: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut statuses: Vec<&str> = COLUMNS.to_vec();
    if app.team.show_cancelled {
        statuses.push("cancelled");
    }
    let column = (width.saturating_sub(1) / statuses.len()).max(8);
    let cells: Vec<Vec<(String, Style)>> = statuses
        .iter()
        .map(|status| column_cells(t, team, status, column - 1, height))
        .collect();
    let rows = cells.iter().map(Vec::len).max().unwrap_or(0).min(height);
    (0..rows)
        .map(|row| {
            let mut spans = vec![Span::raw(" ")];
            for cell in &cells {
                let (text, style) = cell.get(row).cloned().unwrap_or_default();
                let pad = column.saturating_sub(text.width());
                spans.push(Span::styled(text, style));
                spans.push(Span::raw(" ".repeat(pad)));
            }
            Line::from(spans)
        })
        .collect()
}

/// One board column's rows, at most `height`.
fn column_cells(
    t: &Tokens,
    team: &TeamView,
    status: &str,
    width: usize,
    height: usize,
) -> Vec<(String, Style)> {
    let tasks: Vec<_> = team.column(status).collect();
    let header = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let mut cells = vec![(
        truncate_end(&format!("{status} ({})", tasks.len()), width),
        header,
    )];
    if tasks.is_empty() {
        cells.push(("—".into(), Style::default().fg(t.text_subtle)));
    }
    for (i, task) in tasks.iter().enumerate() {
        if cells.len() + 2 > height && i + 1 < tasks.len() {
            let more = format!("+{} more", tasks.len() - i);
            cells.push((more, Style::default().fg(t.text_subtle)));
            break;
        }
        cells.push((
            truncate_end(&format!("• {}", task.title), width),
            Style::default().fg(t.text),
        ));
        let mut owner = match &task.owner {
            Some(owner) => format!("  @{owner}"),
            None => "  unassigned".to_owned(),
        };
        let waits = team.waits(task);
        if waits > 0 {
            owner.push_str(&format!(" · waits {waits}"));
        }
        cells.push((
            truncate_end(&owner, width),
            Style::default().fg(t.text_muted),
        ));
    }
    cells
}

fn key_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let cancelled = if app.team.show_cancelled {
        "c hide cancelled"
    } else {
        "c show cancelled"
    };
    let keys = [
        "↑↓ member",
        "enter open its Session",
        cancelled,
        "r refresh",
        "esc close",
    ];
    let mut lines = vec![String::from(" ")];
    for key in keys {
        let last = lines.last_mut().expect("a line");
        let sep = if last.trim().is_empty() { "" } else { " · " };
        if last.width() + sep.len() + key.width() > width && !last.trim().is_empty() {
            lines.push(format!(" · {key}"));
        } else {
            last.push_str(sep);
            last.push_str(key);
        }
    }
    lines
        .into_iter()
        .map(|l| Line::from(Span::raw(l)))
        .collect()
}

/// The sidebar's Team section: `Team feature`, then each member with its status dot
/// (its status as a word for screen readers).
pub(super) fn sidebar(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let Some(team) = app.sidebar.team.ready() else {
        return;
    };
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let mut head = vec![Span::styled(format!("Team {}", team.name), bold)];
    if team.status != "active" {
        head.push(Span::styled(
            format!(" · {}", team.status_label()),
            Style::default().fg(t.warning),
        ));
    }
    out.push(Line::default());
    out.extend(wrap(Line::from(head), width));
    let mut spans = Vec::new();
    for (i, member) in team.members.iter().enumerate() {
        let sep = if i == 0 { "" } else { "  " };
        if app.screen_reader {
            spans.push(Span::styled(
                format!("{sep}{} {}", member.name, member.status),
                Style::default().fg(t.text),
            ));
            continue;
        }
        let (dot, color) = badge(t, &member.status);
        spans.push(Span::styled(
            format!("{sep}{dot} "),
            Style::default().fg(color),
        ));
        let style = if member.session_id == app.session.id {
            bold
        } else {
            Style::default().fg(t.text)
        };
        spans.push(Span::styled(member.name.clone(), style));
    }
    out.extend(wrap(Line::from(spans), width));
}
