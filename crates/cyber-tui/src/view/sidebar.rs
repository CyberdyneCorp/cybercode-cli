//! The Session sidebar (`tui` → Session sidebar): title, Context, Workflows, MCP, LSP
//! and Modified files from the top, `cwd:branch` and the connection dot with the version at the
//! bottom, on `surface_alt`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::layout::{shorten_path, truncate_end};
use super::wrap::wrap;
use crate::app::App;
use crate::sidebar::{Connection, FILE_ROWS, Row, Section};
use crate::theme::Tokens;

/// Columns of padding on each side.
const PAD: u16 = 2;
/// Context use above this percent is shown in the warning color.
const CONTEXT_WARNING: f64 = 85.0;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.surface_alt)),
        area,
    );
    if area.width <= PAD * 2 || area.height < 2 {
        return;
    }
    let inner = Rect {
        x: area.x + PAD,
        y: area.y + 1,
        width: area.width - PAD * 2,
        height: area.height - 1,
    };
    let width = inner.width as usize;
    let bottom = bottom(app, width);
    let bottom_height = (bottom.len() as u16).min(inner.height);
    let top_height = inner.height - bottom_height;
    let top: Vec<Line> = top(app, width)
        .into_iter()
        .take(top_height as usize)
        .collect();
    f.render_widget(
        Paragraph::new(top),
        Rect {
            height: top_height,
            ..inner
        },
    );
    f.render_widget(
        Paragraph::new(bottom),
        Rect {
            y: inner.y + top_height,
            height: bottom_height,
            ..inner
        },
    );
}

/// The sections, top to bottom, wrapped to `width`.
pub fn top(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let bold = Style::default()
        .fg(app.theme.text)
        .add_modifier(Modifier::BOLD);
    let title = if app.session.title.is_empty() {
        "New session".to_owned()
    } else {
        crate::sidebar::clean(&app.session.title)
    };
    out.extend(wrap(Line::from(Span::styled(title, bold)), width));
    if let Some(parent) = &app.sidebar.parent_title {
        out.push(muted_line(
            app,
            &truncate_end(&format!("↳ {parent}"), width),
        ));
    }
    context(app, &mut out);
    workflows(app, width, &mut out);
    mcp(app, width, &mut out);
    lsp(app, width, &mut out);
    files(app, width, &mut out);
    out
}

fn muted_line(app: &App, text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(app.theme.text_muted),
    ))
}

/// A section header; `›` and the selection style when focused.
fn header(app: &App, row: Option<Row>, text: String) -> Line<'static> {
    let style = Style::default()
        .fg(app.theme.text)
        .add_modifier(Modifier::BOLD);
    focusable(app, row, vec![Span::styled(text, style)])
}

/// The row's spans, marked when the sidebar focus rests on it.
fn focusable(app: &App, row: Option<Row>, spans: Vec<Span<'static>>) -> Line<'static> {
    let focused = row.is_some() && app.sidebar.focused() == row;
    if !focused {
        return Line::from(spans);
    }
    let style = app.theme.selected();
    let mut marked = vec![Span::styled("› ", style)];
    marked.extend(spans.into_iter().map(|s| Span::styled(s.content, style)));
    Line::from(marked)
}

fn context(app: &App, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    out.push(Line::default());
    out.push(header(app, None, "Context".into()));
    if let Some(meter) = app.session.context {
        out.push(muted_line(
            app,
            &format!("{} tokens", grouped(meter.used_tokens)),
        ));
        if let Some(percent) = meter.percent() {
            let style = if percent > CONTEXT_WARNING {
                Style::default().fg(t.warning)
            } else {
                Style::default().fg(t.text_muted)
            };
            out.push(Line::from(Span::styled(
                format!("{percent:.0}% used"),
                style,
            )));
        }
    }
    let cost = super::footer::cost(&app.session);
    out.push(muted_line(app, &format!("{cost} spent")));
}

/// Cells of a Workflows row's bar.
const WORKFLOW_BAR: usize = 6;

/// `audit  ▀▀····  31/100` per active run, the current Session's first, then `+N more`;
/// omitted when no run is active.
fn workflows(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let rows = &app.sidebar.workflows;
    if rows.is_empty() {
        return;
    }
    let t = &app.theme;
    out.push(Line::default());
    out.push(header(app, None, "Workflows".into()));
    for (i, run) in rows.iter().enumerate() {
        let count = format!(" {}/{}", run.done.min(run.total), run.total);
        let name_room = width.saturating_sub(WORKFLOW_BAR + count.len() + 1);
        let mut spans = vec![Span::styled(
            format!("{:<name_room$} ", truncate_end(&run.name, name_room)),
            Style::default().fg(t.text),
        )];
        spans.extend(super::progress::bar(
            app,
            run.done,
            run.total,
            WORKFLOW_BAR,
            0,
        ));
        spans.push(Span::styled(count, Style::default().fg(t.text_muted)));
        out.push(focusable(app, Some(Row::Workflow(i)), spans));
    }
    if app.sidebar.workflows_more > 0 {
        out.push(muted_line(
            app,
            &format!("+{} more", app.sidebar.workflows_more),
        ));
    }
}

/// The dot color of an MCP status word.
fn mcp_color(t: &Tokens, status: &str) -> Color {
    match status {
        "connected" => t.success,
        "failed" => t.error,
        "disabled" => t.text_subtle,
        _ => t.warning,
    }
}

fn mcp(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let servers = app.sidebar.servers();
    if servers.is_empty() {
        return;
    }
    let collapsed = app.sidebar.collapsed.contains(&Section::Mcp);
    out.push(Line::default());
    let arrow = if collapsed { "▶" } else { "▼" };
    out.push(header(
        app,
        Some(Row::Header(Section::Mcp)),
        format!("{arrow} MCP"),
    ));
    if collapsed {
        return;
    }
    for (i, server) in servers.iter().enumerate() {
        let word = server.word();
        let name_room = width.saturating_sub(word.len() + 3);
        out.push(focusable(
            app,
            Some(Row::Mcp(i)),
            vec![
                Span::styled("● ", Style::default().fg(mcp_color(t, &server.status))),
                Span::styled(
                    truncate_end(&server.name, name_room),
                    Style::default().fg(t.text),
                ),
                Span::styled(format!(" {word}"), Style::default().fg(t.text_muted)),
            ],
        ));
        if server.status == "failed"
            && let Some(error) = &server.error
        {
            out.push(Line::from(Span::styled(
                format!("  {}", truncate_end(error, width.saturating_sub(2))),
                Style::default().fg(t.error),
            )));
        }
    }
}

fn lsp(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let Some(servers) = app.sidebar.lsp.ready() else {
        return;
    };
    out.push(Line::default());
    out.push(header(app, None, "LSP".into()));
    if servers.is_empty() {
        out.extend(wrap(
            muted_line(app, "LSPs will activate as files are read"),
            width,
        ));
        return;
    }
    for server in servers {
        let color = match server.status.as_str() {
            "connected" => t.success,
            "broken" => t.error,
            _ => t.warning,
        };
        out.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(
                truncate_end(&server.id, width.saturating_sub(server.status.len() + 3)),
                Style::default().fg(t.text),
            ),
            Span::styled(
                format!(" {}", server.status),
                Style::default().fg(t.text_muted),
            ),
        ]));
    }
}

fn files(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let changes = app.sidebar.changes();
    if changes.is_empty() {
        return;
    }
    let collapsed = app.sidebar.collapsed.contains(&Section::Files);
    out.push(Line::default());
    let arrow = if collapsed { "▶" } else { "▼" };
    out.push(header(
        app,
        Some(Row::Header(Section::Files)),
        format!("{arrow} Modified files"),
    ));
    if collapsed {
        return;
    }
    for (i, change) in changes.iter().take(FILE_ROWS).enumerate() {
        let added = format!("+{}", change.additions);
        let removed = format!("-{}", change.deletions);
        let room = width.saturating_sub(added.len() + removed.len() + 2);
        out.push(focusable(
            app,
            Some(Row::File(i)),
            vec![
                Span::styled(
                    format!("{:<room$}", shorten_path(&change.file, room)),
                    Style::default().fg(t.text),
                ),
                Span::styled(format!(" {added}"), Style::default().fg(t.diff_added)),
                Span::styled(format!(" {removed}"), Style::default().fg(t.diff_removed)),
            ],
        ));
    }
    if changes.len() > FILE_ROWS {
        out.push(muted_line(
            app,
            &format!("+{} more", changes.len() - FILE_ROWS),
        ));
    }
}

/// `29,470`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The Location with home as `~`.
pub fn home_relative(app: &App) -> String {
    crate::sidebar::home_path(&app.session.directory, app.home.as_deref())
}

/// `~/work/app:main` and the worktree name; the branch is omitted outside git or when
/// the `vcs` route is unavailable.
pub fn location_spans(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        crate::sidebar::clean(&home_relative(app)),
        Style::default().fg(t.text_muted),
    )];
    let git = app.sidebar.vcs.ready().and_then(Option::as_ref);
    if let Some(branch) = git.and_then(|g| g.branch.as_ref()) {
        spans.push(Span::styled(":", Style::default().fg(t.text_muted)));
        spans.push(Span::styled(branch.clone(), Style::default().fg(t.text)));
    }
    if let Some(worktree) = git.and_then(|g| g.worktree.as_ref()) {
        spans.push(Span::styled(
            format!(" [{worktree}]"),
            Style::default().fg(t.secondary),
        ));
    }
    spans
}

fn bottom(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = wrap(Line::from(location_spans(app)), width);
    out.push(Line::default());
    let connection = app.sidebar.connection;
    let color = match connection {
        Connection::Connected => t.success,
        Connection::Reconnecting => t.warning,
        Connection::Disconnected => t.error,
    };
    let mut spans = vec![Span::styled("● ", Style::default().fg(color))];
    if connection != Connection::Connected || t.monochrome {
        spans.push(Span::styled(
            format!("{} · ", connection.word()),
            Style::default().fg(t.text_muted),
        ));
    }
    spans.push(Span::styled(
        "Cyber Code",
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    ));
    if let Some(version) = &app.sidebar.version {
        spans.push(Span::styled(
            format!(" {version}"),
            Style::default().fg(t.text_muted),
        ));
    }
    out.extend(wrap(Line::from(spans), width));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn token_counts_are_grouped() {
        assert_eq!(super::grouped(29_470), "29,470");
        assert_eq!(super::grouped(950), "950");
        assert_eq!(super::grouped(1_234_567), "1,234,567");
    }
}
