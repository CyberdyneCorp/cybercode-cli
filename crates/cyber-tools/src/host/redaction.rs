//! Secrets hidden from what the model sees (`tool-registry` → Tool output redaction):
//! the Location's redactor, what it hid from each call before truncation, and the
//! refusal of writes that would store a redaction marker.

use std::path::Path;
use std::sync::{Arc, PoisonError};

use cyber_core::redact::{self, RedactSettings, Redactions, Redactor};
use serde_json::Value;

use super::BuiltinHost;
use crate::tools::{ToolError, failed};

/// The last redactor built, with what it was built from.
#[derive(Default)]
pub(crate) struct RedactorCache(std::sync::Mutex<Option<Built>>);

struct Built {
    settings: RedactSettings,
    known: Vec<String>,
    redactor: Arc<Redactor>,
}

impl BuiltinHost {
    /// The redactor for a resolved config: its `tool_output.redact` settings (the
    /// defaults when unreadable, so a broken config still redacts) and the exact values
    /// of known secrets: secret-named environment variables, secret config entries
    /// (provider keys, MCP environment and headers) and resolved provider keys.
    pub(crate) fn redactor(&self, config: &Value) -> Arc<Redactor> {
        let settings = RedactSettings::from_config(config).unwrap_or_default();
        let mut known = redact::env_secrets(self.opts.env.vars());
        known.extend(redact::config_secrets(config));
        if let Some(models) = &self.opts.models {
            known.extend(models.credential_values());
        }
        known.sort();
        known.dedup();
        let mut cache = self
            .redactors
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(built) = cache.as_ref()
            && built.settings == settings
            && built.known == known
        {
            return Arc::clone(&built.redactor);
        }
        let redactor = Arc::new(Redactor::new(&settings, known.iter().cloned()));
        *cache = Some(Built {
            settings,
            known,
            redactor: Arc::clone(&redactor),
        });
        redactor
    }

    pub(crate) fn location_redactor(&self, location: &Path) -> Arc<Redactor> {
        self.redactor(&self.config_for(location))
    }

    /// Add what the projection hid from a call's raw result to its settlement.
    pub(crate) fn note_redactions(&self, session_id: &str, call_id: &str, found: Redactions) {
        if found.is_empty() {
            return;
        }
        self.call_redactions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((session_id.into(), call_id.into()))
            .or_default()
            .merge(&found);
    }

    pub(crate) fn take_redactions(&self, session_id: &str, call_id: &str) -> Redactions {
        self.call_redactions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(session_id.to_owned(), call_id.to_owned()))
            .unwrap_or_default()
    }
}

/// Refuse a write whose new text holds a redaction marker its current text does not:
/// the model saw a placeholder, never the value, and writing it would destroy the
/// real secret in the file (`builtin-tools` → Redaction markers are never written).
pub(crate) fn refuse_marker(before: &str, after: &str) -> Result<(), ToolError> {
    match redact::introduced_marker(before, after) {
        Some(marker) => Err(failed(marker_refusal(&marker))),
        None => Ok(()),
    }
}

/// Refuse a shell command or terminal input (`bash`, `powershell`, `pty_start`,
/// `pty_write`) that carries a marker: a heredoc, `sed` or redirect could only write the
/// placeholder over a real value, and anything else would send it where the secret was
/// expected.
pub(crate) fn refuse_shell_marker(
    inv: &cyber_server::runtime::Invocation,
) -> Result<(), ToolError> {
    let field = match inv.name.as_str() {
        "bash" | "powershell" | "pty_start" => "command",
        "pty_write" => "input",
        _ => return Ok(()),
    };
    refuse_marker_in_command(inv.input[field].as_str().unwrap_or_default())
}

fn refuse_marker_in_command(command: &str) -> Result<(), ToolError> {
    match redact::markers(command).next() {
        Some(marker) => Err(failed(format!(
            "{} Refer to the secret indirectly instead, for example through the environment variable or file that holds it.",
            marker_refusal(marker)
        ))),
        None => Ok(()),
    }
}

fn marker_refusal(marker: &str) -> String {
    format!(
        "Refused: the content contains the redaction marker {marker}. A secret was hidden from you and that marker only stands in for it; writing the marker would replace the real value. Leave the line that holds the secret unchanged, or ask the user to edit it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_new_to_the_text_are_refused() {
        let error = refuse_marker("KEY=real", "KEY=[REDACTED:assignment]").unwrap_err();
        let ToolError::Failed(message) = error else {
            panic!("expected a failure");
        };
        assert!(message.contains("[REDACTED:assignment]") && message.contains("hidden"));
        assert!(refuse_marker("doc [REDACTED:jwt]", "doc [REDACTED:jwt] more").is_ok());
        assert!(refuse_marker_in_command("echo [REDACTED:github_token] > .env").is_err());
        assert!(refuse_marker_in_command("grep -r REDACTED .").is_ok());
    }
}
