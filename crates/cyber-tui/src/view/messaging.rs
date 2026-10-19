//! Cross-session message surfaces (`tui` → Cross-session message surfaces, design
//! section 15): the received block, the `send_message` call and the idle notice in the
//! transcript, the held card above the composer, the footer badge and `/messages`.
//! Every text from another Session is drawn without control characters.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::layout::truncate_end;
use crate::app::App;
use crate::messaging::{Filter, Notice, Record, Row, clean, outcome_label};
use crate::model::Tool;

/// Rows of the held card: its frame, the sender line, the text and the keys.
pub const CARD_HEIGHT: u16 = 5;
const PEER: usize = 16;
const OUTCOME: usize = 10;

/// `ses_X7KQ2M`: an ID's prefix and its last `keep` characters, the random part of a
/// ULID (its first characters are the creation time, shared by Sessions made together).
fn short(id: &str, keep: usize) -> String {
    let (prefix, rest) = id.rsplit_once('_').unwrap_or(("", id));
    let chars: Vec<char> = rest.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(keep)..].iter().collect();
    match prefix.split('_').next() {
        Some(head) if !head.is_empty() => format!("{head}_{tail}"),
        _ => tail,
    }
}

fn clock(app: &App, ms: i64) -> String {
    crate::turns::clock(ms, app.utc_offset_minutes)
}

/// `2026-10-16 14:02:00`, in the offset clock times use.
fn stamp(app: &App, ms: i64) -> String {
    use chrono::{FixedOffset, Local, TimeZone};
    let Some(utc) = chrono::DateTime::from_timestamp_millis(ms) else {
        return String::new();
    };
    let format = "%Y-%m-%d %H:%M:%S";
    match app
        .utc_offset_minutes
        .and_then(|m| FixedOffset::east_opt(m * 60))
    {
        Some(zone) => zone
            .from_utc_datetime(&utc.naive_utc())
            .format(format)
            .to_string(),
        None => Local
            .from_utc_datetime(&utc.naive_utc())
            .format(format)
            .to_string(),
    }
}

/// `41 B`, `1.2 KB`.
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        _ => format!("{:.1} KB", bytes as f64 / 1024.0),
    }
}

/// `left`, then `right` at the end of `width` columns (after one space when it does
/// not fit).
fn spread(left: Vec<Span<'static>>, right: Span<'static>, width: usize) -> Line<'static> {
    let used: usize = left.iter().map(|s| s.content.width()).sum();
    let pad = width.saturating_sub(used + right.content.width()).max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(right);
    Line::from(spans)
}

/// ` ✉ frontend · ses_01JABC · 14:02                untrusted`, then the text behind a
/// gutter.
pub(super) fn received(app: &App, record: &Record, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let mut head = vec![
        Span::styled(" ✉ ", Style::default().fg(t.info)),
        Span::styled(
            clean(&record.from),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(session) = &record.peer_session_id {
        head.push(Span::styled(format!(" · {}", short(session, 6)), muted));
    }
    head.push(Span::styled(
        format!(" · {}", clock(app, record.created_ms)),
        muted,
    ));
    let mut lines = vec![spread(
        head,
        Span::styled("untrusted ", Style::default().fg(t.warning)),
        width,
    )];
    let gutter = Style::default().fg(t.border);
    for line in record.text.as_deref().unwrap_or_default().lines() {
        lines.push(Line::from(vec![
            Span::styled(" │ ", gutter),
            Span::styled(clean(line), Style::default().fg(t.text)),
        ]));
    }
    lines
}

/// ` ◷ migration is idle · 14:40 · "migration finished"`.
pub(super) fn notice(app: &App, notice: &Notice) -> Line<'static> {
    let t = &app.theme;
    let mut text = if notice.expired {
        format!(" ◷ watch on {} expired", notice.target)
    } else {
        format!(" ◷ {} is idle", notice.target)
    };
    if let Some(ms) = notice.finished_ms {
        text.push_str(&format!(" · {}", clock(app, ms)));
    }
    if !notice.expired && !notice.status.is_empty() {
        text.push_str(&format!(" · \"{}\"", notice.status));
    }
    Line::from(Span::styled(text, Style::default().fg(t.info)))
}

/// ` ● send_message → backend              delivered · msgx_3f2a`, then the sent text;
/// `None` until the call has a result to show, so it keeps the plain tool card.
pub(super) fn sent(app: &App, tool: &Tool, width: usize) -> Option<Vec<Line<'static>>> {
    if tool.name != "send_message" || tool.status != "ok" {
        return None;
    }
    let result: serde_json::Value = serde_json::from_str(&tool.output).ok()?;
    let id = result["message_id"].as_str()?;
    let record = app.messaging.sent(id);
    let to = record.map_or_else(
        || {
            result["to"]["address"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        },
        |r| r.to.clone(),
    );
    let outcome = record.map_or_else(
        || outcome_label(result["outcome"].as_str()?, result["reason"].as_str()).into(),
        |r| Some(r.outcome_label()),
    )?;
    let t = &app.theme;
    let color = match outcome.split(':').next().unwrap_or_default() {
        "delivered" | "released" => t.success,
        "held" => t.warning,
        _ => t.error,
    };
    let head = vec![
        Span::styled(" ● ", Style::default().fg(color)),
        Span::styled(
            "send_message",
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" → {}", clean(&to)), Style::default().fg(t.text)),
    ];
    let tail = Span::styled(
        format!("{} · {} ", clean(&outcome), short(id, 4)),
        Style::default().fg(color),
    );
    let mut lines = vec![spread(head, tail, width)];
    let gutter = Style::default().fg(t.border);
    for line in tool.input["text"].as_str().unwrap_or_default().lines() {
        lines.push(Line::from(vec![
            Span::styled("   │ ", gutter),
            Span::styled(clean(line), Style::default().fg(t.text_muted)),
        ]));
    }
    Some(lines)
}

/// The footer badge.
pub(super) fn badge(app: &App) -> Option<Span<'static>> {
    let (text, held) = app.messages_field()?;
    let t = &app.theme;
    let color = if held { t.warning } else { t.text_muted };
    Some(Span::styled(text, Style::default().fg(color)))
}

/// The card's height, or 0 without a held message.
pub(super) fn card_height(app: &App) -> u16 {
    if app.messaging.card(&app.queued).is_some() {
        CARD_HEIGHT
    } else {
        0
    }
}

/// The newest held message (or the one the user moved to) above the composer.
pub(super) fn card(f: &mut Frame, app: &App, area: Rect) {
    let Some((at, n, record)) = app.messaging.card(&app.queued) else {
        return;
    };
    if area.height < CARD_HEIGHT || area.width < 20 {
        return;
    }
    let t = &app.theme;
    let focused = app.messaging.card_focus;
    let border = if focused { t.border_active } else { t.border };
    let count = if n > 1 {
        format!(" {} of {n} ", at + 1)
    } else {
        String::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            " Held message ",
            Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(count, Style::default().fg(t.text_muted))).right_aligned(),
        );
    let inner = block.inner(area);
    let room = inner.width as usize;
    let muted = Style::default().fg(t.text_muted);
    let from = format!(
        " ✉ from {} · {} · {}",
        clean(&record.from),
        clock(app, record.created_ms),
        size(record.size)
    );
    let first = record
        .text
        .as_deref()
        .and_then(|text| text.lines().find(|l| !l.trim().is_empty()))
        .map(clean)
        .unwrap_or_default();
    let keys = if focused {
        "r release · d drop · enter view · esc back "
    } else {
        "tab select · r release · d drop · enter view "
    };
    let lines = vec![
        Line::from(Span::styled(truncate_end(&from, room), muted)),
        Line::from(Span::styled(
            truncate_end(&format!(" {first}"), room),
            Style::default().fg(t.text),
        )),
        Line::from(Span::styled(truncate_end(keys, room), muted)).right_aligned(),
    ];
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// `/messages`, full screen over the transcript.
pub(super) fn view(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    if area.width < 30 || area.height < 6 {
        return;
    }
    let title = format!(
        " Messages · {} \"{}\" ",
        short(&app.session.id, 6),
        crate::sidebar::clean(&app.session.title)
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            truncate_end(&title, area.width.saturating_sub(30) as usize),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .title_top(Line::from(filters(app)).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let rows = app.messaging.rows(&app.items, &app.queued);
    let lines = body_lines(app, &rows, inner.width as usize);
    let (lines, selected_row) = lines;
    let height = body.height as usize;
    let start = selected_row
        .map_or(0, |row| row.saturating_sub(height.saturating_sub(1)))
        .min(lines.len().saturating_sub(height));
    let shown: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(shown), body);
    let keys = Rect {
        y: inner.bottom().saturating_sub(2),
        height: 2,
        ..inner
    };
    f.render_widget(
        Paragraph::new(key_lines(app, inner.width as usize))
            .style(Style::default().fg(t.text_muted)),
        keys,
    );
}

/// `[all] in  out  held`.
fn filters(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = Vec::new();
    for filter in Filter::ALL {
        let label = filter.label();
        if filter == app.messaging.filter {
            spans.push(Span::styled(format!("[{label}]"), t.selected()));
        } else {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(t.text_muted),
            ));
        }
        spans.push(Span::raw(" "));
    }
    spans
}

/// The rows, the expansion under the selected one, and the selected row's line.
fn body_lines(app: &App, rows: &[Row], width: usize) -> (Vec<Line<'static>>, Option<usize>) {
    let t = &app.theme;
    if rows.is_empty() {
        let text = "  No messages yet. Other Sessions reach this one with send_message.";
        return (
            vec![Line::from(Span::styled(
                text,
                Style::default().fg(t.text_muted),
            ))],
            None,
        );
    }
    let selected = app.messaging.selected_index(rows);
    let mut lines = Vec::new();
    let mut selected_row = None;
    for (i, row) in rows.iter().enumerate() {
        let text = truncate_end(&row_text(app, row), width);
        if Some(i) == selected {
            selected_row = Some(lines.len());
            let pad = width.saturating_sub(text.width());
            lines.push(Line::from(Span::styled(
                format!("{text}{}", " ".repeat(pad)),
                t.selected(),
            )));
            if app.messaging.expanded {
                lines.extend(expansion(app, row, width));
            }
        } else {
            lines.push(Line::from(Span::styled(text, row_style(app, row))));
        }
    }
    (lines, selected_row)
}

fn row_style(app: &App, row: &Row) -> Style {
    let t = &app.theme;
    match row {
        Row::Record(r) if r.outcome == "held" => Style::default().fg(t.warning),
        Row::Record(r) if r.outcome == "refused" || r.outcome == "dropped" => {
            Style::default().fg(t.text_muted)
        }
        Row::Notice { .. } => Style::default().fg(t.info),
        Row::Record(_) => Style::default().fg(t.text),
    }
}

/// `  14:02  in   frontend          delivered   41 B   which port does the API…`.
fn row_text(app: &App, row: &Row) -> String {
    let (direction, peer, outcome, bytes, first) = match row {
        Row::Record(r) => {
            let first = r
                .text
                .as_deref()
                .and_then(|text| text.lines().find(|l| !l.trim().is_empty()))
                .map(clean)
                .or_else(|| r.reason.clone())
                .unwrap_or_default();
            let direction = if r.incoming { "in" } else { "out" };
            (
                direction,
                clean(r.peer()),
                r.outcome.clone(),
                size(r.size),
                first,
            )
        }
        Row::Notice { notice, .. } => {
            let what = if notice.expired { "expired" } else { "is idle" };
            let first = format!("{} {what}", notice.target);
            (
                "in",
                notice.target.clone(),
                "notice".into(),
                String::new(),
                first,
            )
        }
    };
    format!(
        "  {}  {direction:<4} {:<PEER$} {:<OUTCOME$} {bytes:>7}   {first}",
        clock(app, row.time_ms()),
        truncate_end(&peer, PEER),
        truncate_end(&outcome, OUTCOME),
    )
}

/// The full text, then the record's ID, digest, Session and times.
fn expansion(app: &App, row: &Row, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let text = Style::default().fg(t.text);
    let muted = Style::default().fg(t.text_muted);
    let line = |s: String, style| Line::from(Span::styled(truncate_end(&s, width), style));
    let mut out = Vec::new();
    match row {
        Row::Record(r) => {
            let body = r
                .text
                .as_deref()
                .unwrap_or("(refused messages keep no text)");
            out.extend(
                body.lines()
                    .map(|l| line(format!("      {}", clean(l)), text)),
            );
            out.push(line(
                format!("      {} · sha256 {}", r.message_id, r.sha256),
                muted,
            ));
            let mut detail = format!("      sent {}", stamp(app, r.created_ms));
            if r.updated_ms != r.created_ms {
                detail.push_str(&format!(" · {} {}", r.outcome, stamp(app, r.updated_ms)));
            }
            if let Some(session) = &r.peer_session_id {
                detail.push_str(&format!(" · peer {session}"));
            }
            out.push(line(detail, muted));
        }
        Row::Notice { item_id, notice } => {
            out.push(line(format!("      \"{}\"", notice.status), text));
            out.push(line(format!("      {item_id}"), muted));
        }
    }
    out
}

/// The key lines; `a accept messages` only under the `non_interactive` override.
fn key_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut keys = vec![
        "↑↓ move",
        "enter expand",
        "r release",
        "d drop",
        "tab filter",
    ];
    if app.session.inbound_reason.as_deref() == Some("non_interactive") {
        keys.push("a accept messages");
    }
    keys.push("esc close");
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

#[cfg(test)]
mod tests {
    #[test]
    fn short_ids_keep_the_prefix_and_the_random_tail() {
        assert_eq!(super::short("ses_01JABCDEFGHX7KQ2M", 6), "ses_X7KQ2M");
        assert_eq!(super::short("msgx_3f2a91c0", 4), "msgx_91c0");
        assert_eq!(super::short("msgx_watch_w1", 4), "msgx_w1");
        assert_eq!(super::short("plain", 3), "ain");
    }
}
