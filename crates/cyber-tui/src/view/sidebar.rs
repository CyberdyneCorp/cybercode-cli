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
use crate::mouse::{Act, List, Target};
use crate::sidebar::{Connection, FILE_ROWS, MCP_ROWS, McpServer, Row, Section};
use crate::theme::Tokens;

/// Columns of padding on each side.
const PAD: u16 = 2;

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
    let sections = sections(app, width, top_height as usize);
    app.sidebar.set_drawn_reachable(
        sections
            .rows
            .iter()
            .filter_map(|(_, row)| match row {
                Row::Reachable(key) => Some(key.clone()),
                _ => None,
            })
            .collect(),
    );
    let shown = !app.sidebar.collapsed.contains(&Section::Sessions)
        && sections.rows.iter().any(|(line, row)| {
            *row == Row::Header(Section::Sessions) && line.saturating_add(1) < top_height as usize
        });
    app.overview_visible
        .store(shown, std::sync::atomic::Ordering::Relaxed);
    push_rows(
        app,
        Rect {
            x: area.x,
            width: area.width,
            height: top_height,
            ..inner
        },
        &sections.rows,
    );
    let top: Vec<Line> = sections
        .lines
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

/// Section headers act on one click, as `<leader>e` and Enter on them; other rows
/// take a second click.
fn push_rows(app: &App, area: Rect, rows: &[(usize, Row)]) {
    let order = app.sidebar.rows();
    for (line, row) in rows {
        let Some(index) = order.iter().position(|r| r == row) else {
            continue;
        };
        let act = match row {
            Row::Header(_) => Act::First,
            _ => Act::Second,
        };
        let target = match row {
            Row::Reachable(key) => Target::SidebarEntry(key.clone()),
            Row::Directory => Target::SidebarDirectory,
            _ => Target::Row {
                list: List::Sidebar,
                index,
                act,
            },
        };
        super::push_lines(app, area, *line..*line + 1, 0, target);
    }
}

/// The sections' lines, and the line each focusable row is drawn on.
#[derive(Default)]
struct Out {
    lines: Vec<Line<'static>>,
    rows: Vec<(usize, Row)>,
}

impl Out {
    /// A row focus can rest on.
    fn row(&mut self, row: Row, line: Line<'static>) {
        self.rows.push((self.lines.len(), row));
        self.lines.push(line);
    }
}

impl std::ops::Deref for Out {
    type Target = Vec<Line<'static>>;

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl std::ops::DerefMut for Out {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lines
    }
}

/// The sections, top to bottom, wrapped to `width`.
#[cfg(test)]
pub fn top(app: &App, width: usize) -> Vec<Line<'static>> {
    sections(app, width, usize::MAX).lines
}

fn sections(app: &App, width: usize, height: usize) -> Out {
    let mut out = Out::default();
    let bold = Style::default()
        .fg(app.theme.text)
        .add_modifier(Modifier::BOLD);
    let title = if app.session.title.is_empty() {
        "New session".to_owned()
    } else {
        crate::sidebar::clean(&app.session.title)
    };
    out.extend(wrap(Line::from(Span::styled(title, bold)), width));
    if app.sidebar.server_stale() {
        let warning = Style::default().fg(app.theme.warning);
        out.extend(wrap(
            Line::from(Span::styled(
                "⚠ older server build · cyber service restart".to_owned(),
                warning,
            )),
            width,
        ));
    }
    if let Some(parent) = &app.sidebar.parent_title {
        out.push(muted_line(
            app,
            &truncate_end(&format!("↳ {parent}"), width),
        ));
    }
    // The header carries it when the sidebar is not docked.
    if let Some(branch) = app.branches.header() {
        out.push(muted_line(app, &truncate_end(&branch, width)));
    }
    context(app, width, &mut out);
    reachable(app, width, height, &mut out);
    goal(app, width, &mut out);
    workflows(app, width, &mut out);
    super::team::sidebar(app, width, &mut out);
    mcp(app, width, &mut out);
    peers(app, width, &mut out);
    lsp(app, width, &mut out);
    machine(app, width, &mut out);
    files(app, width, &mut out);
    out
}

fn reachable(app: &App, width: usize, height: usize, out: &mut Out) {
    use crate::agents::{Source as AgentSource, Status};
    use crate::session_overview;
    let collapsed = app.sidebar.collapsed.contains(&Section::Sessions);
    let arrow = if collapsed { "▶" } else { "▼" };
    out.push(Line::default());
    out.row(
        Row::Header(Section::Sessions),
        header(
            app,
            Some(Row::Header(Section::Sessions)),
            format!("{arrow} Sessions & agents"),
        ),
    );
    if collapsed {
        return;
    }
    let full = session_overview::project(&app.directory, &app.session.id);
    let mut status = Out::default();
    reachable_source_status(app, &mut status);
    let fixed = full.counts.iter().filter(|count| **count > 0).count()
        + status.len()
        + usize::from(full.unavailable_peers > 0)
        + 1;
    let limit = height.saturating_sub(out.len() + fixed) / 2;
    let overview = session_overview::project_limit(&app.directory, &app.session.id, limit);
    reachable_groups(app, width, out, &overview);
    if overview.total == 0 {
        let state = app.directory.source(AgentSource::Messaging);
        let text = if app.session.id.is_empty() {
            "Attach a Session to discover"
        } else if state.loading && state.status == Status::Unread {
            "Loading reachable Sessions…"
        } else if state.error.is_some() {
            "Session listing unavailable"
        } else {
            match state.status {
                Status::Unread => "Reachable Sessions not loaded",
                Status::Disabled => "Messaging disabled here",
                Status::Unsupported => "Directory unsupported by server",
                _ => "No other reachable Sessions",
            }
        };
        out.push(muted_line(app, &truncate_end(text, width)));
    }
    reachable_source_status(app, out);
    if overview.unavailable_peers > 0 {
        out.push(muted_line(
            app,
            &format!("{} peer availability records", overview.unavailable_peers),
        ));
    }
    let footer = if overview.total > overview.entries.len() {
        format!(
            "+{} more · /agents",
            overview.total - overview.entries.len()
        )
    } else {
        "Open /agents".into()
    };
    out.row(
        Row::Directory,
        header(app, Some(Row::Directory), truncate_end(&footer, width)),
    );
}

fn reachable_source_status(app: &App, out: &mut Out) {
    use crate::agents::{Source, Status};
    let messaging = app.directory.source(Source::Messaging);
    if messaging.error.is_some() && !messaging.rows.is_empty() {
        out.push(muted_line(app, "Session listing stale"));
    }
    let hub = app.directory.source(Source::Hub);
    let text = if hub.error.is_some() {
        Some("Hub unavailable / access lost")
    } else if hub.loading && hub.status == Status::Unread {
        Some("Hub roster loading…")
    } else {
        match hub.status {
            Status::Inactive => Some("Hub off / project not bound"),
            Status::Disabled => Some("Hub disabled here"),
            Status::Unsupported => Some("Hub unsupported by server"),
            _ => None,
        }
    };
    if let Some(text) = text {
        out.push(muted_line(app, text));
    }
}

fn reachable_groups(
    app: &App,
    width: usize,
    out: &mut Out,
    overview: &crate::session_overview::Overview<'_>,
) {
    use crate::session_overview::{self, Group};
    for group in Group::ALL {
        if overview.counts[group as usize] == 0 {
            continue;
        }
        out.push(muted_line(
            app,
            &truncate_end(
                &format!("{} ({})", group.title(), overview.counts[group as usize]),
                width,
            ),
        ));
        for entry in overview
            .entries
            .iter()
            .filter(|entry| session_overview::group(entry) == group)
        {
            let row = Row::Reachable(entry.key.clone());
            out.row(
                row.clone(),
                header(
                    app,
                    Some(row),
                    truncate_end(session_overview::name(entry), width.saturating_sub(2)),
                ),
            );
            out.push(muted_line(
                app,
                &truncate_end(&format!("  {}", session_overview::detail(entry)), width),
            ));
        }
    }
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

/// Cells of the Context bar at most; narrower sidebars shrink it.
const CONTEXT_BAR: usize = 16;

/// `Context  ███▍      ┃    22%`, then `29.5K / 128K tokens`, the auto-compaction line and
/// the cost. Before the first reply only the window is known (`0 / 400K tokens · model`);
/// with an unknown window there is no bar and no percent.
fn context(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    out.push(Line::default());
    let indent = "Context ".len();
    let detail_width = width.saturating_sub(indent);
    match app.session.context {
        Some(meter) if meter.window.is_some() => {
            out.push(context_bar(app, &meter, width));
            for detail in context_details(app, &meter) {
                let (text, style) = detail;
                out.push(Line::from(vec![
                    Span::raw(" ".repeat(indent)),
                    Span::styled(truncate_end(&text, detail_width), style),
                ]));
            }
        }
        Some(meter) => {
            out.push(header(app, None, "Context".into()));
            let text = if meter.measured {
                format!("{} tokens · window unknown", grouped(meter.used_tokens))
            } else {
                "window unknown".to_owned()
            };
            out.push(muted_line(app, &truncate_end(&text, width)));
        }
        None => out.push(header(app, None, "Context".into())),
    }
    let cost = super::footer::cost(&app.session);
    let pad = if app.session.context.is_some_and(|m| m.window.is_some()) {
        " ".repeat(indent)
    } else {
        String::new()
    };
    out.push(muted_line(app, &format!("{pad}{cost} spent")));
}

/// The bar color: `success`, `warning` from 70%, `error` from the auto-compaction point
/// (90% when auto-compaction is off).
pub fn context_color(t: &Tokens, percent: f64, compact: Option<f64>) -> Color {
    if percent >= compact.unwrap_or(MACHINE_HIGH) {
        t.error
    } else if percent >= MACHINE_WARN {
        t.warning
    } else {
        t.success
    }
}

fn context_bar(app: &App, meter: &crate::model::ContextMeter, width: usize) -> Line<'static> {
    let t = &app.theme;
    let percent = meter.percent().unwrap_or(0.0);
    let compact = meter.compact_percent();
    let bar = CONTEXT_BAR.min(width.saturating_sub(13)).max(4);
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled("Context ".to_owned(), bold)];
    let marker = compact.map(|c| (((c / 100.0) * bar as f64).floor() as usize).min(bar - 1));
    if t.monochrome {
        let mut text: Vec<char> = crate::sidebar::bar_text(percent, bar.saturating_sub(2))
            .chars()
            .collect();
        if let Some(m) = marker.map(|m| (m + 1).min(text.len().saturating_sub(2)).max(1)) {
            text[m] = '|';
        }
        spans.push(Span::raw(text.into_iter().collect::<String>()));
    } else {
        let fill = context_color(t, percent, compact);
        let cells: Vec<char> = crate::sidebar::bar_cells(percent, bar).chars().collect();
        let filled = cells.iter().take_while(|c| **c == '█').count();
        for (i, cell) in cells.into_iter().enumerate() {
            if marker == Some(i) {
                let bg = if i < filled { fill } else { t.border };
                spans.push(Span::styled("│", Style::default().fg(t.text).bg(bg)));
            } else {
                spans.push(Span::styled(
                    cell.to_string(),
                    Style::default().fg(fill).bg(t.border),
                ));
            }
        }
    }
    spans.push(Span::styled(
        format!(" {percent:>3.0}%"),
        Style::default().fg(t.text),
    ));
    Line::from(spans)
}

/// The lines under the bar: tokens against the window, then the auto-compaction state.
fn context_details(app: &App, meter: &crate::model::ContextMeter) -> Vec<(String, Style)> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let window = meter.window.map_or_else(String::new, window_label);
    let tokens = if meter.compacted {
        format!("compacted · measured after the next reply · {window} window")
    } else if meter.measured {
        format!(
            "{} / {window} tokens",
            super::footer::compact(meter.used_tokens)
        )
    } else {
        format!(
            "0 / {window} tokens · {}",
            app.catalog.model_name(&app.session.model)
        )
    };
    let mut out = vec![(tokens, muted)];
    let compact = meter.compact_percent();
    let compacting = app.compacting();
    let state = match compact {
        _ if compacting => ("compacting…".to_owned(), Style::default().fg(t.warning)),
        Some(c) if meter.measured && meter.percent().is_some_and(|p| p >= c) => (
            "auto-compact pending".to_owned(),
            Style::default().fg(t.error),
        ),
        Some(c) => (format!("│ auto-compact at {c:.0}%"), muted),
        None => ("auto-compact off".to_owned(), muted),
    };
    out.push(state);
    out
}

/// `128K`, `400K`, `1M`: the window without a trailing `.0`.
fn window_label(n: u64) -> String {
    super::footer::compact(n)
        .replace(".0K", "K")
        .replace(".0M", "M")
}

/// Cells of a Workflows row's bar.
const WORKFLOW_BAR: usize = 6;

/// `audit  ▀▀····  31/100` per active run (`#2 in queue` for a queued one), the current
/// Session's first, then `+N more`; omitted when no run is active.
/// The current Goal: its condition, status with turns against the budget and cost, the
/// last verdict and the queued count (`tui` → Goal and loop indicators).
fn goal(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let lines = app.goals.sidebar();
    let Some((condition, rest)) = lines.split_first() else {
        return;
    };
    out.push(Line::default());
    out.push(header(app, None, "Goal".into()));
    out.extend(wrap(
        Line::from(Span::styled(
            condition.clone(),
            Style::default().fg(app.theme.text),
        )),
        width,
    ));
    for line in rest {
        out.push(muted_line(app, &truncate_end(line, width)));
    }
}

fn workflows(app: &App, width: usize, out: &mut Out) {
    let rows = &app.sidebar.workflows;
    if rows.is_empty() {
        return;
    }
    let t = &app.theme;
    out.push(Line::default());
    out.push(header(app, None, "Workflows".into()));
    for (i, run) in rows.iter().enumerate() {
        let tail = workflow_tail(app, run);
        let tail_width: usize = tail.iter().map(Span::width).sum();
        let name_room = width.saturating_sub(tail_width + 1);
        let mut spans = vec![Span::styled(
            format!("{:<name_room$} ", truncate_end(&run.name, name_room)),
            Style::default().fg(t.text),
        )];
        spans.extend(tail);
        out.row(
            Row::Workflow(i),
            focusable(app, Some(Row::Workflow(i)), spans),
        );
    }
    if app.sidebar.workflows_more > 0 {
        out.push(muted_line(
            app,
            &format!("+{} more", app.sidebar.workflows_more),
        ));
    }
}

/// A Workflows row after the name: the bar and `done/total`, or the queue position.
fn workflow_tail(app: &App, run: &crate::sidebar::WorkflowRow) -> Vec<Span<'static>> {
    let t = &app.theme;
    if let Some(n) = run.queue_position {
        return vec![Span::styled(
            format!("#{n} in queue"),
            Style::default().fg(t.info),
        )];
    }
    let count = format!(" {}/{}", run.done.min(run.total), run.total);
    let mut spans = super::progress::bar(app, run.done, run.total, WORKFLOW_BAR, 0);
    spans.push(Span::styled(count, Style::default().fg(t.text_muted)));
    spans
}

/// The dot color of an MCP status word.
fn mcp_color(t: &Tokens, server: &McpServer) -> Color {
    match server.status.as_str() {
        "connected" => t.success,
        "failed" if server.blocked() => t.text_subtle,
        "failed" => t.error,
        "disabled" | "idle" => t.text_subtle,
        _ => t.warning,
    }
}

fn mcp(app: &App, width: usize, out: &mut Out) {
    let t = &app.theme;
    if app.sidebar.mcp.ready().is_none() {
        return;
    }
    let servers = app.sidebar.servers();
    let collapsed = app.sidebar.collapsed.contains(&Section::Mcp);
    out.push(Line::default());
    let arrow = if collapsed { "▶" } else { "▼" };
    let row = Row::Header(Section::Mcp);
    out.row(row.clone(), header(app, Some(row), format!("{arrow} MCP")));
    if collapsed {
        return;
    }
    if servers.is_empty() {
        out.push(muted_line(app, "No MCP servers"));
        out.extend(wrap(
            Line::from(Span::styled(
                "add one: cyber mcp add",
                Style::default().fg(t.text_subtle),
            )),
            width,
        ));
        return;
    }
    for (i, server) in servers.iter().take(MCP_ROWS).enumerate() {
        out.row(
            Row::Mcp(i),
            focusable(app, Some(Row::Mcp(i)), mcp_row(t, server, width)),
        );
        if let Some(note) = mcp_note(server) {
            let color = if server.status == "failed" && !server.blocked() {
                t.error
            } else {
                t.text_muted
            };
            out.push(Line::from(Span::styled(
                format!("  {}", truncate_end(&note, width.saturating_sub(2))),
                Style::default().fg(color),
            )));
        }
    }
    if servers.len() > MCP_ROWS {
        out.push(muted_line(
            app,
            &format!("+{} more · /status", servers.len() - MCP_ROWS),
        ));
    }
}

/// `● name remote  Connected · 2 tools`, with the name truncated to fit.
fn mcp_row(t: &Tokens, server: &McpServer, width: usize) -> Vec<Span<'static>> {
    let word = server.word();
    let tools = (server.status == "connected").then(|| match server.tools {
        1 => " · 1 tool".to_owned(),
        n => format!(" · {n} tools"),
    });
    let transport = server
        .transport
        .as_deref()
        .map(|t| format!(" {t}"))
        .unwrap_or_default();
    let tail = word.len() + 1 + tools.as_ref().map_or(0, String::len);
    let name_room = width
        .saturating_sub(2 + tail + transport.chars().count())
        .max(4);
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(mcp_color(t, server))),
        Span::styled(
            truncate_end(&server.name, name_room),
            Style::default().fg(t.text),
        ),
    ];
    if 2 + server.name.chars().count().min(name_room) + transport.len() + tail <= width {
        spans.push(Span::styled(transport, Style::default().fg(t.text_subtle)));
    }
    spans.push(Span::styled(
        format!(" {word}"),
        Style::default().fg(t.text_muted),
    ));
    if let Some(tools) = tools {
        spans.push(Span::styled(tools, Style::default().fg(t.text_subtle)));
    }
    spans
}

/// The second line under a server that needs attention.
fn mcp_note(server: &McpServer) -> Option<String> {
    match server.status.as_str() {
        "needs_auth" | "needs_client_registration" => {
            Some(format!("cyber mcp auth {}", server.name))
        }
        "failed" => server.error.clone(),
        _ => None,
    }
}

/// `Peers  desk ●  laptop ○`: the remotes and whether each answered, only when there are
/// some; the MCP section above is unchanged.
fn peers(app: &App, width: usize, out: &mut Vec<Line<'static>>) {
    let t = &app.theme;
    let Some(remotes) = app.sidebar.peers.ready().filter(|r| !r.is_empty()) else {
        return;
    };
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled("Peers", bold)];
    for remote in remotes {
        let (dot, color) = if remote.online {
            ("●", t.success)
        } else {
            ("○", t.text_subtle)
        };
        spans.push(Span::styled(
            format!("  {}", remote.name),
            Style::default().fg(t.text),
        ));
        spans.push(Span::styled(format!(" {dot}"), Style::default().fg(color)));
    }
    out.push(Line::default());
    out.extend(wrap(Line::from(spans), width));
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

/// Cells of a Machine bar at most; narrower sidebars shrink it.
const MACHINE_BAR: usize = 14;
/// Below this percent a Machine bar is `success`, then `warning` up to [`MACHINE_HIGH`].
const MACHINE_WARN: f64 = 70.0;
/// From this percent a Machine bar is `error`.
const MACHINE_HIGH: f64 = 90.0;

/// The Machine bar's color for `percent` in use.
pub fn machine_color(t: &Tokens, percent: f64) -> Color {
    if percent >= MACHINE_HIGH {
        t.error
    } else if percent >= MACHINE_WARN {
        t.warning
    } else {
        t.success
    }
}

/// `▼ Machine  macOS 15.0`, then CPU, Mem and Disk bars with their details and the load
/// line; `—` when `GET /system` failed before any sample.
fn machine(app: &App, width: usize, out: &mut Out) {
    if !app.machine_stats() || !app.sidebar.machine_shown() {
        return;
    }
    let t = &app.theme;
    let collapsed = app.sidebar.collapsed.contains(&Section::Machine);
    let arrow = if collapsed { "▶" } else { "▼" };
    let title = format!("{arrow} Machine");
    let label = app
        .sidebar
        .machine
        .ready()
        .and_then(|m| m.label.clone())
        .unwrap_or_default();
    out.push(Line::default());
    let room = width.saturating_sub(title.chars().count() + 1);
    let mut line = header(app, Some(Row::Header(Section::Machine)), title);
    if !label.is_empty() && room > 3 {
        let label = truncate_end(&label, room);
        let pad = room - label.chars().count();
        line.spans.push(Span::styled(
            format!(" {}{label}", " ".repeat(pad)),
            Style::default().fg(t.text_muted),
        ));
    }
    out.row(Row::Header(Section::Machine), line);
    if collapsed {
        return;
    }
    let Some(m) = app.sidebar.machine.ready() else {
        if app.sidebar.server_stale() {
            let warning = Style::default().fg(app.theme.warning);
            out.extend(wrap(
                Line::from(Span::styled(
                    crate::perform::NEWER_SERVER.to_owned(),
                    warning,
                )),
                width,
            ));
        } else {
            out.push(muted_line(app, "—"));
        }
        return;
    };
    let bar = MACHINE_BAR.min(width.saturating_sub(11)).max(4);
    let cpu_detail = match m.cpu_idle {
        Some(idle) => format!("{} cores · {idle:.0}% idle", m.cores),
        None => format!("{} cores · measuring", m.cores),
    };
    machine_row(app, "CPU", m.cpu_busy(), bar, &cpu_detail, width, out);
    let memory = format!(
        "{} / {} GB · {} free",
        crate::sidebar::gigabytes(m.memory_total.saturating_sub(m.memory_available)),
        crate::sidebar::gigabytes(m.memory_total),
        crate::sidebar::gigabytes(m.memory_available)
    );
    machine_row(app, "Mem", m.memory_used(), bar, &memory, width, out);
    if let Some((total, available)) = m.disk {
        let disk = format!(
            "{} / {} GB · {} free",
            crate::sidebar::gigabytes(total.saturating_sub(available)),
            crate::sidebar::gigabytes(total),
            crate::sidebar::gigabytes(available)
        );
        machine_row(app, "Disk", m.disk_used(), bar, &disk, width, out);
    }
    if let Some([one, five, fifteen]) = m.load {
        out.push(muted_line(
            app,
            &truncate_end(&format!("load {one:.1} {five:.1} {fifteen:.1}"), width),
        ));
    }
}

/// `CPU  ███▍          18%` and, indented under the bar, the detail line.
fn machine_row(
    app: &App,
    name: &str,
    percent: Option<f64>,
    bar: usize,
    detail: &str,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        format!("{name:<5}"),
        Style::default().fg(t.text_muted),
    )];
    match percent {
        Some(p) if t.monochrome => {
            spans.push(Span::raw(crate::sidebar::bar_text(
                p,
                bar.saturating_sub(2),
            )));
        }
        Some(p) => spans.push(Span::styled(
            crate::sidebar::bar_cells(p, bar),
            Style::default().fg(machine_color(t, p)).bg(t.border),
        )),
        None => spans.push(Span::styled(" ".repeat(bar), Style::default().bg(t.border))),
    }
    let shown = percent.map_or_else(|| "  —".to_owned(), |p| format!("{:>3.0}%", p));
    spans.push(Span::styled(
        format!(" {shown}"),
        Style::default().fg(t.text),
    ));
    out.push(Line::from(spans));
    out.push(muted_line(
        app,
        &format!("     {}", truncate_end(detail, width.saturating_sub(5))),
    ));
}

fn files(app: &App, width: usize, out: &mut Out) {
    let t = &app.theme;
    let changes = app.sidebar.changes();
    if changes.is_empty() {
        return;
    }
    let collapsed = app.sidebar.collapsed.contains(&Section::Files);
    out.push(Line::default());
    let arrow = if collapsed { "▶" } else { "▼" };
    let row = Row::Header(Section::Files);
    out.row(
        row.clone(),
        header(app, Some(row), format!("{arrow} Modified files")),
    );
    if collapsed {
        return;
    }
    for (i, change) in changes.iter().take(FILE_ROWS).enumerate() {
        let added = format!("+{}", change.additions);
        let removed = format!("-{}", change.deletions);
        let room = width.saturating_sub(added.len() + removed.len() + 2);
        let spans = vec![
            Span::styled(
                format!("{:<room$}", shorten_path(&change.file, room)),
                Style::default().fg(t.text),
            ),
            Span::styled(format!(" {added}"), Style::default().fg(t.diff_added)),
            Span::styled(format!(" {removed}"), Style::default().fg(t.diff_removed)),
        ];
        out.row(Row::File(i), focusable(app, Some(Row::File(i)), spans));
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
