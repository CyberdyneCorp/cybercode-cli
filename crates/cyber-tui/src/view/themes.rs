//! The theme switcher overlay (`tui` → Theme switcher): the theme list on the left, a
//! preview drawn with the highlighted theme's tokens on the right, and the keys below.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::switcher::{Filter, Row, Switcher, level};
use crate::theme::{Depth, color};

/// The list pane's width, borders included.
const LIST: u16 = 40;
const NAME: usize = 16;
const KEYS: &str = "↑↓ select · tab filter · type to search · enter keep · p project · esc revert";
const MARKDOWN: &str = "## Heading\n\nA paragraph with `inline code` and [a link](https://example.com).\n\n- a list item\n- another item\n\n```rust\nfn main() {\n    let answer = 42; // the answer\n    println!(\"{answer}\");\n}\n```\n";

pub fn draw(f: &mut Frame, app: &App, s: &Switcher, area: Rect) {
    let t = &app.theme;
    let rect = super::overlays::centered(area, 124, 36);
    f.render_widget(Clear, rect);
    let block = super::overlays::frame(app, "Theme");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < LIST + 10 {
        return;
    }
    let [body, keys] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let [list, preview] =
        Layout::horizontal([Constraint::Length(LIST), Constraint::Min(1)]).areas(body);
    f.render_widget(
        Paragraph::new(list_lines(app, s, list.height as usize)).block(
            Block::default()
                .borders(Borders::RIGHT)
                .padding(ratatui::widgets::Padding::left(1))
                .border_style(Style::default().fg(t.border)),
        ),
        list,
    );
    let preview = Rect {
        x: preview.x + 2,
        width: preview.width.saturating_sub(3),
        ..preview
    };
    f.render_widget(
        Paragraph::new(preview_lines(app, preview.width as usize))
            .wrap(ratatui::widgets::Wrap { trim: false }),
        preview,
    );
    let keys_text = crate::view::layout::truncate_end(KEYS, keys.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(keys_text, Style::default().fg(t.text_muted))),
        keys,
    );
}

/// `All · Dark · Light`, the search, then the `DARK` and `LIGHT` groups, scrolled so the
/// selected row is visible.
fn list_lines(app: &App, s: &Switcher, height: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut head = vec![filters(app), search(app, s), Line::default()];
    let visible = s.visible(app.theme_filter);
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut selected_line = 0;
    let mut group: Option<bool> = None;
    for (pos, &i) in visible.iter().enumerate() {
        let row = &s.rows[i];
        if group != Some(row.light) {
            if group.is_some() {
                rows.push(Line::default());
            }
            let label = if row.light { "LIGHT" } else { "DARK" };
            rows.push(Line::from(Span::styled(
                label,
                Style::default()
                    .fg(t.text_muted)
                    .add_modifier(Modifier::BOLD),
            )));
            group = Some(row.light);
        }
        if pos == s.selected {
            selected_line = rows.len();
        }
        rows.push(row_line(app, s, row, pos == s.selected));
    }
    if visible.is_empty() {
        rows.push(Line::from(Span::styled(
            "no matches",
            Style::default().fg(t.text_muted),
        )));
    }
    let room = height.saturating_sub(head.len()).max(1);
    let first = selected_line.saturating_sub(room - 1);
    head.extend(rows.into_iter().skip(first).take(room));
    head
}

fn filters(app: &App) -> Line<'static> {
    let t = &app.theme;
    let mut spans = Vec::new();
    for (i, filter) in Filter::ALL.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(t.text_muted)));
        }
        let style = if filter == app.theme_filter {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        spans.push(Span::styled(format!(" {} ", filter.label()), style));
    }
    Line::from(spans)
}

fn search(app: &App, s: &Switcher) -> Line<'static> {
    let t = &app.theme;
    let query = if s.query.is_empty() {
        Span::styled("type to search", Style::default().fg(t.text_subtle))
    } else {
        Span::styled(crate::sidebar::clean(&s.query), Style::default().fg(t.text))
    };
    Line::from(vec![
        Span::styled("› ", Style::default().fg(t.accent)),
        query,
    ])
}

/// `❯ name  ● ● ● ● ●  current custom`, the swatch on the theme's own background.
fn row_line(app: &App, s: &Switcher, row: &Row, selected: bool) -> Line<'static> {
    let t = &app.theme;
    let name = crate::view::layout::truncate_end(&row.name, NAME);
    let (marker, style) = if selected {
        ("❯ ", t.selected())
    } else {
        ("  ", Style::default().fg(t.text))
    };
    let mut spans = vec![Span::styled(format!("{marker}{name:<NAME$}"), style)];
    if !t.monochrome {
        let on = Style::default().bg(row.background);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(" ", on));
        for c in row.swatch {
            spans.push(Span::styled("●", on.fg(c)));
            spans.push(Span::styled(" ", on));
        }
    }
    if row.name == s.original {
        spans.push(Span::styled(" current", Style::default().fg(t.accent)));
    }
    if row.custom {
        spans.push(Span::styled(" custom", Style::default().fg(t.text_muted)));
    }
    Line::from(spans)
}

/// The preview, in the tokens of the highlighted theme (the one now applied).
fn preview_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let bold = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let mut out = vec![Line::from(vec![
        Span::styled("Preview · ", Style::default().fg(t.text_muted)),
        Span::styled(t.name.clone(), bold),
    ])];
    if t.monochrome {
        out.push(Line::default());
        for text in [
            "Colors are disabled (NO_COLOR or tui.color none).",
            "The chosen theme applies when colors are enabled again.",
        ] {
            out.push(Line::from(Span::styled(text, Style::default().fg(t.text))));
        }
        return out;
    }
    out.push(Line::default());
    out.extend(crate::view::welcome::preview_logo(app));
    out.push(Line::default());
    let ctx = crate::markdown::Ctx::new(t, width, false);
    out.extend(app.markdown.message(MARKDOWN, &ctx).lines.iter().cloned());
    out.push(Line::default());
    out.push(Line::from(Span::styled(
        format!("{:<width$}", "+ an added line"),
        Style::default().fg(t.diff_added).bg(t.diff_added_bg),
    )));
    out.push(Line::from(Span::styled(
        format!("{:<width$}", "- a removed line"),
        Style::default().fg(t.diff_removed).bg(t.diff_removed_bg),
    )));
    out.push(Line::default());
    out.push(Line::from(vec![
        Span::styled("✓ success", Style::default().fg(t.success)),
        Span::raw("   "),
        Span::styled("⚠ warning", Style::default().fg(t.warning)),
        Span::raw("   "),
        Span::styled("✗ error", Style::default().fg(t.error)),
    ]));
    out.push(Line::default());
    out.push(Line::from(vec![
        Span::styled("▣ ", Style::default().fg(t.accent)),
        Span::styled("Build · main · 11.3s", Style::default().fg(t.text_muted)),
    ]));
    out.push(Line::default());
    out.push(contrast(app));
    out
}

/// `Contrast: text 15.3:1 AAA · muted 5.2:1 AA`, from the theme's 24-bit colors, and
/// `low contrast` in `warning` when body text is below 4.5:1.
fn contrast(app: &App) -> Line<'static> {
    let t = &app.theme;
    let full = app.themes.get(&t.name).resolve(t.light, Depth::TrueColor);
    let muted = Style::default().fg(t.text_muted);
    let (Some(text), Some(subtle)) = (
        color::contrast(full.text, full.background),
        color::contrast(full.text_muted, full.background),
    ) else {
        return Line::from(Span::styled(
            "Contrast: set by the terminal's own colors",
            muted,
        ));
    };
    let mut spans = vec![Span::styled(
        format!(
            "Contrast: text {text:.1}:1 {} · muted {subtle:.1}:1 {}",
            level(text),
            level(subtle)
        ),
        muted,
    )];
    if text < 4.5 {
        spans.push(Span::styled(
            " · low contrast",
            Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}
