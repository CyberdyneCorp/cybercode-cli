//! Wrapping styled lines to a width, so scrolling can count rows exactly.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Split `line` into rows of at most `width` columns, keeping span styles.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        let mut buf = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                if !buf.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut buf), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut current)).style(line.style));
                used = 0;
            }
            buf.push(c);
            used += w;
        }
        if !buf.is_empty() {
            current.push(Span::styled(buf, span.style));
        }
    }
    rows.push(Line::from(current).style(line.style));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_wrap_and_short_ones_do_not() {
        let rows = wrap(Line::from("abcdefghij"), 4);
        let text: Vec<String> = rows.iter().map(|l| l.to_string()).collect();
        assert_eq!(text, vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap(Line::from("ab"), 4).len(), 1);
        assert_eq!(wrap(Line::from(""), 4).len(), 1);
    }
}
