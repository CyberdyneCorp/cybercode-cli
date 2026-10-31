//! The message list: Markdown text with highlighted code, collapsible reasoning and tool
//! cards.

use std::ops::Range;
use std::sync::Arc;

use cyber_core::untrusted::{self, UntrustedRecord};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::wrap::{wrap, wrap_hanging};
use crate::app::App;
use crate::markdown::{Ctx, Rendered};
use crate::model::{Item, Tool};
use crate::mouse::Target;
use crate::osc::ScreenLink;
use crate::theme::Tokens;

const COLLAPSE_OVER: usize = 30;
const COLLAPSED_TAIL: usize = 10;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(1) as usize;
    let (mut out, toggles) = rows(app, width);
    let selected = select(app, &mut out);
    let height = area.height as usize;
    let max_scroll = out.lines.len().saturating_sub(height);
    let scroll = (app.scroll as usize).min(max_scroll);
    let mut start = out.lines.len().saturating_sub(height + scroll);
    if let Some(row) = selected.filter(|&r| r < start || r >= start + height) {
        start = row
            .saturating_sub(height / 3)
            .min(out.lines.len().saturating_sub(height));
    }
    let links = out
        .links
        .iter()
        .filter(|l| l.row >= start && l.row < start + height)
        .map(|l| ScreenLink {
            x: area.x + l.col as u16,
            y: area.y + (l.row - start) as u16,
            text: l.text.clone(),
            url: l.url.clone(),
        })
        .collect();
    app.set_screen_links(links);
    for (lines, action) in toggles {
        if let Some(key) = app.keymap.event("global", action) {
            super::push_lines(app, area, lines, start, Target::Key(key));
        }
    }
    let visible: Vec<Line> = out.lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(visible), area);
}

/// Mark the selected code block's label row; its row, if it is drawn.
fn select(app: &App, out: &mut Rendered) -> Option<usize> {
    let code = out.code.get(app.code_select?)?;
    let style = app.theme.selected();
    let line = out.lines.get_mut(code.row)?;
    if let Some(pad) = line.spans.get_mut(code.span) {
        *pad = Span::styled("› ", style);
    }
    if let Some(label) = line.spans.get_mut(code.span + 1) {
        label.style = style;
    }
    Some(code.row)
}

/// Rows already wrapped to `width`, with Markdown links and code blocks.
struct Out<'a> {
    ctx: Ctx<'a>,
    rendered: Rendered,
    /// Collapsed rows and the global action that expands them, for clicks.
    toggles: Vec<Toggle>,
}

/// The rows of a collapsed line and the action that toggles it.
type Toggle = (Range<usize>, &'static str);

impl Out<'_> {
    fn push(&mut self, line: Line<'static>) {
        self.rendered.lines.extend(wrap(line, self.ctx.width));
    }

    /// [`Out::push`] for a collapsed line that `action` expands.
    fn push_toggle(&mut self, line: Line<'static>, action: &'static str) {
        let start = self.rendered.lines.len();
        self.push(line);
        self.toggles
            .push((start..self.rendered.lines.len(), action));
    }

    /// [`Out::push`], with the wrapped rows continuing after `hang`.
    fn push_hanging(&mut self, line: Line<'static>, hang: Span<'static>) {
        let rows = wrap_hanging(line, self.ctx.width, &[hang]);
        self.rendered.lines.extend(rows);
    }

    fn markdown(&mut self, rendered: &Rendered) {
        self.rendered.append(rendered);
    }
}

/// Every row of the conversation at `width`.
#[cfg(test)]
pub fn transcript(app: &App, width: usize) -> Rendered {
    rows(app, width).0
}

/// [`transcript`], and the collapsed rows a click expands.
fn rows(app: &App, width: usize) -> (Rendered, Vec<Toggle>) {
    let t = &app.theme;
    let mut out = Out {
        ctx: Ctx::new(t, width, app.hyperlinks),
        rendered: Rendered::default(),
        toggles: Vec::new(),
    };
    if app.items.is_empty() && app.streaming.is_empty() {
        out.push(Line::from(Span::styled(
            format!(
                "  {} — ask anything. /help for commands.",
                crate::sidebar::display_path(&app.session.directory)
            ),
            Style::default().fg(t.text_muted),
        )));
    }
    let markers = crate::turns::markers(&app.items, app.session.running);
    let (quiet, hidden) = crate::loops::quiet_runs(&app.items, &app.loops);
    notices(app, None, &mut out);
    for (i, item) in app.items.iter().enumerate() {
        if hidden.contains(&i) {
            continue;
        }
        if let Some((count, label)) = quiet.get(&i) {
            out.push(quiet_line(app, *count, label));
            out.push(Line::default());
            continue;
        }
        item_lines(app, item, &mut out);
        out.push(Line::default());
        if let Some(marker) = markers.iter().find(|m| m.after == i) {
            out.push(marker_line(app, marker));
            out.push(Line::default());
        }
        notices(app, Some(item.id()), &mut out);
    }
    for (id, text) in app.unlisted_streams() {
        let rendered = app.markdown.streaming(id, text, &out.ctx);
        out.markdown(&rendered);
        out.push(Line::default());
    }
    (out.rendered, out.toggles)
}

/// The Session's workflow run notices that came after the item `after` (`None`: before
/// the first item, or after one no longer shown).
fn notices(app: &App, after: Option<&str>, out: &mut Out) {
    let t = &app.theme;
    let shown = |id: &str| app.items.iter().any(|item| item.id() == id);
    for notice in app
        .watch
        .notices
        .iter()
        .filter(|n| n.session_id == app.session.id)
    {
        let anchor = notice.after.as_deref().filter(|id| shown(id));
        if anchor != after {
            continue;
        }
        let color = match notice.text.chars().next() {
            Some('✓') => t.success,
            Some('✗') => t.error,
            Some('$' | '⏸') => t.warning,
            _ => t.text_muted,
        };
        out.push(Line::from(Span::styled(
            format!("  {}", notice.text),
            Style::default().fg(color),
        )));
        out.push(Line::default());
    }
}

/// `▣ Build · amini-coder · 11.3s`, with the end time under `/timestamps` and the
/// outcome of an interrupted or failed run; a sentence in screen-reader mode.
fn marker_line(app: &App, m: &crate::turns::Marker) -> Line<'static> {
    use crate::turns::{Outcome, duration, title_case};
    let t = &app.theme;
    let agent = title_case(&m.agent);
    let model = app.catalog.model_name(&m.model);
    let took = duration(m.duration_ms);
    let (outcome, color) = match m.outcome {
        Outcome::Done => ("", t.text_muted),
        Outcome::Interrupted => ("interrupted", t.warning),
        Outcome::Failed => ("failed", t.error),
    };
    let at = app
        .timestamps
        .then(|| crate::turns::clock(m.ended_ms, app.utc_offset_minutes));
    if app.screen_reader {
        let mut text = format!("  Turn finished: agent {agent}, model {model}, {took}");
        for extra in [at.as_deref(), Some(outcome)].into_iter().flatten() {
            if !extra.is_empty() {
                text.push_str(&format!(", {extra}"));
            }
        }
        return Line::from(Span::styled(text, Style::default().fg(t.text_muted)));
    }
    let tint = Style::default().fg(super::composer::agent_color(app, &m.agent));
    let muted = Style::default().fg(t.text_muted);
    let mut spans = vec![
        Span::styled("  ▣ ", tint),
        Span::styled(agent, tint),
        Span::styled(format!(" · {model} · {took}"), muted),
    ];
    if let Some(at) = at {
        spans.push(Span::styled(format!(" · {at}"), muted));
    }
    if !outcome.is_empty() {
        spans.push(Span::styled(" · ", muted));
        spans.push(Span::styled(outcome, Style::default().fg(color)));
    }
    Line::from(spans)
}

fn item_lines(app: &App, item: &Item, out: &mut Out) {
    body_lines(app, item, out);
    if let Item::User {
        untrusted: Some(record),
        ..
    } = item
    {
        untrusted_notice(app, record, out);
    }
}

fn body_lines(app: &App, item: &Item, out: &mut Out) {
    let t = &app.theme;
    if let Some(lines) = message_lines(app, item, out.ctx.width) {
        for line in lines {
            out.push(line);
        }
        return;
    }
    match item {
        Item::User { text, .. } => {
            let bold = Style::default().fg(t.user).add_modifier(Modifier::BOLD);
            for (i, line) in text.lines().enumerate() {
                let prefix = if i == 0 { "› " } else { "  " };
                out.push_hanging(
                    Line::from(vec![
                        Span::styled(prefix, bold),
                        Span::styled(line.to_string(), Style::default().fg(t.user)),
                    ]),
                    Span::styled("  ", bold),
                );
            }
        }
        Item::System { text, .. } => {
            let first = text
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('<'))
                .unwrap_or("context updated");
            out.push(Line::from(Span::styled(
                format!("  ⚙ {first}"),
                Style::default()
                    .fg(t.text_muted)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
        Item::Assistant {
            id,
            text,
            reasoning,
            tools,
            error,
            ..
        } => {
            assistant(app, id, text, reasoning, tools, error.as_deref(), out);
        }
    }
}

/// `↻ 6 quiet checks · loop 7xk2`: consecutive iterations that found nothing new.
fn quiet_line(app: &App, count: usize, label: &str) -> Line<'static> {
    let t = &app.theme;
    let checks = if count == 1 { "check" } else { "checks" };
    Line::from(vec![
        Span::styled("↻ ", Style::default().fg(t.accent)),
        Span::styled(
            format!("{count} quiet {checks} · loop {label}"),
            Style::default().fg(t.text_muted),
        ),
    ])
}

/// `↻ check the deploy · loop 7xk2 #3`: an iteration's prompt.
fn iteration_lines(app: &App, id: &str, text: &str) -> Option<Vec<Line<'static>>> {
    let (label, n) = crate::loops::iteration_label(id)?;
    let t = &app.theme;
    let first = text.lines().next().unwrap_or_default().to_string();
    Some(vec![Line::from(vec![
        Span::styled("↻ ", Style::default().fg(t.accent)),
        Span::styled(first, Style::default().fg(t.user)),
        Span::styled(
            format!(" · loop {label} #{n}"),
            Style::default().fg(t.text_muted),
        ),
    ])])
}

/// A received message or an idle notice, drawn as such only when the Session holds its
/// `in` record or the server named it a notice; a loop iteration's prompt.
fn message_lines(app: &App, item: &Item, width: usize) -> Option<Vec<Line<'static>>> {
    let Item::User { id, text, .. } = item else {
        return None;
    };
    if crate::loops::is_iteration(id) {
        return iteration_lines(app, id, text);
    }
    if let Some(line) = crate::goals::prompt_line(text) {
        let t = &app.theme;
        let (mark, rest) = line.split_at(line.find(' ').unwrap_or(0));
        return Some(vec![Line::from(vec![
            Span::styled(mark.to_string(), Style::default().fg(t.accent)),
            Span::styled(rest.to_string(), Style::default().fg(t.text_muted)),
        ])]);
    }
    if let Some(record) = app.messaging.received(id) {
        return Some(super::messaging::received(app, record, width));
    }
    let notice = crate::messaging::Notice::of(item)?;
    Some(vec![super::messaging::notice(app, &notice)])
}

fn assistant(
    app: &App,
    id: &str,
    text: &str,
    reasoning: &str,
    tools: &[Tool],
    error: Option<&str>,
    out: &mut Out,
) {
    let t = &app.theme;
    if !reasoning.is_empty() {
        if app.show_reasoning {
            for line in reasoning.lines() {
                out.push(Line::from(Span::styled(
                    format!("  ∴ {line}"),
                    Style::default()
                        .fg(t.text_muted)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        } else {
            let line = Line::from(Span::styled(
                "  ∴ thinking (Ctrl+T to show)",
                Style::default()
                    .fg(t.text_muted)
                    .add_modifier(Modifier::ITALIC),
            ));
            out.push_toggle(line, "reasoning_toggle");
        }
    }
    let rendered = match app.streaming.get(id) {
        Some(streaming) if text.is_empty() => {
            Arc::new(app.markdown.streaming(id, streaming, &out.ctx))
        }
        _ => app.markdown.message(text, &out.ctx),
    };
    out.markdown(&rendered);
    for tool in tools {
        match super::messaging::sent(app, tool, out.ctx.width) {
            Some(lines) => lines.into_iter().for_each(|line| out.push(line)),
            None => tool_card(app, tool, out),
        }
    }
    if let Some(e) = error {
        out.push(Line::from(Span::styled(
            format!("  ✗ {e}"),
            Style::default().fg(t.error),
        )));
    }
}

fn status_style(t: &Tokens, status: &str) -> Style {
    let color = match status {
        "completed" => t.success,
        "failed" | "unknown" => t.error,
        "interrupted" => t.warning,
        _ => t.accent,
    };
    Style::default().fg(color)
}

fn tool_card(app: &App, tool: &Tool, out: &mut Out) {
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
            Style::default().fg(t.text_muted),
        ),
        Span::styled(format!("  {status}"), status_style(t, status)),
    ]));
    if let Some(note) = tool.redaction_note() {
        out.push(Line::from(Span::styled(
            format!("    ⊘ {note}"),
            Style::default().fg(t.warning),
        )));
    }
    let rows = tool_rows(app, tool, &out.ctx);
    let collapsed = !app.expand_tools && rows.lines.len() > COLLAPSE_OVER;
    let shown = if collapsed {
        &rows.lines[rows.lines.len() - COLLAPSED_TAIL..]
    } else {
        &rows.lines[..]
    };
    if collapsed {
        let line = Line::from(Span::styled(
            format!(
                "    … {} lines hidden (Ctrl+O to expand)",
                rows.lines.len() - COLLAPSED_TAIL
            ),
            Style::default().fg(t.text_muted),
        ));
        out.push_toggle(line, "tool_details");
    }
    let gutter = Span::styled("    │ ", Style::default().fg(t.border));
    for line in shown {
        let mut spans = vec![gutter.clone()];
        spans.extend(line.spans.iter().cloned());
        let hang = format!("    │ {:1$}", "", number_width(tool, line));
        out.push_hanging(Line::from(spans), Span::styled(hang, gutter.style));
    }
    if let Some(record) = &tool.untrusted {
        untrusted_notice(app, record, out);
    }
}

/// The injection signal notice under a result or message (`untrusted-content` →
/// Injection signal notices), with its consequence for the Session's Mode; the excerpts
/// follow while tool details are expanded (Ctrl+O). It adds rows and never hides the
/// content above it.
fn untrusted_notice(app: &App, record: &UntrustedRecord, out: &mut Out) {
    let mode = if app.session.effective_mode.is_empty() {
        &app.session.mode
    } else {
        &app.session.effective_mode
    };
    let consequence = untrusted::consequence(mode, true, app.session.untrusted_policy);
    let Some(line) = record.notice(consequence) else {
        return;
    };
    let style = Style::default().fg(app.theme.warning);
    out.push_hanging(
        Line::from(Span::styled(format!("    {line}"), style)),
        Span::styled("      ", style),
    );
    if !app.expand_tools {
        return;
    }
    let muted = Style::default().fg(app.theme.text_muted);
    for signal in &record.signals {
        let excerpt = untrusted::visible_value(&signal.excerpt);
        out.push_hanging(
            Line::from(Span::styled(
                format!("      {}: {excerpt}", signal.kind.as_str()),
                muted,
            )),
            Span::styled("        ", muted),
        );
    }
}

/// Columns of a highlighted `read` row's line number (`12 `), so that its wrapped rows
/// continue under the code instead of reading as another line number; 0 otherwise.
fn number_width(tool: &Tool, line: &Line) -> usize {
    if tool.name != "read" {
        return 0;
    }
    let number = |s: &&Span| {
        let n = s.content.trim_start();
        n.len() > 1 && n.ends_with(' ') && n.trim_end().bytes().all(|b| b.is_ascii_digit())
    };
    line.spans.first().filter(number).map_or(0, Span::width)
}

/// A tool's output rows; `read` and `write` code highlighted by the file's syntax once
/// the call succeeded.
fn tool_rows(app: &App, tool: &Tool, ctx: &Ctx) -> Arc<Rendered> {
    let key = (
        &tool.call_id,
        &tool.status,
        &tool.output,
        tool.input["content"].as_str(),
        ctx.key(),
    );
    app.markdown.memo(&key, || {
        let t = ctx.t;
        let lines = code_rows(tool, t).unwrap_or_else(|| {
            tool.output
                .lines()
                .map(|line| Line::from(Span::styled(line.to_string(), diff_style(t, line))))
                .collect()
        });
        Rendered {
            lines,
            ..Rendered::default()
        }
    })
}

fn code_rows(tool: &Tool, t: &Tokens) -> Option<Vec<Line<'static>>> {
    if tool.status != "ok" {
        return None;
    }
    let path = tool.input["path"].as_str()?;
    let (before, numbers, code, after) = match tool.name.as_str() {
        "read" => {
            let (numbers, code, notes) = numbered(&tool.output)?;
            (Vec::new(), numbers, code, notes)
        }
        "write" => {
            let code = tool.input["content"].as_str()?.to_owned();
            let before = tool.output.lines().map(str::to_owned).collect();
            (before, Vec::new(), code, Vec::new())
        }
        _ => return None,
    };
    let set = crate::highlight::syntaxes()?;
    let syntax = crate::highlight::for_path(set, path)?;
    let highlighted = crate::highlight::highlight(set, syntax, &code.replace('\t', "    "), t)?;
    let muted = |l: String| Line::from(Span::styled(l, Style::default().fg(t.text_muted)));
    let digits = numbers.iter().map(String::len).max().unwrap_or(0);
    let mut lines: Vec<Line<'static>> = before.into_iter().map(muted).collect();
    for (i, runs) in highlighted.into_iter().enumerate() {
        let mut spans = Vec::new();
        if let Some(n) = numbers.get(i) {
            spans.push(Span::styled(
                format!("{n:>digits$} "),
                Style::default().fg(t.text_subtle),
            ));
        }
        spans.extend(runs.into_iter().map(|(style, s)| Span::styled(s, style)));
        lines.push(Line::from(spans));
    }
    lines.extend(after.into_iter().map(muted));
    Some(lines)
}

/// `read` output: `<n>: <line>` rows, then notes such as `(next_offset: 5)`, as the line
/// numbers, the code and the notes; `None` when it has no numbered rows.
fn numbered(output: &str) -> Option<(Vec<String>, String, Vec<String>)> {
    let mut numbers = Vec::new();
    let mut code = String::new();
    let mut notes = Vec::new();
    for line in output.lines() {
        let row = line
            .split_once(": ")
            .filter(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        match row {
            Some((n, text)) if notes.is_empty() => {
                numbers.push(n.to_owned());
                code.push_str(text);
                code.push('\n');
            }
            _ => notes.push(line.to_owned()),
        }
    }
    (!numbers.is_empty()).then_some((numbers, code, notes))
}

/// Unified-diff lines use the `diff_*` tokens; other output is muted.
pub(super) fn diff_style(t: &Tokens, line: &str) -> Style {
    if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(t.diff_added).bg(t.diff_added_bg)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(t.diff_removed).bg(t.diff_removed_bg)
    } else if line.starts_with("@@") {
        Style::default().fg(t.info)
    } else {
        Style::default().fg(t.diff_context)
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}
