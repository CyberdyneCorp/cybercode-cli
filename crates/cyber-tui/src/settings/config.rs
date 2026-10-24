//! The Config tab (`tui` → Settings Config tab): TUI preferences edited in place, each
//! with its source. A change applies at once and is saved to `kv.json`; a later edit of
//! `tui.jsonc` wins over it.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::{Action, App};
use crate::prefs::{EDITABLE, Origin};

/// What the tab says while the server has no `config` route.
pub const CORE_POINTER: &str = "Core settings: use cyber config or edit cyber.jsonc";

/// The preference's value as the UI uses it now, and where it comes from: a toggle
/// made in this run (Ctrl+T, `/timestamps`) shows as `session`.
pub fn current(app: &App, key: &str) -> (String, Origin) {
    let setting = app.prefs.settings.iter().find(|s| s.key == key);
    let (value, origin) = setting.map_or((String::new(), Origin::Default), |s| {
        (s.value.clone(), s.origin)
    });
    let on = |b: bool| if b { "on" } else { "off" }.to_owned();
    match key {
        "timestamps" if (value == "on") != app.timestamps => (on(app.timestamps), Origin::Session),
        "reasoning" if (value == "expanded") != app.show_reasoning => {
            let shown = if app.show_reasoning {
                "expanded"
            } else {
                "collapsed"
            };
            (shown.to_owned(), Origin::Session)
        }
        _ => (value, origin),
    }
}

/// The values `key` cycles through; `theme` opens the switcher instead.
fn values(key: &str) -> Vec<String> {
    EDITABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, values)| values.iter().map(|v| (*v).to_owned()).collect())
        .unwrap_or_default()
}

/// The line of the first preference row: below the heading and a blank line.
pub const FIRST_ROW: usize = 2;

/// The tab's rows: one per preference, the selected one marked.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let key_width = EDITABLE.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 2;
    let mut out = vec![
        super::status::heading(app, "TUI preferences"),
        Line::default(),
    ];
    for (index, (key, _)) in EDITABLE.iter().enumerate() {
        let (value, origin) = current(app, key);
        let selected = index == app.settings.config_row;
        let marker = if selected { "› " } else { "  " };
        let row = format!("{marker}{key:<key_width$}{value:<14}");
        let room = width.saturating_sub(origin.label().len() + 1);
        let row = crate::view::layout::truncate_end(&row, room);
        let (style, source) = if selected {
            (t.selected(), t.selected())
        } else {
            (
                Style::default().fg(t.text),
                Style::default().fg(t.text_subtle),
            )
        };
        out.push(Line::from(vec![
            Span::styled(row, style),
            Span::styled(format!(" {}", origin.label()), source),
        ]));
    }
    out.push(Line::default());
    out.push(Line::from(Span::styled(
        "Saved to kv.json; a later edit of tui.jsonc wins.",
        Style::default().fg(t.text_subtle),
    )));
    out.push(Line::from(Span::styled(
        CORE_POINTER,
        Style::default().fg(t.text_muted),
    )));
    out
}

impl App {
    pub(crate) fn config_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let last = EDITABLE.len() - 1;
        let row = &mut self.settings.config_row;
        match key.code {
            KeyCode::Up => *row = row.saturating_sub(1),
            KeyCode::Down => *row = (*row + 1).min(last),
            KeyCode::Home => *row = 0,
            KeyCode::End => *row = last,
            KeyCode::Enter | KeyCode::Char(' ')
                if EDITABLE.get(*row).is_some_and(|(key, _)| *key == "theme") =>
            {
                self.open_theme_switcher(true);
            }
            KeyCode::Enter | KeyCode::Char(' ') => return self.cycle_preference(),
            _ => {}
        }
        Vec::new()
    }

    /// Move the selected preference to its next value, apply it and save it.
    fn cycle_preference(&mut self) -> Vec<Action> {
        let Some((key, _)) = EDITABLE.get(self.settings.config_row) else {
            return Vec::new();
        };
        let values = values(key);
        let (value, _) = current(self, key);
        let next = values
            .iter()
            .position(|v| *v == value)
            .map_or(0, |i| (i + 1) % values.len().max(1));
        let Some(next) = values.get(next).cloned() else {
            return Vec::new();
        };
        self.apply_preference(key, &next);
        self.prefs.set(key, &next, Origin::Saved);
        vec![Action::SavePreference {
            key,
            value: crate::prefs::Preferences::json(key, &next),
        }]
    }

    /// Make the UI follow a preference value at once.
    pub fn apply_preference(&mut self, key: &str, value: &str) {
        match key {
            "theme" => self.set_theme(value),
            "theme_mode" => {
                self.appearance.mode = crate::theme::ThemeMode::parse(Some(value));
                self.set_theme(&self.theme.name.clone());
            }
            "color" => {
                let mut prefs = self.prefs.clone();
                prefs.set("color", value, Origin::Saved);
                self.appearance.depth = prefs.depth();
                self.set_theme(&self.theme.name.clone());
            }
            "sidebar" => {
                self.sidebar.preference =
                    crate::sidebar::Preference::parse(Some(value)).unwrap_or_default();
                self.sidebar.toggled = false;
            }
            "machine_stats" => {
                // Off hides the section at once; on asks at the next tick.
                self.sidebar.machine = crate::sidebar::Source::Pending;
                self.sidebar.machine_failed = false;
                self.sidebar.machine_due = None;
            }
            "reasoning" => self.show_reasoning = value == "expanded",
            "timestamps" => self.timestamps = value == "on",
            "accessibility.reduced_motion" => {
                self.reduced_motion = self.screen_reader || value == "on";
            }
            // `recap` is read from `prefs` when a recap could be asked for; `diff_style` is
            // saved for the view that will read it.
            _ => {}
        }
    }
}
