//! Virtual hex/ASCII viewport. Only visible rows are formatted; all controls and
//! interpretations are derived from bytes, never from decoded terminal text.
use crate::{app::App, hex::Workspace, sidebar::clean, theme::Tokens};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    if area.width < 38 || area.height < 10 {
        f.render_widget(Paragraph::new("Widen the hex view (38×10 minimum)"), area);
        return;
    }
    let h = &app.hex;
    let title = h.document.as_ref().map_or("Opening…".into(), |d| {
        format!("{}   {} bytes", clean(&d.path), d.length)
    });
    let row = |y, height| Rect {
        y: area.y + y,
        height,
        ..area
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " HEX  ",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(title),
        ])),
        row(0, 1),
    );
    let revision = h
        .document
        .as_ref()
        .map(|d| d.revision.chars().take(12).collect::<String>())
        .unwrap_or_else(|| "pending".into());
    f.render_widget(
        Paragraph::new(format!("LIVE VIEW  ·  observed {revision}  ·  read only"))
            .style(Style::default().fg(t.text_muted)),
        row(1, 1),
    );
    let per_row = bytes_per_row(area.width);
    let headings = format!(
        "{:<18}{:<width$}ASCII",
        "OFFSET",
        "HEX BYTES",
        width = per_row * 3 + usize::from(per_row == 16) + 3
    );
    f.render_widget(
        Paragraph::new(headings).style(Style::default().fg(t.text_subtle)),
        row(2, 1),
    );
    let body = Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(10).max(1),
        ..area
    };
    f.render_widget(Paragraph::new(rows(h, t, per_row, body.height)), body);
    let inspector = row(area.height - 6, 4);
    f.render_widget(
        Paragraph::new(inspect(h)).style(Style::default().fg(t.secondary)),
        inspector,
    );
    let status = if let Some((kind, input)) = &h.input {
        format!(
            "{} › {}  [Enter apply · Esc cancel]",
            kind.label(),
            clean(input)
        )
    } else if let Some(error) = &h.error {
        clean(error)
    } else {
        format!(
            "offset 0x{:X} / {}   bookmarks {}{}",
            h.cursor,
            h.cursor,
            h.bookmarks.len(),
            if h.loading { "   loading…" } else { "" }
        )
    };
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(if h.error.is_some() {
            t.error
        } else {
            t.text_muted
        })),
        row(area.height - 2, 1),
    );
    let hints = [
        ("hex_close", "close"),
        ("hex_goto", "goto"),
        ("hex_search", "find"),
        ("hex_endian", "LE/BE"),
        ("hex_bookmark", "bookmark"),
        ("hex_search_next", "next match"),
        ("hex_copy", "copy byte"),
    ]
    .iter()
    .filter_map(|(action, label)| {
        app.keymap
            .label("hex", action)
            .map(|key| format!("{key} {label}"))
    })
    .collect::<Vec<_>>()
    .join(" · ");
    f.render_widget(
        Paragraph::new(hints).style(Style::default().fg(t.text_subtle)),
        row(area.height - 1, 1),
    );
}

pub fn bytes_per_row(width: u16) -> usize {
    if width >= 94 {
        16
    } else if width >= 56 {
        8
    } else {
        4
    }
}

fn rows(h: &Workspace, t: &Tokens, count: usize, height: u16) -> Vec<Line<'static>> {
    let cursor_row = h.cursor / count as u64;
    let first_row = cursor_row.saturating_sub(u64::from(height) / 2);
    let first = (first_row * count as u64).max(h.page_offset);
    let mut result = Vec::new();
    for row in 0..height {
        let Some(offset) = first.checked_add(u64::from(row) * count as u64) else {
            break;
        };
        let Some(local) = offset
            .checked_sub(h.page_offset)
            .and_then(|at| usize::try_from(at).ok())
        else {
            break;
        };
        let Some(bytes) = h
            .bytes
            .get(local..h.bytes.len().min(local.saturating_add(count)))
        else {
            break;
        };
        if bytes.is_empty() {
            break;
        }
        let mut spans = vec![Span::styled(
            format!("{offset:016X}  "),
            Style::default().fg(t.text_subtle),
        )];
        for (i, byte) in bytes.iter().enumerate() {
            if i == 8 {
                spans.push(Span::raw(" "));
            }
            let selected = offset + i as u64 == h.cursor;
            let style = if selected {
                t.selected()
            } else {
                Style::default().fg(if *byte == 0 { t.text_subtle } else { t.text })
            };
            spans.push(Span::styled(
                format!("{}{byte:02X}", if selected { ">" } else { " " }),
                style,
            ));
        }
        spans.push(Span::raw("   "));
        for (i, byte) in bytes.iter().enumerate() {
            let text = if (0x20..=0x7e).contains(byte) {
                char::from(*byte)
            } else {
                '.'
            };
            let style = if offset + i as u64 == h.cursor {
                t.selected()
            } else {
                Style::default().fg(t.text_muted)
            };
            spans.push(Span::styled(text.to_string(), style));
        }
        result.push(Line::from(spans));
    }
    result
}

fn inspect(h: &Workspace) -> Vec<Line<'static>> {
    let bytes = h.byte_slice();
    let endian = if h.big_endian { "BE" } else { "LE" };
    let Some(first) = bytes.first() else {
        return vec![Line::from(format!(
            "INSPECTOR {endian} · EOF · values unavailable"
        ))];
    };
    let integer = |size: usize| -> Option<u64> {
        let range = bytes.get(..size)?;
        let mut buf = [0u8; 8];
        if h.big_endian {
            buf[8 - size..].copy_from_slice(range);
            Some(u64::from_be_bytes(buf))
        } else {
            buf[..size].copy_from_slice(range);
            Some(u64::from_le_bytes(buf))
        }
    };
    let show = |v: Option<u64>| v.map_or("unavailable".into(), |v| v.to_string());
    let signed = |v: Option<u64>, size: usize| {
        v.map_or("unavailable".into(), |v| {
            (((v << (64 - size * 8)) as i64) >> (64 - size * 8)).to_string()
        })
    };
    let float32 = integer(4).map_or("unavailable".into(), |v| {
        format!("{:e}", f32::from_bits(v as u32))
    });
    let float64 = integer(8).map_or("unavailable".into(), |v| format!("{:e}", f64::from_bits(v)));
    vec![
        Line::from(format!(
            "INSPECTOR {endian}   bits {first:08b}   u8 {first}   i8 {}",
            *first as i8
        )),
        Line::from(format!(
            "u16 {}  i16 {}  u32 {}  i32 {}",
            show(integer(2)),
            signed(integer(2), 2),
            show(integer(4)),
            signed(integer(4), 4)
        )),
        Line::from(format!(
            "u64 {}  i64 {}",
            show(integer(8)),
            signed(integer(8), 8)
        )),
        Line::from(format!("f32 {float32}  f64 {float64}")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspector_never_guesses_missing_bytes_and_explicitly_changes_endianness() {
        let mut h = Workspace {
            bytes: vec![1, 2, 3, 4],
            ..Default::default()
        };
        let text = |h: &Workspace| {
            inspect(h)
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(text(&h).contains("u16 513"));
        assert!(text(&h).contains("u64 unavailable"));
        h.big_endian = true;
        assert!(text(&h).contains("u16 258"));
        h.cursor = 4;
        assert!(text(&h).contains("EOF"));
    }
}
