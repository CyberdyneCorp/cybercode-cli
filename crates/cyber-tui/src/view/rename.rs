//! The `/rename` dialog.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::overlays::{centered, frame};
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App, title: &str, area: Rect) {
    let rect = centered(area, 72, 5);
    let t = &app.theme;
    let width = rect.width.saturating_sub(4) as usize;
    let shown: String = crate::sidebar::clean(title);
    // Keep the end of a long title, where the cursor is, in view.
    let skip = shown
        .chars()
        .count()
        .saturating_sub(width.saturating_sub(1));
    let visible: String = shown.chars().skip(skip).collect();
    let hint = "Enter save · Esc cancel · Ctrl+U clear";
    let lines = vec![
        Line::from(vec![
            Span::styled(visible, Style::default().fg(t.text)),
            Span::styled("▏", Style::default().fg(t.accent)),
        ]),
        Line::default(),
        Line::from(Span::styled(hint, Style::default().fg(t.text_muted))),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(frame(app, "Rename session")),
        rect,
    );
}
