//! The repository map view (`tui` → Repository map view): the index status header, the
//! map with the selected row highlighted, then the keys.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::layout::truncate_end;
use super::overlays::{centered, frame};
use crate::app::App;
use crate::repo_map::MapView;

const KEYS: &str = "↑↓ select · enter open in explorer · r rebuild · esc close";

pub fn draw(f: &mut Frame, app: &App, view: &MapView, area: Rect) {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let rect = centered(area, 110, area.height.saturating_sub(2));
    let width = rect.width.saturating_sub(4) as usize;
    // Header, blank, rows, blank, keys, inside the borders.
    let room = (rect.height as usize).saturating_sub(6).max(1);
    let first = view.selected.saturating_sub(room.saturating_sub(1));
    let mut lines = vec![
        Line::from(Span::styled(truncate_end(&view.header, width), muted)),
        Line::default(),
    ];
    for (index, row) in view.rows.iter().enumerate().skip(first).take(room) {
        let style = if index == view.selected {
            t.selected()
        } else if row.path.is_some() {
            Style::default().fg(t.accent)
        } else {
            Style::default().fg(t.text)
        };
        lines.push(Line::from(Span::styled(
            truncate_end(&row.text, width),
            style,
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(truncate_end(KEYS, width), muted)));
    let title = match &view.focus {
        Some(focus) => format!("Repository map · {focus}"),
        None => "Repository map".into(),
    };
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(frame(app, &title)), rect);
}
