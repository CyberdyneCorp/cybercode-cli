//! The output style picker (`tui` → Output style picker): the search, one row per style
//! with its description, origin and the `current` and `default` labels, the listing's
//! issues, then the keys.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::layout::truncate_end;
use super::overlays::{centered, frame};
use crate::app::App;
use crate::output_styles::{Listing, Picker, Style as OutputStyle};

const KEYS: &str = "↑↓ select · type to search · enter switch · d save default · esc close";
const NAME: usize = 16;

pub fn draw(f: &mut Frame, app: &App, picker: &Picker, area: Rect) {
    let Some(listing) = &app.output_styles.listing else {
        return;
    };
    let t = &app.theme;
    let muted = Style::default().fg(t.text_muted);
    let rect = centered(area, 96, 20);
    let width = rect.width.saturating_sub(4) as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("› ", Style::default().fg(t.accent)),
            Span::styled(picker.query.clone(), Style::default().fg(t.text)),
            Span::styled("▏", Style::default().fg(t.accent)),
        ]),
        Line::default(),
    ];
    lines.extend(rows(app, picker, listing, width));
    if !listing.issues.is_empty() {
        lines.push(Line::default());
    }
    for issue in &listing.issues {
        lines.push(Line::from(Span::styled(
            issue.clone(),
            Style::default().fg(t.warning),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(truncate_end(KEYS, width), muted)));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, "Output style")),
        rect,
    );
}

/// `name  description  origin · current · default`, the selected row marked and
/// highlighted, the labels aligned.
fn rows(app: &App, picker: &Picker, listing: &Listing, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let visible = picker.visible(listing);
    if visible.is_empty() {
        let none = Span::styled("No matching style", Style::default().fg(t.text_muted));
        return vec![Line::from(none)];
    }
    let (current, _) = app.output_style();
    let tags: Vec<String> = visible
        .iter()
        .map(|&i| labels(&listing.styles[i], &current, &listing.effective))
        .collect();
    let tag_width = tags
        .iter()
        .map(|tag| tag.chars().count())
        .max()
        .unwrap_or(0);
    let room = width.saturating_sub(NAME + tag_width + 5);
    visible
        .iter()
        .zip(tags)
        .enumerate()
        .map(|(pos, (&i, tags))| {
            let style = &listing.styles[i];
            let selected = pos == picker.selected;
            let marker = if selected { "› " } else { "  " };
            let row = format!(
                "{marker}{:<NAME$} {:<room$} ",
                style.name,
                truncate_end(&style.description, room)
            );
            let (text, label) = if selected {
                (t.selected(), t.selected())
            } else {
                (
                    Style::default().fg(t.text),
                    Style::default()
                        .fg(t.text_subtle)
                        .add_modifier(Modifier::DIM),
                )
            };
            Line::from(vec![Span::styled(row, text), Span::styled(tags, label)])
        })
        .collect()
}

/// `built-in · current · default`: the origin, then whether the Session uses the style
/// and whether it is the configured default.
fn labels(style: &OutputStyle, current: &str, effective: &str) -> String {
    let mut tags = vec![style.origin.as_str()];
    if style.name == current {
        tags.push("current");
    }
    if style.name == effective {
        tags.push("default");
    }
    tags.join(" · ")
}
