//! `cyber exec --output-style` (`system-context` → Output style selection): the name is
//! checked against the Location's listing before any Session exists.

use cyber_client::Client;
use cyber_core::output_styles::{HELD, valid_name};
use serde_json::Value;

use super::ExecArgs;
use super::setup::api;
use crate::error::CliError;

/// Refuse a `--output-style` the Location does not list (exit 2 with the reason and
/// the available names); without the flag, warn when the configured style does not apply.
pub async fn check(client: &Client, args: &ExecArgs) -> Result<(), CliError> {
    let Some(name) = &args.output_style else {
        if !args.quiet
            && let Ok(listing) = client.get("/config/output-styles").await
            && let Some(reason) = listing["data"]["unavailable"].as_str()
        {
            eprintln!("warning: {reason}; the default output style applies");
        }
        return Ok(());
    };
    let listing = client.get("/config/output-styles").await.map_err(api)?;
    refusal(&listing["data"], name).map_or(Ok(()), Err)
}

/// Why `name` cannot be selected from `listing`, with the names that can.
fn refusal(listing: &Value, name: &str) -> Option<CliError> {
    let styles = listing["styles"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if styles.iter().any(|style| style["name"] == name) {
        return None;
    }
    let issues = listing["issues"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let invalid = issues.iter().find(|issue| {
        let path = std::path::Path::new(issue["path"].as_str().unwrap_or_default());
        path.file_stem().and_then(|stem| stem.to_str()) == Some(name)
    });
    let reason = match invalid {
        Some(issue) => format!(
            "output style \"{name}\" is invalid: {}",
            issue["message"].as_str().unwrap_or_default()
        ),
        None if issues.iter().any(|issue| issue["message"] == HELD) => {
            format!("unknown output style \"{name}\"; {HELD}")
        }
        None => format!("unknown output style \"{name}\""),
    };
    let names: Vec<&str> = styles.iter().filter_map(|s| s["name"].as_str()).collect();
    Some(CliError::usage(reason).with_hint(format!("available: {}", names.join(", "))))
}

/// The syntax check, before any server contact.
pub fn validate(args: &ExecArgs) -> Result<(), CliError> {
    match &args.output_style {
        Some(name) if !valid_name(name) => Err(CliError::usage(format!(
            "invalid output style name \"{name}\""
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use serde_json::json;

    fn exec(args: &[&str]) -> ExecArgs {
        match Cli::try_parse_from(args).unwrap().command {
            Some(Command::Exec(args)) => *args,
            _ => panic!("expected exec"),
        }
    }

    #[test]
    fn output_style_flag_parses_and_is_syntax_checked() {
        let args = exec(&["cyber", "exec", "--output-style", "concise", "hi"]);
        assert_eq!(args.output_style.as_deref(), Some("concise"));
        assert!(validate(&args).is_ok());
        assert_eq!(exec(&["cyber", "exec", "hi"]).output_style, None);
        let error =
            validate(&exec(&["cyber", "exec", "--output-style", "../x", "hi"])).unwrap_err();
        assert_eq!(error.message, "invalid output style name \"../x\"");
        assert!(Cli::try_parse_from(["cyber", "exec", "--output-style"]).is_err());
    }

    #[test]
    fn refusal_names_the_reason_and_the_available_styles() {
        let listing = json!({
            "styles": [{"name": "default"}, {"name": "concise"}],
            "issues": [
                {"path": "/r/.cyber/output-styles", "message": HELD},
                {"path": "/u/output-styles/broken.md", "message": "empty body"},
            ],
        });
        assert!(refusal(&listing, "concise").is_none());
        let unknown = refusal(&listing, "nope").unwrap();
        assert_eq!(unknown.code, 2);
        assert_eq!(
            unknown.message,
            format!("unknown output style \"nope\"; {HELD}")
        );
        assert_eq!(unknown.hint.as_deref(), Some("available: default, concise"));
        let broken = refusal(&listing, "broken").unwrap();
        assert_eq!(
            broken.message,
            "output style \"broken\" is invalid: empty body"
        );
    }
}
