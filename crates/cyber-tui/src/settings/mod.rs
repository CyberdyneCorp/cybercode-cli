//! The Settings panel (`tui` → Settings panel): Status, Config, Usage and Stats tabs
//! over the transcript. Each tab loads when it is shown; a reply for an older load or
//! another Session is dropped.

pub mod config;
pub mod cost;
pub mod heatmap;
pub mod insights;
pub mod limits;
pub mod stats;
pub mod status;
pub mod usage;

use std::sync::atomic::{AtomicU16, Ordering};

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{Action, App, Overlay};

/// The tabs, in tab-bar order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Status,
    Config,
    Usage,
    Stats,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Status, Tab::Config, Tab::Usage, Tab::Stats];

    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Config => "Config",
            Self::Usage => "Usage",
            Self::Stats => "Stats",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn step(self, forward: bool) -> Self {
        let n = Self::ALL.len();
        let i = self.index();
        Self::ALL[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }]
    }
}

/// One tab's data: the last value, whether a load is running and its error. The
/// generation drops replies to older loads.
#[derive(Debug, Clone, PartialEq)]
pub struct Loadable<T> {
    pub data: Option<T>,
    pub loading: bool,
    pub error: Option<String>,
    generation: u64,
}

impl<T> Default for Loadable<T> {
    fn default() -> Self {
        Self {
            data: None,
            loading: false,
            error: None,
            generation: 0,
        }
    }
}

impl<T> Loadable<T> {
    /// Start a load; its reply must carry the returned generation.
    pub fn start(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        self.error = None;
        self.generation
    }

    pub fn apply(&mut self, generation: u64, result: Result<T, String>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(data) => self.data = Some(data),
            Err(error) => self.error = Some(error),
        }
    }

    pub fn forget(&mut self) {
        *self = Self {
            generation: self.generation,
            ..Self::default()
        };
    }
}

#[derive(Debug, Default)]
pub struct Settings {
    pub tab: Tab,
    /// Lines scrolled from the top of the tab.
    pub scroll: u16,
    /// The furthest the last frame could scroll.
    pub(crate) max_scroll: AtomicU16,
    /// The Status tab's sources.
    pub status: Loadable<status::Status>,
    /// The selected Config row.
    pub config_row: usize,
    /// The Stats tab's period, subtab, scope and data.
    pub stats: stats::StatsState,
    /// The Usage tab's contributing factors.
    pub insights: insights::InsightsState,
    /// The Usage tab's provider-reported windows.
    pub limits: limits::LimitsState,
}

impl Settings {
    fn clamp(&self, scroll: u16) -> u16 {
        scroll.min(self.max_scroll.load(Ordering::Relaxed))
    }

    pub(crate) fn set_max_scroll(&self, max: u16) {
        self.max_scroll.store(max, Ordering::Relaxed);
    }
}

impl App {
    /// Open the panel on `tab`, or on the last used tab.
    pub fn open_settings(&mut self, tab: Option<Tab>) -> Vec<Action> {
        self.completion = None;
        self.overlay = Overlay::Settings;
        self.show_tab(tab.unwrap_or(self.settings.tab))
    }

    fn show_tab(&mut self, tab: Tab) -> Vec<Action> {
        self.settings.tab = tab;
        self.settings.scroll = 0;
        self.load_tab()
    }

    /// The current tab's data, read again.
    fn load_tab(&mut self) -> Vec<Action> {
        match self.settings.tab {
            Tab::Status => vec![Action::LoadStatus {
                generation: self.settings.status.start(),
            }],
            Tab::Usage => {
                let mut actions = self.load_cost();
                actions.extend(self.load_limits());
                actions.extend(self.load_insights());
                actions
            }
            Tab::Stats => self.load_stats(),
            Tab::Config => Vec::new(),
        }
    }

    /// The Session changed: its Status data no longer applies.
    pub(crate) fn forget_settings(&mut self) {
        self.settings.status.forget();
    }

    pub(crate) fn settings_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match self.keymap.action("settings", &key, false) {
            Some("settings_close") => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            Some("settings_next") => return self.show_tab(self.settings.tab.step(true)),
            Some("settings_previous") => return self.show_tab(self.settings.tab.step(false)),
            Some("settings_refresh") => return self.load_tab(),
            _ => {}
        }
        match key.code {
            KeyCode::Right => self.show_tab(self.settings.tab.step(true)),
            KeyCode::Left => self.show_tab(self.settings.tab.step(false)),
            KeyCode::Char(c @ '1'..='4') => self.show_tab(Tab::ALL[c as usize - '1' as usize]),
            // The `/cost` overlay's refresh key, kept for one release.
            KeyCode::Char('R') if self.settings.tab == Tab::Usage => self.load_tab(),
            KeyCode::Char(c @ ('d' | 'w')) if self.settings.tab == Tab::Usage => {
                self.insights_key(c).unwrap_or_default()
            }
            _ if self.settings.tab == Tab::Config => self.config_key(key),
            _ if self.settings.tab == Tab::Stats => self.stats_key(key),
            _ => {
                self.scroll_key(key.code);
                Vec::new()
            }
        }
    }

    fn scroll_key(&mut self, code: KeyCode) {
        let s = &mut self.settings;
        let scroll = match code {
            KeyCode::Up => s.scroll.saturating_sub(1),
            KeyCode::Down => s.scroll.saturating_add(1),
            KeyCode::PageUp => s.scroll.saturating_sub(10),
            KeyCode::PageDown => s.scroll.saturating_add(10),
            KeyCode::Home => 0,
            KeyCode::End => u16::MAX,
            _ => return,
        };
        s.scroll = s.clamp(scroll);
    }

    /// The bottom row: the current tab's keys, from the binding table.
    pub fn settings_keys(&self) -> String {
        let mut keys = Vec::new();
        let mut push = |key: Option<String>, what: &str| {
            if let Some(key) = key {
                keys.push(format!("{key} {what}"));
            }
        };
        if self.settings.tab == Tab::Config {
            push(Some("↑↓".into()), "select");
            push(Some("enter".into()), "change");
        } else {
            push(Some("↑↓".into()), "scroll");
        }
        if self.settings.tab == Tab::Usage {
            push(Some("d".into()), "day");
            push(Some("w".into()), "week");
        }
        if self.settings.tab == Tab::Stats {
            push(Some("p".into()), "period");
            push(Some("v".into()), "view");
            push(Some("s".into()), "scope");
        }
        if matches!(self.settings.tab, Tab::Status | Tab::Usage | Tab::Stats) {
            push(self.keymap.label("settings", "settings_refresh"), "refresh");
        }
        push(self.keymap.label("settings", "settings_next"), "next tab");
        push(Some("1-4".into()), "jump");
        push(self.keymap.label("settings", "settings_close"), "close");
        keys.join(" · ")
    }
}
