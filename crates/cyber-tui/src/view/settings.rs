//! The Settings panel (`tui` → Settings panel): the tab bar, the current tab's rows in
//! a scrollable paragraph, and the tab's keys on the bottom row.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::App;
use crate::mouse::{List, Target};
use crate::settings::Tab;

/// Columns of padding on the left.
const PAD: u16 = 2;
const TITLE: &str = "Settings";

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().fg(t.text).bg(t.background)),
        area,
    );
    if area.width <= PAD * 2 || area.height < 4 {
        return;
    }
    let inner = Rect {
        x: area.x + PAD,
        y: area.y + 1,
        width: area.width - PAD * 2,
        height: area.height - 1,
    };
    f.render_widget(Paragraph::new(tab_bar(app)), Rect { height: 1, ..inner });
    push_tabs(app, Rect { height: 1, ..inner });
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let lines = lines(app, body.width as usize);
    let max = (lines.len() as u16).saturating_sub(body.height);
    app.settings.set_max_scroll(max);
    if app.settings.tab == Tab::Config {
        let skip = app.settings.scroll.min(max) as usize;
        for row in 0..crate::prefs::EDITABLE.len() {
            let line = crate::settings::config::FIRST_ROW + row;
            super::push_lines(
                app,
                body,
                line..line + 1,
                skip,
                Target::row(List::Config, row),
            );
        }
    }
    f.render_widget(
        Paragraph::new(lines).scroll((app.settings.scroll.min(max), 0)),
        body,
    );
    let keys = crate::view::layout::truncate_end(&app.settings_keys(), inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(keys, Style::default().fg(t.text_muted))),
        Rect {
            y: inner.bottom().saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

/// Each tab of the bar is a target that presses its number key.
fn push_tabs(app: &App, bar: Rect) {
    let mut x = bar.x.saturating_add(TITLE.len() as u16);
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        let width = tab.label().len() as u16 + 2;
        x = x.saturating_add(2);
        let rect = Rect { x, width, ..bar };
        let key = KeyEvent::new(KeyCode::Char((b'1' + i as u8) as char), KeyModifiers::NONE);
        app.mouse.push(rect.intersection(bar), Target::Key(key));
        x = x.saturating_add(width);
    }
}

/// `Settings  Status  Config  Usage  Stats`, the active tab in the selection style.
fn tab_bar(app: &App) -> Line<'static> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        TITLE,
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )];
    for tab in Tab::ALL {
        spans.push(Span::raw("  "));
        let style = if tab == app.settings.tab {
            t.selected()
        } else {
            Style::default().fg(t.text)
        };
        spans.push(Span::styled(format!(" {} ", tab.label()), style));
    }
    Line::from(spans)
}

/// The current tab's rows.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    match app.settings.tab {
        Tab::Status => crate::settings::status::lines(app, width),
        Tab::Config => crate::settings::config::lines(app, width),
        Tab::Usage => crate::settings::usage::lines(app, width),
        Tab::Stats => crate::settings::stats::lines(app, width),
    }
}
