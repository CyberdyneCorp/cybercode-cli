//! The message list: Markdown text with highlighted code, collapsible reasoning and tool
//! cards.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::wrap::{wrap, wrap_hanging};
use crate::app::App;
use crate::markdown::{Ctx, Rendered};
use crate::model::{Item, Tool};
use crate::osc::ScreenLink;
use crate::theme::Tokens;

const COLLAPSE_OVER: usize = 30;
const COLLAPSED_TAIL: usize = 10;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(1) as usize;
    let mut out = transcript(app, width);
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
}

impl Out<'_> {
    fn push(&mut self, line: Line<'static>) {
        self.rendered.lines.extend(wrap(line, self.ctx.width));
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
pub fn transcript(app: &App, width: usize) -> Rendered {
    let t = &app.theme;
    let mut out = Out {
        ctx: Ctx::new(t, width, app.hyperlinks),
        rendered: Rendered::default(),
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
    notices(app, None, &mut out);
    for (i, item) in app.items.iter().enumerate() {
        item_lines(app, item, &mut out);
        out.push(Line::default());
        if let Some(marker) = markers.iter().find(|m| m.after == i) {
            out.push(marker_line(app, marker));
            out.push(Line::default());
        }
        notices(app, Some(item.id()), &mut out);
    }
    for (id, text) in &app.streaming {
        if !app.items.iter().any(|i| matches!(i, Item::Assistant { id: aid, text: done, .. } if aid == id && !done.is_empty())) {
            let rendered = app.markdown.streaming(id, text, &out.ctx);
            out.markdown(&rendered);
            out.push(Line::default());
        }
    }
    out.rendered
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
    let t = &app.theme;
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
            out.push(Line::from(Span::styled(
                "  ∴ thinking (Ctrl+T to show)",
                Style::default()
                    .fg(t.text_muted)
                    .add_modifier(Modifier::ITALIC),
            )));
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
        tool_card(app, tool, out);
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
    let rows = tool_rows(app, tool, &out.ctx);
    let collapsed = !app.expand_tools && rows.lines.len() > COLLAPSE_OVER;
    let shown = if collapsed {
        &rows.lines[rows.lines.len() - COLLAPSED_TAIL..]
    } else {
        &rows.lines[..]
    };
    if collapsed {
        out.push(Line::from(Span::styled(
            format!(
                "    … {} lines hidden (Ctrl+O to expand)",
                rows.lines.len() - COLLAPSED_TAIL
            ),
            Style::default().fg(t.text_muted),
        )));
    }
    let gutter = Span::styled("    │ ", Style::default().fg(t.border));
    for line in shown {
        let mut spans = vec![gutter.clone()];
        spans.extend(line.spans.iter().cloned());
        let hang = format!("    │ {:1$}", "", number_width(tool, line));
        out.push_hanging(Line::from(spans), Span::styled(hang, gutter.style));
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
