//! `/agents`, full screen over the transcript (`tui` → Agent directory): the rows of
//! every source grouped as This server, Peers and Project hub, each group with its own
//! loading or error line, the compose or rename editor, and two key lines. State is a
//! glyph and a word, so it reads without color.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::layout::{shorten_path, truncate_end};
use crate::agents::{
    Directory, Editor, EditorKind, Entry, Group, KindFilter, Recipient, Source, State, Status,
};
use crate::app::App;
use crate::mouse::{List, Target};

const ADDRESS: usize = 26;
const KIND: usize = 14;
const STATE: usize = 13;
const OPEN: usize = 6;
/// Below this width the Location column goes, then below [`NO_KIND`] the kind.
const NO_LOCATION: usize = 100;
const NO_KIND: usize = 70;

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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            truncate_end(&title(app), area.width.saturating_sub(50) as usize),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .title_top(Line::from(chips(app)).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    f.render_widget(
        Paragraph::new(filter_line(app, width)),
        Rect { height: 1, ..inner },
    );
    let editor = app
        .directory
        .editor
        .as_ref()
        .map_or(0, |e| editor_height(e, inner.height));
    let body = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(3 + editor),
        ..inner
    };
    draw_body(f, app, body);
    if let Some(e) = &app.directory.editor {
        let rect = Rect {
            y: body.bottom(),
            height: editor,
            ..inner
        };
        draw_editor(f, app, e, rect);
    }
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

/// ` Agents · review ` (the attached Session's title, else its ID).
fn title(app: &App) -> String {
    let name = crate::sidebar::clean(&app.session.title);
    let name = if name.is_empty() {
        app.session.id.clone()
    } else {
        name
    };
    format!(" Agents · {name} ")
}

/// `[all] sessions  subagents  teammates  peers  hub`.
fn chips(app: &App) -> Vec<Span<'static>> {
    let (all, current) = (&KindFilter::ALL, app.directory.kind);
    super::filter_chips(all, current, KindFilter::label, &app.theme)
}

fn filter_line(app: &App, width: usize) -> Line<'static> {
    let t = &app.theme;
    if app.directory.filter.is_empty() {
        let hint = " / type to filter by address, name, kind or location";
        return Line::from(Span::styled(
            truncate_end(hint, width),
            Style::default().fg(t.text_subtle),
        ));
    }
    Line::from(vec![
        Span::styled(" / ", Style::default().fg(t.accent)),
        Span::styled(
            truncate_end(&app.directory.filter, width.saturating_sub(4)),
            Style::default().fg(t.text),
        ),
        Span::styled("▏", Style::default().fg(t.accent)),
    ])
}

/// The body's lines and, for each visible row, its line.
struct Body {
    lines: Vec<Line<'static>>,
    rows: Vec<usize>,
}

fn draw_body(f: &mut Frame, app: &App, body: Rect) {
    let d = &app.directory;
    let visible = d.visible();
    let built = body_lines(app, &visible, body.width as usize);
    let selected = d
        .selected_index(&visible)
        .and_then(|i| built.rows.get(i).copied())
        .map_or(0..0, |row| row..row + 1);
    let height = body.height as usize;
    let start = app
        .mouse
        .window(List::Directory, selected, height, built.lines.len());
    for (i, line) in built.rows.iter().enumerate() {
        super::push_lines(
            app,
            body,
            *line..line + 1,
            start,
            Target::row(List::Directory, i),
        );
    }
    let shown: Vec<Line> = built.lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(shown), body);
}

fn body_lines(app: &App, visible: &[&Entry], width: usize) -> Body {
    let d = &app.directory;
    let mut body = Body {
        lines: Vec::new(),
        rows: Vec::new(),
    };
    if d.empty() {
        body.lines = empty_lines(app, width);
        return body;
    }
    let selected = d.selected_index(visible);
    for group in Group::ALL {
        let rows: Vec<(usize, &Entry)> = visible
            .iter()
            .enumerate()
            .filter(|(_, e)| e.group() == group)
            .map(|(i, e)| (i, *e))
            .collect();
        let notes = group_notes(d, group);
        if rows.is_empty() && notes.is_empty() {
            continue;
        }
        body.lines.push(header(app, group));
        for note in notes {
            body.lines.push(note_line(app, &note, width));
        }
        for (i, entry) in rows {
            body.rows.push(body.lines.len());
            body.lines
                .push(row_line(app, entry, width, Some(i) == selected));
        }
        body.lines.push(Line::default());
    }
    if visible.is_empty() && d.rows().next().is_some() {
        body.lines.push(no_match(app, width));
    }
    body
}

fn header(app: &App, group: Group) -> Line<'static> {
    let t = &app.theme;
    Line::from(Span::styled(
        format!(" {}", group.title()),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    ))
}

/// A group's loading, disabled and error lines. Peers share the listing's lines, which
/// This server shows.
fn group_notes(d: &Directory, group: Group) -> Vec<String> {
    let source = match group {
        Group::Server => d.source(Source::Messaging),
        Group::Hub => d.source(Source::Hub),
        Group::Peers => return Vec::new(),
    };
    let mut notes = Vec::new();
    if source.loading && source.rows.is_empty() {
        notes.push("loading…".to_owned());
    }
    match (group, source.status) {
        (Group::Server, Status::Disabled) => {
            notes.push("messaging is disabled here (messaging.enabled)".into())
        }
        (Group::Server, Status::Unsupported) => {
            notes.push("this server does not list reachable Sessions".into())
        }
        _ => {}
    }
    if let Some(error) = &source.error {
        notes.push(match group {
            Group::Hub => format!("project hub: {error}"),
            _ => format!("could not refresh: {error}"),
        });
    }
    notes
}

fn note_line(app: &App, note: &str, width: usize) -> Line<'static> {
    let t = &app.theme;
    let style = if note.starts_with("loading") {
        Style::default().fg(t.text_muted)
    } else {
        Style::default().fg(t.warning)
    };
    Line::from(Span::styled(
        truncate_end(&format!("   {note}"), width),
        style,
    ))
}

/// The glyph and its color: running `●`, waiting `!`, online `●`, idle and offline `○`.
fn glyph(app: &App, state: State) -> (&'static str, Style) {
    let t = &app.theme;
    let (glyph, color) = match state {
        State::Running => ("●", t.info),
        State::WaitingInput => ("!", t.warning),
        State::Online => ("●", t.success),
        State::Idle => ("○", t.text_muted),
        State::Offline => ("○", t.text_subtle),
        State::Unknown => ("·", t.text_subtle),
    };
    (glyph, Style::default().fg(color))
}

/// `   ● migration   session   running   open   ~/work/app`; a row with no safe address
/// is marked `⚠`.
fn row_line(app: &App, entry: &Entry, width: usize, selected: bool) -> Line<'static> {
    let t = &app.theme;
    let (glyph, glyph_style) = glyph(app, entry.state);
    let address = if entry.target.is_none() && entry.participant.is_none() {
        format!("⚠ {}", entry.address)
    } else {
        entry.address.clone()
    };
    let mut text = format!("{:<ADDRESS$}", truncate_end(&address, ADDRESS));
    if width >= NO_KIND {
        text.push_str(&format!(" {:<KIND$}", entry.kind.label()));
    }
    let open = if entry.can_receive { "open" } else { "closed" };
    text.push_str(&format!(" {:<STATE$} {open:<OPEN$}", entry.state.label()));
    if width >= NO_LOCATION {
        let room = width.saturating_sub(text.width() + 6);
        text.push_str(&format!(" {}", location(app, entry, room)));
    }
    let text = truncate_end(&text, width.saturating_sub(4));
    if selected {
        let pad = width.saturating_sub(text.width() + 4);
        let line = format!("  {glyph} {text}{}", " ".repeat(pad));
        return Line::from(Span::styled(line, t.selected()));
    }
    let style = if entry.can_receive {
        Style::default().fg(t.text)
    } else {
        Style::default().fg(t.text_muted)
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(glyph, glyph_style),
        Span::styled(format!(" {text}"), style),
    ])
}

/// A local directory from `~`, shortened in the middle; peers and the hub as labelled.
fn location(app: &App, entry: &Entry, room: usize) -> String {
    match &entry.location {
        crate::agents::Location::Local(directory) => shorten_path(
            &crate::sidebar::home_path(directory, app.home.as_deref()),
            room,
        ),
        other => truncate_end(&other.label(), room),
    }
}

fn no_match(app: &App, width: usize) -> Line<'static> {
    let d = &app.directory;
    let mut text = format!("  No match for \"{}\"", d.filter);
    if d.kind != KindFilter::All {
        text.push_str(&format!(" in {}", d.kind.label()));
    }
    text.push_str(" · esc clears");
    Line::from(Span::styled(
        truncate_end(&text, width),
        Style::default().fg(app.theme.text_muted),
    ))
}

fn empty_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let name = title(app);
    let name = name.trim().trim_start_matches("Agents · ");
    let muted = Style::default().fg(t.text_muted);
    [
        (
            format!("  No other Session can be reached from {name}."),
            Style::default().fg(t.text),
        ),
        (
            "  Pair a machine with `cyber peers add` to reach its Sessions.".into(),
            muted,
        ),
        (
            "  Bind a project with /collab to reach its hub agents.".into(),
            muted,
        ),
    ]
    .into_iter()
    .map(|(text, style)| Line::from(Span::styled(truncate_end(&text, width), style)))
    .collect()
}

/// The editor's frame, its lines (at most five shown) and the status line.
fn editor_height(editor: &Editor, room: u16) -> u16 {
    let lines = editor.input.split('\n').count().clamp(1, 5) as u16;
    (lines + 3).min(room.saturating_sub(5))
}

fn draw_editor(f: &mut Frame, app: &App, editor: &Editor, area: Rect) {
    let t = &app.theme;
    let title = match &editor.kind {
        EditorKind::Rename { .. } => format!(" Rename {} ", editor.label),
        EditorKind::Message {
            to: Recipient::Hub { .. },
        } => format!(" Hub message to {} as your account ", editor.label),
        EditorKind::Message { .. } => format!(" Message to {} ", editor.label),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_active))
        .title(Span::styled(
            truncate_end(&title, area.width.saturating_sub(4) as usize),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    let width = inner.width as usize;
    let input: Vec<&str> = editor.input.split('\n').collect();
    let room = inner.height.saturating_sub(1) as usize;
    let mut lines: Vec<Line> = input[input.len().saturating_sub(room)..]
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                truncate_end(l, width),
                Style::default().fg(t.text),
            ))
        })
        .collect();
    if let Some(last) = lines.last_mut() {
        last.spans
            .push(Span::styled("▏", Style::default().fg(t.accent)));
    }
    lines.push(editor_status(app, editor, width));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// `sending…`, the last error, or the editor's keys.
fn editor_status(app: &App, editor: &Editor, width: usize) -> Line<'static> {
    let t = &app.theme;
    let (text, style) = match (&editor.error, editor.busy) {
        (_, true) => ("sending…".to_owned(), Style::default().fg(t.text_muted)),
        (Some(error), false) => (error.clone(), Style::default().fg(t.error)),
        (None, false) => (editor_keys(editor), Style::default().fg(t.text_muted)),
    };
    Line::from(Span::styled(truncate_end(&text, width), style))
}

fn editor_keys(editor: &Editor) -> String {
    match &editor.kind {
        EditorKind::Rename { .. } => "enter save · esc cancel".into(),
        EditorKind::Message {
            to: Recipient::Listing(_),
        } => {
            let notify = if editor.notify { "on" } else { "off" };
            format!(
                "enter send · shift+enter newline · ctrl+n notify when idle: {notify} · esc cancel"
            )
        }
        EditorKind::Message { .. } => "enter send · shift+enter newline · esc cancel".into(),
    }
}

fn key_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let keys: &[&str] = if app.directory.editor.is_some() {
        &[]
    } else {
        &[
            "↑↓ move",
            "type filter",
            "tab kind",
            "enter message",
            "ctrl+w watch",
            "ctrl+o open",
            "ctrl+r rename",
            "ctrl+g held messages",
            "ctrl+l refresh",
            "esc clear/close",
        ]
    };
    let mut lines = super::key_hints(keys, width);
    lines.truncate(2);
    lines
}
