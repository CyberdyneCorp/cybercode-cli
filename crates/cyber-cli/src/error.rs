//! Error format and exit codes (`cli-commands` → Error format and exit codes).

use serde_json::json;

use crate::cli::Format;

pub const EXIT_RUNTIME: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

#[derive(Debug)]
pub struct CliError {
    pub code: u8,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            message: message.into(),
            hint: None,
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_RUNTIME,
            message: message.into(),
            hint: None,
        }
    }

    /// A command or value whose owning milestone has not shipped.
    pub fn unavailable(what: &str, milestone: &str) -> Self {
        Self::usage(format!("{what} is not available in this build yet"))
            .with_hint(format!("planned for milestone {milestone}; see ROADMAP.md"))
    }

    /// Exit with `code` without printing anything (the command already reported).
    pub fn silent(code: u8) -> Self {
        Self {
            code,
            message: String::new(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn print(&self, format: Option<Format>) {
        if self.message.is_empty() {
            return;
        }
        if format == Some(Format::Json) {
            let body = json!({ "error": { "code": self.code, "message": self.message, "hint": self.hint } });
            eprintln!("{body}");
            return;
        }
        eprintln!("Error: {}", self.message);
        if let Some(hint) = &self.hint {
            eprintln!("Hint: {hint}");
        }
    }
}

impl From<cyber_core::config::ConfigError> for CliError {
    fn from(e: cyber_core::config::ConfigError) -> Self {
        use cyber_core::config::ConfigError;
        match e {
            ConfigError::Io { .. } => Self::runtime(e.to_string()),
            _ => Self::usage(e.to_string()),
        }
    }
}

impl From<cyber_store::StoreError> for CliError {
    fn from(e: cyber_store::StoreError) -> Self {
        Self::runtime(e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::runtime(e.to_string())
    }
}
