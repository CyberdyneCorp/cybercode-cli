//! The theme switcher (`tui` → Theme switcher): built-in and custom themes grouped as
//! `DARK` and `LIGHT` by their background, filtered and fuzzy-searched, applied live as
//! the selection moves. Enter keeps the theme in `kv.json`, `p` writes it to the
//! project's `.cyber/tui.jsonc`, and Esc restores the theme the switcher opened with.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

use crate::app::{Action, App, Overlay};
use crate::prefs::Origin;
use crate::theme::{Depth, color};

/// How long `theme_cycle` announces the new theme in the status footer.
pub const ANNOUNCE_SECS: u64 = 2;

/// `All · Dark · Light`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Dark,
    Light,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Dark, Filter::Light];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::All => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::All,
        }
    }

    fn admits(self, light: bool) -> bool {
        match self {
            Self::All => true,
            Self::Dark => !light,
            Self::Light => light,
        }
    }
}

/// One theme of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    /// Grouped under `LIGHT`: the resolved background is light.
    pub light: bool,
    pub custom: bool,
    pub background: Color,
    /// `accent`, `secondary`, `success`, `warning` and `error`.
    pub swatch: [Color; 5],
}

/// The open switcher.
#[derive(Debug, Clone, PartialEq)]
pub struct Switcher {
    /// The theme active when it opened, restored by Esc.
    pub original: String,
    pub query: String,
    /// Every theme: the `DARK` group, then `LIGHT`, each in catalog order.
    pub rows: Vec<Row>,
    /// Index into [`Switcher::visible`].
    pub selected: usize,
    /// Opened from the Config tab, which Esc and Enter return to.
    pub from_settings: bool,
}

impl Switcher {
    /// Indices of the rows the filter and the search admit, in display order.
    pub fn visible(&self, filter: Filter) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| filter.admits(r.light))
            .filter(|(_, r)| crate::fuzzy::score(&self.query, &r.name).is_some())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn current(&self, filter: Filter) -> Option<&Row> {
        let visible = self.visible(filter);
        visible.get(self.selected).map(|&i| &self.rows[i])
    }

    /// Select `name` when it is visible, else the first visible row.
    fn select(&mut self, filter: Filter, name: &str) {
        let visible = self.visible(filter);
        self.selected = visible
            .iter()
            .position(|&i| self.rows[i].name == name)
            .unwrap_or(0);
    }
}

/// Whether a background reads as light: closer in contrast to black than to white.
fn light_background(background: Color, fallback: bool) -> bool {
    match color::luminance(background) {
        Some(l) => (l + 0.05) / 0.05 > 1.05 / (l + 0.05),
        None => fallback,
    }
}

impl App {
    /// Every theme as a row, resolved for this terminal's background.
    fn theme_rows(&self) -> Vec<Row> {
        let light = self.appearance.light();
        let mut rows: Vec<Row> = self
            .themes
            .names()
            .map(|name| {
                let doc = self.themes.get(name);
                let full = doc.resolve(light, Depth::TrueColor);
                let t = doc.resolve(light, self.appearance.depth);
                Row {
                    name: name.to_owned(),
                    light: light_background(full.background, light),
                    custom: doc.custom,
                    background: t.background,
                    swatch: [t.accent, t.secondary, t.success, t.warning, t.error],
                }
            })
            .collect();
        rows.sort_by_key(|r| r.light);
        rows
    }

    /// `/theme`, the palette's `Theme…` and the Config tab's theme row.
    pub(crate) fn open_theme_switcher(&mut self, from_settings: bool) {
        self.completion = None;
        let mut switcher = Switcher {
            original: self.theme.name.clone(),
            query: String::new(),
            rows: self.theme_rows(),
            selected: 0,
            from_settings,
        };
        if !switcher
            .visible(self.theme_filter)
            .iter()
            .any(|&i| switcher.rows[i].name == self.theme.name)
        {
            self.theme_filter = Filter::All;
        }
        switcher.select(self.theme_filter, &self.theme.name);
        self.overlay = Overlay::Themes(Box::new(switcher));
    }

    fn close_switcher(&mut self, from_settings: bool) {
        self.overlay = if from_settings {
            Overlay::Settings
        } else {
            Overlay::None
        };
    }

    /// Show the highlighted theme across the whole TUI.
    fn preview_selected(&mut self) {
        let Overlay::Themes(s) = &self.overlay else {
            return;
        };
        if let Some(name) = s.current(self.theme_filter).map(|r| r.name.clone()) {
            self.set_theme(&name);
        }
    }

    pub(crate) fn switcher_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let filter = self.theme_filter;
        let Overlay::Themes(s) = &mut self.overlay else {
            return Vec::new();
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let (original, from_settings) = (s.original.clone(), s.from_settings);
                self.set_theme(&original);
                self.close_switcher(from_settings);
                return Vec::new();
            }
            KeyCode::Enter => return self.keep_theme(false),
            KeyCode::Char('p') if s.query.is_empty() && !ctrl => return self.keep_theme(true),
            KeyCode::Up => s.selected = s.selected.saturating_sub(1),
            KeyCode::Down => {
                let last = s.visible(filter).len().saturating_sub(1);
                s.selected = (s.selected + 1).min(last);
            }
            KeyCode::Tab => {
                self.theme_filter = filter.next();
                let name = self.theme.name.clone();
                s.select(self.theme_filter, &name);
            }
            KeyCode::Backspace => {
                s.query.pop();
                let name = self.theme.name.clone();
                s.select(filter, &name);
            }
            KeyCode::Char(c) if !ctrl => {
                s.query.push(c);
                s.selected = 0;
            }
            _ => return Vec::new(),
        }
        self.preview_selected();
        Vec::new()
    }

    /// Enter keeps the highlighted theme in `kv.json`; `p` writes it to the project.
    fn keep_theme(&mut self, project: bool) -> Vec<Action> {
        let Overlay::Themes(s) = &self.overlay else {
            return Vec::new();
        };
        let from_settings = s.from_settings;
        let Some(name) = s.current(self.theme_filter).map(|r| r.name.clone()) else {
            return Vec::new();
        };
        self.set_theme(&name);
        self.close_switcher(from_settings);
        if project {
            self.prefs.set("theme", &name, Origin::Project);
            vec![Action::SaveProjectTheme(name)]
        } else {
            self.prefs.set("theme", &name, Origin::Saved);
            vec![Action::SaveTheme(name)]
        }
    }

    /// `theme_cycle`: the next theme in the switcher's filter, kept and announced.
    pub(crate) fn cycle_theme(&mut self) -> Vec<Action> {
        let rows: Vec<Row> = self
            .theme_rows()
            .into_iter()
            .filter(|r| self.theme_filter.admits(r.light))
            .collect();
        if rows.is_empty() {
            return Vec::new();
        }
        let next = rows
            .iter()
            .position(|r| r.name == self.theme.name)
            .map_or(0, |i| (i + 1) % rows.len());
        let name = rows[next].name.clone();
        self.set_theme(&name);
        self.prefs.set("theme", &name, Origin::Saved);
        self.announcement = Some((format!("Theme: {name}"), self.now()));
        vec![Action::SaveTheme(name)]
    }

    /// The `theme_cycle` announcement while it lasts.
    pub fn announced(&self) -> Option<&str> {
        let (text, at): &(String, Instant) = self.announcement.as_ref()?;
        (self.now().saturating_duration_since(*at).as_secs() < ANNOUNCE_SECS)
            .then_some(text.as_str())
    }
}

/// WCAG level of a contrast ratio: `AAA`, `AA`, `AA large` or `fail`.
pub fn level(ratio: f64) -> &'static str {
    match ratio {
        r if r >= 7.0 => "AAA",
        r if r >= 4.5 => "AA",
        r if r >= 3.0 => "AA large",
        _ => "fail",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_background_groups() {
        assert_eq!(level(15.0), "AAA");
        assert_eq!(level(5.0), "AA");
        assert_eq!(level(3.2), "AA large");
        assert_eq!(level(2.0), "fail");
        assert!(light_background(Color::Rgb(255, 255, 255), false));
        assert!(!light_background(Color::Rgb(11, 15, 20), true));
        assert!(
            light_background(Color::Reset, true),
            "system follows the terminal"
        );
    }

    #[test]
    fn groups_are_dark_then_light_and_the_filter_cycles() {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        app.open_theme_switcher(false);
        let Overlay::Themes(s) = &app.overlay else {
            panic!("not open")
        };
        let first_light = s.rows.iter().position(|r| r.light).unwrap();
        assert!(s.rows[..first_light].iter().all(|r| !r.light));
        assert!(s.rows[first_light..].iter().all(|r| r.light));
        for name in ["github-light", "solarized-light", "cyber-light"] {
            assert!(s.rows.iter().any(|r| r.name == name && r.light), "{name}");
        }
        assert_eq!(s.current(Filter::All).unwrap().name, "cyber");
        assert_eq!(Filter::All.next().next().next(), Filter::All);
    }
}
