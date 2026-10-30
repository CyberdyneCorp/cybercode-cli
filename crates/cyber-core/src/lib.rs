//! Shared foundations of Cyber Code: build information, XDG paths, identifiers,
//! configuration loading with workspace trust, and evaluation manifests.

pub mod agent_mentions;
pub mod browser;
pub mod budget;
pub mod commands;
pub mod config;
pub mod env;
pub mod eval;
pub mod features;
pub mod files;
pub mod goals;
pub mod hooks;
pub mod ids;
pub mod import;
pub mod intelligence;
pub mod log;
pub mod loops;
pub mod memory;
pub mod output_styles;
pub mod paths;
pub mod plugins;
pub mod project;
pub mod prompts;
pub mod review;
pub mod skills;
pub mod teams;
pub mod trust;
pub mod untrusted;
pub mod usage_limit;
pub mod vcs;
pub mod version;
pub mod wildcard;
pub mod worktrees;
