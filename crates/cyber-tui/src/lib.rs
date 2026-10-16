//! The terminal UI (`tui`). It talks to the server only through the public API.

mod admissions;
mod app;
mod children;
mod composer;
mod explorer;
mod fuzzy;
mod highlight;
mod hook_definitions;
mod hooks;
mod keys;
mod markdown;
mod memory;
mod model;
mod osc;
mod palette;
mod perform;
mod prefs;
mod recap;
mod review;
mod runner;
mod settings;
mod sidebar;
mod store;
mod switcher;
mod theme;
mod turns;
mod view;

#[cfg(test)]
mod screens;
#[cfg(test)]
mod screens_a11y;
#[cfg(test)]
mod screens_chrome;
#[cfg(test)]
mod screens_explorer;
#[cfg(test)]
mod screens_markdown;
#[cfg(test)]
mod screens_recap;
#[cfg(test)]
mod screens_settings;
#[cfg(test)]
mod screens_sidebar;
#[cfg(test)]
mod screens_stats;
#[cfg(test)]
mod screens_themes;
#[cfg(test)]
mod screens_usage;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

pub use runner::run;

/// Which Session to open.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    New,
    /// `--continue`: the most recent root Session of the Location.
    Last,
    /// `--resume <id|name>`.
    Resume(String),
    /// `--resume` without a value: open the picker.
    Pick,
}

pub struct TuiOptions {
    pub client: cyber_client::Client,
    pub directory: String,
    /// `~/.local/state/cyber`: history, drafts, preferences.
    pub state_dir: PathBuf,
    /// The global config directory: `tui.jsonc` and `themes/*.json`.
    pub config_dir: PathBuf,
    pub start: Start,
    pub fork: bool,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub mode: Option<String>,
    /// Submitted once the UI is up.
    pub prompt: Option<String>,
}

/// Printed after the TUI exits.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub session_id: String,
    pub title: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
}
