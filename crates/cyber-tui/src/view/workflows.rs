//! The Workflow monitor (`tui` → Workflow monitor): the run list, a run's agent tree
//! by phase, and a run's log, full screen over the transcript. Each row has a
//! known-progress nyan bar; only the selected running run's bar animates. At 80 columns
//! the phase and elapsed columns are dropped, below 64 each run takes two rows, and
//! screen readers get each row as one sentence.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::layout::truncate_end;
use crate::app::App;
use crate::workflows::{Agent, Confirm, Group, LogView, RunDetail, RunView, Summary};

/// Inner widths from which the list shows every column, or one row per run.
const WIDE_FROM: usize = 98;
const ONE_ROW_FROM: usize = 62;
const NAME: usize = 12;
/// The most the wide layout widens the name column by; the rest stays blank.
const NAME_EXTRA: usize = 12;
const STATUS: usize = 19;
const PHASE: usize = 10;
const AGENTS: usize = 14;
const TOKENS: usize = 7;
const COST: usize = 15;
const ELAPSED: usize = 7;
/// The medium layout's bar: no room for the cat.
const MEDIUM_BAR: usize = 11;
/// The wide layout's bar takes the rest of the row, within these bounds.
const BAR_MIN: usize = 20;
const BAR_MAX: usize = 40;
/// The wide layout's columns other than the name's extra width and the bar.
const WIDE_FIXED: usize =
    3 + NAME + 1 + STATUS + 1 + PHASE + 1 + 2 + AGENTS + 1 + COST + 1 + ELAPSED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Wide,
    Medium,
    Narrow,
}

fn width_of(inner: usize) -> Width {
    if inner >= WIDE_FROM {
        Width::Wide
    } else if inner >= ONE_ROW_FROM {
        Width::Medium
    } else {
        Width::Narrow
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    app.progress.set_on_screen(false);
    if area.width < 20 || area.height < 6 {
        return;
    }
    let w = &app.workflows;
    if let Some(log) = &w.log {
        draw_log(f, app, log, area);
    } else if let Some(view) = &w.run {
        draw_run(f, app, view, area);
    } else {
        draw_list(f, app, area);
    }
}

/// The rounded frame with a title on each side; the right one is left out when both
/// do not fit, and the left one is cut to the width.
fn frame(app: &App, width: u16, left: String, right: String) -> Block<'static> {
    let t = &app.theme;
    let room = (width as usize).saturating_sub(4);
    let left = truncate_end(&left, room.saturating_sub(2));
    let fits = left.chars().count() + right.chars().count() + 6 <= room;
    let right = if fits && !right.is_empty() {
        format!(" {right} ")
    } else {
        String::new()
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            format!(" {left} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(right, Style::default().fg(t.text_muted))).right_aligned(),
        )
}

/// The rows of `area` inside its frame: the body and the key line.
fn split(block: &Block, area: Rect) -> (Rect, Rect) {
    let inner = block.inner(area);
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let keys = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    (body, keys)
}

fn muted(app: &App, text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(app.theme.text_muted),
    ))
}

/// `text` cut or padded to exactly `width` columns.
fn pad(text: &str, width: usize) -> String {
    let cut = truncate_end(text, width);
    let used = unicode_width::UnicodeWidthStr::width(cut.as_str());
    format!("{cut}{}", " ".repeat(width.saturating_sub(used)))
}

/// `1.2M`, `18K`, `9.8K`, `950`.
pub fn tokens_text(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..10_000 => format!("{:.1}K", tokens as f64 / 1e3),
        10_000..1_000_000 => format!("{}K", tokens / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1e6),
    }
}

/// `12m04s`, `1h02m`.
pub fn elapsed_text(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (hours, minutes) = (seconds / 3_600, seconds / 60 % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m{:02}s", seconds % 60)
    }
}

/// An agent's duration: `41s`, then as [`elapsed_text`].
fn duration_text(ms: Option<u64>) -> String {
    match ms {
        None => "-".into(),
        Some(ms) if ms < 60_000 => format!("{}s", ms / 1_000),
        Some(ms) => elapsed_text(ms),
    }
}

/// A run's active time as listed, counting on while it runs.
fn elapsed(run: &Summary, since: Duration) -> u64 {
    let extra = if run.active() { since } else { Duration::ZERO };
    run.elapsed_ms + extra.as_millis() as u64
}

fn cost_text(run: &Summary, budget: bool) -> String {
    match run.budget_usd.filter(|_| budget) {
        Some(max) => format!("${:.2} / ${max:.2}", run.cost_usd),
        None => format!("${:.2}", run.cost_usd),
    }
}

/// Spend over 80% of the budget is a warning.
fn cost_style(app: &App, run: &Summary) -> Style {
    let t = &app.theme;
    match run.budget_usd {
        Some(max) if max > 0.0 && run.cost_usd > max * 0.8 => Style::default().fg(t.warning),
        _ => Style::default().fg(t.text),
    }
}

fn status_color(app: &App, status: &str) -> Color {
    let t = &app.theme;
    match status {
        "running" | "waiting_input" => t.success,
        "queued" => t.info,
        "paused" => t.warning,
        "failed" | "budget_exceeded" => t.error,
        _ => t.text_muted,
    }
}

fn counts_text(run: &Summary) -> String {
    let a = &run.agents;
    format!("{}/{}/{}/{}", a.completed, a.running, a.queued, a.failed)
}

/// `run_01JA2…K3`, `agt_…33`.
fn short_id(id: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= head + tail + 1 {
        return id.to_owned();
    }
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

/// `3 dollars 41`, `41 cents`.
fn money_words(usd: f64) -> String {
    let cents = (usd * 100.0).round() as u64;
    match (cents / 100, cents % 100) {
        (0, c) => format!("{c} cents"),
        (d, 0) => format!("{d} dollars"),
        (d, c) => format!("{d} dollars {c}"),
    }
}

/// `12 minutes`, `45 seconds`, `1 hour 2 minutes`.
fn time_words(ms: u64) -> String {
    let unit = |n: u64, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    let seconds = ms / 1_000;
    match (seconds / 3_600, seconds / 60 % 60) {
        (0, 0) => unit(seconds, "second"),
        (0, m) => unit(m, "minute"),
        (h, 0) => unit(h, "hour"),
        (h, m) => format!("{} {}", unit(h, "hour"), unit(m, "minute")),
    }
}

/// A run as one sentence with every value in words.
pub fn spoken(run: &Summary, since: Duration) -> String {
    let mut parts = vec![run.name.clone()];
    if let Some(n) = run.queue_position {
        parts.push(format!("queued, number {n} in queue"));
        return parts.join(", ");
    }
    parts.push(run.status.replace('_', " "));
    if run.settling() > 0 {
        parts.push(format!("{} agents settling", run.settling()));
    }
    if let Some(phase) = &run.phase {
        parts.push(format!("phase {phase}"));
    }
    let a = &run.agents;
    parts.push(format!("{} of {} agents done", a.completed, a.total));
    parts.push(format!("{} running", a.running));
    parts.push(format!("{} queued", a.queued));
    if a.failed > 0 {
        parts.push(format!("{} failed", a.failed));
    }
    parts.push(format!("{} tokens", run.tokens));
    let budget = run
        .budget_usd
        .map_or(String::new(), |max| format!(" of {max}"));
    parts.push(format!("{}{budget}", money_words(run.cost_usd)));
    parts.push(time_words(elapsed(run, since)));
    parts.join(", ")
}

/// The animation frame of a run's bar: only the selected running run moves.
fn frame_of(app: &App, run: &Summary, selected: bool) -> u64 {
    if selected && run.active() {
        app.progress.set_on_screen(true);
        app.progress.frame
    } else {
        0
    }
}

fn run_bar(app: &App, run: &Summary, width: usize, frame: u64) -> Vec<Span<'static>> {
    if let Some(n) = run.queue_position {
        return vec![Span::styled(
            pad(&format!("#{n} in queue"), width),
            Style::default().fg(app.theme.info),
        )];
    }
    super::progress::bar(app, run.agents.completed, run.agents.total, width, frame)
}

/// `3 running · 1 queued` of the listed runs.
fn tally(runs: &[Summary]) -> String {
    let count = |f: &dyn Fn(&Summary) -> bool| runs.iter().filter(|r| f(r)).count();
    let running = count(&|r| r.active());
    let queued = count(&|r| r.status == "queued");
    let paused = count(&|r| r.status == "paused");
    let parts: Vec<String> = [(running, "running"), (queued, "queued"), (paused, "paused")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
    if parts.is_empty() {
        "no active runs".into()
    } else {
        parts.join(" · ")
    }
}

fn location(app: &App) -> String {
    if app.workflows.all {
        "all Locations".into()
    } else {
        crate::sidebar::home_path(&app.workflows.directory, app.home.as_deref())
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let w = &app.workflows;
    let block = frame(
        app,
        area.width,
        format!("Workflows · {}", location(app)),
        tally(w.summaries()),
    );
    let (body, keys) = split(&block, area);
    f.render_widget(block, area);
    let inner = body.width as usize;
    let lines = match &w.runs.data {
        _ if w.runs.data.is_none() && w.runs.error.is_some() => {
            vec![muted(
                app,
                format!(" {}", w.runs.error.clone().unwrap_or_default()),
            )]
        }
        None => vec![muted(app, " Loading…")],
        Some(runs) if runs.is_empty() => vec![muted(app, empty(app))],
        Some(_) => list_lines(app, inner, body.height as usize),
    };
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(key_line(app, inner, list_keys(app))), keys);
}

fn empty(app: &App) -> &'static str {
    if app.workflows.all {
        " No Workflow Runs."
    } else {
        " No Workflow Runs in this Location."
    }
}

/// The header and every run's rows, scrolled so the selected run shows.
fn list_lines(app: &App, inner: usize, height: usize) -> Vec<Line<'static>> {
    let w = &app.workflows;
    let since = w.since(app.now());
    let layout = width_of(inner);
    let mut lines = Vec::new();
    if !app.screen_reader {
        lines.push(header(app, layout, inner));
    }
    let mut selected_end = 0;
    for (i, run) in w.summaries().iter().enumerate() {
        let selected = i == w.selected;
        lines.extend(run_lines(app, run, selected, layout, inner, since));
        if let Some(refusal) = w.refusals.get(&run.id) {
            lines.push(refusal_line(app, refusal, inner));
        }
        if selected {
            selected_end = lines.len();
        }
    }
    let skip = selected_end.saturating_sub(height);
    let mut shown: Vec<Line> = lines.into_iter().skip(skip).collect();
    if skip > 0 && !app.screen_reader {
        shown[0] = header(app, layout, inner);
    }
    shown
}

fn refusal_line(app: &App, refusal: &str, inner: usize) -> Line<'static> {
    Line::from(Span::styled(
        truncate_end(&format!("     ⚠ {refusal}"), inner),
        Style::default().fg(app.theme.warning),
    ))
}

/// Whether the wide layout has room for the tokens column beside a bar of `BAR_MIN`.
fn tokens_shown(layout: Width, inner: usize) -> bool {
    layout == Width::Wide && inner >= WIDE_FIXED + TOKENS + 1 + BAR_MIN
}

fn header(app: &App, layout: Width, inner: usize) -> Line<'static> {
    let (name, bar) = columns(layout, inner);
    let text = match layout {
        Width::Wide => {
            let tokens = if tokens_shown(layout, inner) {
                format!("{} ", pad("TOKENS", TOKENS))
            } else {
                String::new()
            };
            format!(
                "   {} {} {} {} {} {tokens}{} ELAPSED",
                pad("NAME", name),
                pad("STATUS", STATUS),
                pad("PHASE", PHASE),
                pad("PROGRESS", bar + 1),
                pad("AGENTS d/r/q/f", AGENTS),
                pad("COST", COST),
            )
        }
        Width::Medium => format!(
            "   {} {} {} {} COST",
            pad("NAME", name),
            pad("STATUS", STATUS),
            pad("PROGRESS", bar + 1),
            pad("d/r/q/f", AGENTS),
        ),
        Width::Narrow => format!("   {} {} d/r/q/f", pad("NAME", name), pad("STATUS", STATUS)),
    };
    Line::from(Span::styled(
        truncate_end(&text, inner),
        Style::default().fg(app.theme.text_subtle),
    ))
}

/// The name and bar widths of a layout.
fn columns(layout: Width, inner: usize) -> (usize, usize) {
    match layout {
        Width::Wide => {
            let tokens = if tokens_shown(layout, inner) {
                TOKENS + 1
            } else {
                0
            };
            let free = inner.saturating_sub(WIDE_FIXED + tokens);
            let bar = free.clamp(BAR_MIN, BAR_MAX);
            (NAME + free.saturating_sub(bar).min(NAME_EXTRA), bar)
        }
        Width::Medium => {
            let fixed = 3 + 1 + STATUS + 1 + MEDIUM_BAR + 2 + AGENTS + 1 + COST;
            (inner.saturating_sub(fixed).max(6), MEDIUM_BAR)
        }
        Width::Narrow => {
            let name = inner.saturating_sub(3 + 1 + STATUS + 1 + 10).max(6);
            (name, inner.saturating_sub(3 + 1 + 8))
        }
    }
}

fn run_lines(
    app: &App,
    run: &Summary,
    selected: bool,
    layout: Width,
    inner: usize,
    since: Duration,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    if app.screen_reader {
        let style = if selected {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        return vec![Line::from(Span::styled(
            truncate_end(&format!(" {}", spoken(run, since)), inner),
            style,
        ))];
    }
    let (name_width, bar_width) = columns(layout, inner);
    let frame = frame_of(app, run, selected);
    let mut head = vec![
        Span::styled(
            if selected { " › " } else { "   " },
            Style::default().fg(t.accent),
        ),
        Span::styled(
            pad(&run.name, name_width),
            if selected {
                t.selected()
            } else {
                Style::default().fg(t.text).add_modifier(Modifier::BOLD)
            },
        ),
        Span::raw(" "),
        Span::styled(
            pad(&run.status_text(), STATUS),
            Style::default().fg(status_color(app, &run.status)),
        ),
        Span::raw(" "),
    ];
    let queued = run.queue_position.is_some();
    let dash = |s: String| if queued { "-".to_owned() } else { s };
    let agents = Span::styled(
        pad(&dash(counts_text(run)), AGENTS),
        Style::default().fg(t.text),
    );
    match layout {
        Width::Wide => {
            head.push(Span::styled(
                pad(run.phase.as_deref().unwrap_or("-"), PHASE),
                Style::default().fg(t.text_muted),
            ));
            head.push(Span::raw(" "));
            head.extend(run_bar(app, run, bar_width, frame));
            head.extend([Span::raw("  "), agents, Span::raw(" ")]);
            if tokens_shown(layout, inner) {
                head.extend([
                    Span::styled(
                        pad(&dash(tokens_text(run.tokens)), TOKENS),
                        Style::default().fg(t.text_muted),
                    ),
                    Span::raw(" "),
                ]);
            }
            head.extend([
                Span::styled(pad(&dash(cost_text(run, true)), COST), cost_style(app, run)),
                Span::raw(" "),
                Span::styled(
                    dash(elapsed_text(elapsed(run, since))),
                    Style::default().fg(t.text_muted),
                ),
            ]);
            vec![Line::from(head)]
        }
        Width::Medium => {
            head.extend(run_bar(app, run, bar_width, frame));
            head.extend([
                Span::raw("  "),
                agents,
                Span::raw(" "),
                Span::styled(dash(cost_text(run, inner >= 72)), cost_style(app, run)),
            ]);
            vec![Line::from(head)]
        }
        Width::Narrow => {
            head.push(agents);
            let mut second = vec![Span::raw("   ")];
            second.extend(run_bar(app, run, bar_width, frame));
            second.push(Span::raw(" "));
            second.push(Span::styled(
                dash(cost_text(run, false)),
                cost_style(app, run),
            ));
            vec![Line::from(head), Line::from(second)]
        }
    }
}

/// The keys of a view, or the pending confirmation instead.
fn key_line(app: &App, inner: usize, keys: Vec<String>) -> Line<'static> {
    let t = &app.theme;
    if let Some(confirm) = &app.workflows.confirm {
        let text = match confirm {
            Confirm::Stop { name, .. } => format!(" Stop {name}? y stop · any other key cancels"),
            Confirm::Revision { revision, .. } => {
                format!(
                    " Script changed — start revision {revision}? y yes · any other key cancels"
                )
            }
        };
        return Line::from(Span::styled(
            truncate_end(&text, inner),
            Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
        ));
    }
    muted(app, truncate_end(&format!(" {}", keys.join(" · ")), inner))
}

fn hints(app: &App, actions: &[&str]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|action| app.keymap.hint("workflows", action))
        .map(|(key, what)| format!("{key} {what}"))
        .collect()
}

fn list_keys(app: &App) -> Vec<String> {
    let mut keys = hints(
        app,
        &[
            "workflows_open",
            "workflows_pause",
            "workflows_resume",
            "workflows_stop",
            "workflows_resume_interrupted",
            "workflows_script",
            "workflows_log",
            "workflows_scope",
        ],
    );
    keys.push("esc".into());
    keys
}

fn run_keys(app: &App) -> Vec<String> {
    let open = app
        .keymap
        .label("workflows", "workflows_open")
        .map(|key| format!("{key}/t transcript"))
        .unwrap_or_else(|| "t transcript".into());
    let mut keys = vec![open];
    keys.extend(hints(
        app,
        &[
            "workflows_pause",
            "workflows_stop",
            "workflows_log",
            "workflows_script",
        ],
    ));
    keys.push("esc back".into());
    keys
}

fn draw_run(f: &mut Frame, app: &App, view: &RunView, area: Rect) {
    let Some(detail) = &view.detail.data else {
        let block = frame(app, area.width, short_id(&view.run_id, 9, 2), String::new());
        let (body, _) = split(&block, area);
        f.render_widget(block, area);
        let text = view
            .detail
            .error
            .clone()
            .map_or_else(|| " Loading…".to_owned(), |e| format!(" {e}"));
        f.render_widget(Paragraph::new(muted(app, text)), body);
        return;
    };
    let run = &detail.summary;
    let since = app.workflows.since(app.now());
    let left = format!(
        "{} · {} · {} · revision {}",
        run.name,
        short_id(&run.id, 9, 2),
        run.status_text(),
        run.revision
    );
    let right = format!(
        "{} · {}",
        cost_text(run, true),
        elapsed_text(elapsed(run, since))
    );
    let block = frame(app, area.width, left, right);
    let (body, keys) = split(&block, area);
    f.render_widget(block, area);
    let inner = body.width as usize;
    let mut lines = vec![run_progress(app, run, inner)];
    lines.extend(tree_lines(
        app,
        detail,
        view.selected,
        inner,
        body.height as usize - 1,
    ));
    if let Some(refusal) = app.workflows.refusals.get(&run.id) {
        lines.push(refusal_line(app, refusal, inner));
    }
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(key_line(app, inner, run_keys(app))), keys);
}

/// The run's bar over the width and `31/100 agents · 31%`.
fn run_progress(app: &App, run: &Summary, inner: usize) -> Line<'static> {
    let a = &run.agents;
    let percent = (a.completed * 100).checked_div(a.total).unwrap_or(0);
    let label = format!("{}/{} agents · {percent}%", a.completed, a.total);
    if app.screen_reader {
        return muted(app, format!(" {}", spoken(run, Duration::ZERO)));
    }
    let width = inner.saturating_sub(label.chars().count() + 3);
    let mut spans = vec![Span::raw(" ")];
    spans.extend(run_bar(app, run, width, frame_of(app, run, true)));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(label, Style::default().fg(app.theme.success)));
    Line::from(spans)
}

/// Phases with their agents, scrolled so the selected agent shows.
fn tree_lines(
    app: &App,
    detail: &RunDetail,
    selected: usize,
    inner: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut row = 0;
    let mut selected_end = 0;
    for group in detail.groups() {
        lines.push(group_line(app, &group, inner));
        for &index in &group.agents {
            let agent = &detail.agents[index];
            lines.push(agent_line(app, agent, row == selected, inner));
            if row == selected {
                selected_end = lines.len();
            }
            row += 1;
        }
        if group.queued > 0 {
            lines.push(muted(app, format!("     ◌ {} queued", group.queued)));
        }
    }
    let skip = selected_end.saturating_sub(height);
    lines.into_iter().skip(skip).collect()
}

/// `▼ Verify      11/80  · 16 running · 53 queued`.
fn group_line(app: &App, group: &Group, inner: usize) -> Line<'static> {
    let t = &app.theme;
    let marker = if group.total > 0 { "▼" } else { "▶" };
    let title = group.title.as_deref().unwrap_or("(no phase)");
    let mut extra = Vec::new();
    if group.running > 0 {
        extra.push(format!("{} running", group.running));
    }
    if group.queued > 0 {
        extra.push(format!("{} queued", group.queued));
    }
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("  · {}", extra.join(" · "))
    };
    let text = format!(
        " {marker} {} {}/{}{extra}",
        pad(title, 12),
        group.done,
        group.total
    );
    Line::from(Span::styled(
        truncate_end(&text, inner),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    ))
}

fn agent_glyph(app: &App, status: &str) -> Span<'static> {
    let t = &app.theme;
    let (glyph, color) = match status {
        "completed" => ("✓", t.success),
        "running" => ("●", t.accent),
        "failed" => ("✗", t.error),
        "interrupted" => ("⏸", t.warning),
        "cancelled" => ("■", t.text_muted),
        _ => ("◌", t.text_muted),
    };
    Span::styled(glyph, Style::default().fg(color))
}

fn agent_spoken(agent: &Agent) -> String {
    let mut parts = vec![short_id(&agent.id, 4, 2), agent.status.clone()];
    parts.extend(agent.label.clone());
    parts.extend(agent.tier.as_ref().map(|tier| format!("tier {tier}")));
    parts.push(format!("{} tokens", agent.tokens));
    parts.push(money_words(agent.cost));
    parts.extend(agent.duration_ms.map(time_words));
    parts.join(", ")
}

/// `  › ● agt_…33  verify finding 7  verify 31.0K  $0.18   1m02s  cyber/run-…/33`.
fn agent_line(app: &App, agent: &Agent, selected: bool, inner: usize) -> Line<'static> {
    let t = &app.theme;
    let marker = if selected { "   › " } else { "     " };
    if app.screen_reader {
        let style = if selected {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        return Line::from(Span::styled(
            truncate_end(&format!("{marker}{}", agent_spoken(agent)), inner),
            style,
        ));
    }
    let id_style = if selected {
        t.selected()
    } else {
        Style::default().fg(t.text)
    };
    let wide = inner >= ONE_ROW_FROM;
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(t.accent)),
        agent_glyph(app, &agent.status),
        Span::raw(" "),
        Span::styled(pad(&short_id(&agent.id, 4, 2), 8), id_style),
        Span::raw("  "),
        Span::styled(
            pad(
                agent.label.as_deref().unwrap_or("-"),
                if wide { 18 } else { 12 },
            ),
            Style::default().fg(t.text),
        ),
        Span::raw(" "),
    ];
    if wide {
        spans.push(Span::styled(
            pad(agent.tier.as_deref().unwrap_or("-"), 8),
            Style::default().fg(t.text_muted),
        ));
        spans.push(Span::styled(
            format!("{:>6}  ${:.2}  ", tokens_text(agent.tokens), agent.cost),
            Style::default().fg(t.text),
        ));
    }
    spans.push(Span::styled(
        format!("{:>6}", duration_text(agent.duration_ms)),
        Style::default().fg(t.text_muted),
    ));
    if let Some(branch) = agent.branch.as_ref().filter(|_| wide) {
        spans.push(Span::styled(
            format!("  {branch}"),
            Style::default().fg(t.text_subtle),
        ));
    }
    Line::from(spans)
}

fn draw_log(f: &mut Frame, app: &App, log: &LogView, area: Rect) {
    let block = frame(
        app,
        area.width,
        format!("{} · log", log.name),
        short_id(&log.run_id, 9, 2),
    );
    let (body, keys) = split(&block, area);
    f.render_widget(block, area);
    let inner = body.width as usize;
    let lines: Vec<Line> = match (&log.lines.data, &log.lines.error) {
        (_, Some(error)) => vec![muted(app, format!(" {error}"))],
        (None, None) => vec![muted(app, " Loading…")],
        (Some(lines), None) if lines.is_empty() => vec![muted(app, " Nothing logged yet.")],
        (Some(lines), None) => lines
            .iter()
            .skip(log.scroll as usize)
            .map(|line| Line::from(truncate_end(&format!(" {line}"), inner)))
            .collect(),
    };
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(muted(app, " ↑↓ scroll · esc back")), keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_in_words_and_short_forms() {
        assert_eq!(money_words(3.41), "3 dollars 41");
        assert_eq!(money_words(0.04), "4 cents");
        assert_eq!(money_words(10.0), "10 dollars");
        assert_eq!(time_words(724_000), "12 minutes");
        assert_eq!(time_words(1_000), "1 second");
        assert_eq!(time_words(3_720_000), "1 hour 2 minutes");
        assert_eq!(short_id("run_01JA2BCDEFK3", 9, 2), "run_01JA2…K3");
        assert_eq!(short_id("agt_1", 4, 2), "agt_1");
        assert_eq!(duration_text(Some(41_000)), "41s");
        assert_eq!(duration_text(Some(62_000)), "1m02s");
        assert_eq!(tokens_text(12_100), "12K");
        assert_eq!(pad("audit", 7), "audit  ");
        assert_eq!(pad("a-long-name", 5), "a-lo…");
    }

    #[test]
    fn layouts_by_inner_width() {
        assert_eq!(width_of(158), Width::Wide);
        assert_eq!(width_of(118), Width::Wide);
        assert_eq!(width_of(78), Width::Medium);
        assert_eq!(width_of(48), Width::Narrow);
        let (name, bar) = columns(Width::Wide, 118);
        assert_eq!(name, NAME);
        assert!(tokens_shown(Width::Wide, 118));
        assert_eq!(
            3 + name
                + 1
                + STATUS
                + 1
                + PHASE
                + 1
                + bar
                + 2
                + AGENTS
                + 1
                + TOKENS
                + 1
                + COST
                + 1
                + ELAPSED,
            118
        );
        // Too narrow for tokens beside the shortest bar: the column is left out.
        assert!(!tokens_shown(Width::Wide, 100));
        let (name, bar) = columns(Width::Wide, 100);
        assert_eq!((name, bar), (NAME, BAR_MIN));
        assert!(!tokens_shown(Width::Medium, 78));
    }
}
