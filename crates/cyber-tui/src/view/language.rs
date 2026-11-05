//! The language picker (`localization` → Language selection): one row per choice, each
//! language in its own name, the current one marked, then what a choice changes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::overlays::{centered, frame};
use crate::app::App;
use crate::language::{CHOICES, Picker, name};

pub fn draw(f: &mut Frame, app: &App, picker: &Picker, area: Rect) {
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let rect = centered(area, 70, CHOICES.len() as u16 + 7);
    let current = app.language_value();
    let mut lines = Vec::new();
    for (index, (tag, _)) in CHOICES.iter().enumerate() {
        let marker = if index == picker.selected {
            "› "
        } else {
            "  "
        };
        let mut text = format!("{marker}{:<30}", name(app, tag));
        if *tag == current {
            text.push_str(app.tr("current"));
        }
        let style = if index == picker.selected {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        app.tr("Saved as your default; the model replies in it from its next turn."),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        app.tr("↑↓ select · enter apply · esc close"),
        muted,
    )));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, app.tr("Language"))),
        rect,
    );
}
