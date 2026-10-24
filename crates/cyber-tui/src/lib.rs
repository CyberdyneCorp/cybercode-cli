//! The terminal UI (`tui`). It talks to the server only through the public API.

mod admissions;
mod app;
mod attachments;
pub mod books;
mod branches;
mod children;
mod collaboration;
mod composer;
mod explorer;
mod export;
mod fuzzy;
mod goals;
mod highlight;
mod hook_definitions;
mod hooks;
mod import_offer;
mod keys;
mod login;
mod loops;
mod markdown;
mod memory;
mod messaging;
mod model;
mod mouse;
mod osc;
mod palette;
mod perform;
mod plugins;
mod prefs;
mod progress;
mod recap;
mod rename;
mod review;
mod runner;
mod settings;
mod side_chat;
mod sidebar;
mod store;
mod switcher;
mod team;
mod theme;
mod turns;
mod update;
mod view;
mod workflow_library;
mod workflow_watch;
mod workflows;

#[cfg(test)]
mod screens;
#[cfg(test)]
mod screens_a11y;
#[cfg(test)]
mod screens_attachments;
#[cfg(test)]
mod screens_branches;
#[cfg(test)]
mod screens_chrome;
#[cfg(test)]
mod screens_context;
#[cfg(test)]
mod screens_explorer;
#[cfg(test)]
mod screens_export;
#[cfg(test)]
mod screens_goals;
#[cfg(test)]
mod screens_indicators;
#[cfg(test)]
mod screens_machine;
#[cfg(test)]
mod screens_markdown;
#[cfg(test)]
mod screens_mcp;
#[cfg(test)]
mod screens_messages;
#[cfg(test)]
mod screens_parallel;
#[cfg(test)]
mod screens_peers;
#[cfg(test)]
mod screens_progress;
#[cfg(test)]
mod screens_recap;
#[cfg(test)]
mod screens_rename;
#[cfg(test)]
mod screens_settings;
#[cfg(test)]
mod screens_side_chat;
#[cfg(test)]
mod screens_sidebar;
#[cfg(test)]
mod screens_stats;
#[cfg(test)]
mod screens_tasks;
#[cfg(test)]
mod screens_team;
#[cfg(test)]
mod screens_themes;
#[cfg(test)]
mod screens_update;
#[cfg(test)]
mod screens_usage;
#[cfg(test)]
mod screens_workflows;
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
    /// `--from-pr <number>`: the picker over the Sessions linked to that pull request.
    PickPr(u64),
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

#[cfg(test)]
mod screens_collaboration;
