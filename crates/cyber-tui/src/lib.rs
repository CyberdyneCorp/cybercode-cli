//! The terminal UI (`tui`). It talks to the server only through the public API.

mod admissions;
mod app;
mod children;
mod composer;
mod cost;
mod fuzzy;
mod hook_definitions;
mod hooks;
mod model;
mod perform;
mod runner;
mod store;
mod theme;
mod view;

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
