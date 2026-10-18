//! `cyber pr <number>` and `cyber --from-pr` (`vcs-integration` → PR checkout and linked
//! sessions): check out a pull request with `gh pr checkout` (which adds a fork remote
//! when needed) and resume the most recent Session linked to it, or start a new one.
//! Sessions are linked when they create or push to a PR branch. With `--format json` the
//! decision is printed instead of opening the TUI.

use clap::Args;
use cyber_client::Client;
use serde_json::{Value, json};

use crate::cli::{GlobalArgs, LaunchArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Args)]
pub struct PrArgs {
    /// The pull request number or URL.
    #[arg(value_name = "NUMBER|URL")]
    pub pr: String,
}

/// A PR number, or the number of a `…/pull/<number>` URL.
pub fn parse(text: &str) -> Result<u64, CliError> {
    let digits = match text.split_once("/pull/") {
        Some((_, rest)) => rest.split(['/', '#', '?']).next().unwrap_or_default(),
        None => text.trim_start_matches('#'),
    };
    digits
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| CliError::usage(format!("{text}: expected a pull request number or URL")))
}

pub fn run(args: PrArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let number = parse(&args.pr)?;
    checkout(ctx, number)?;
    let rt = super::serve::runtime()?;
    let linked = rt.block_on(linked_sessions(ctx, number, 1))?;
    let session = linked.first().cloned();
    if output::is_json(global.format) {
        return output::json(&json!({
            "pr": number,
            "checked_out": true,
            "session_id": session,
            "resumed": session.is_some(),
        }));
    }
    let launch = LaunchArgs {
        resume: session,
        ..LaunchArgs::default()
    };
    super::tui::run(&launch, global, None)
}

/// `gh pr checkout <number>` in the Location; its output goes to stderr.
fn checkout(ctx: &Context, number: u64) -> Result<(), CliError> {
    let output = std::process::Command::new("gh")
        .args(["pr", "checkout", &number.to_string()])
        .current_dir(&ctx.location)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            CliError::runtime(format!("cannot run gh: {error}"))
                .with_hint("install the GitHub CLI (gh) and run gh auth login")
        })?;
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(CliError::runtime(format!("gh pr checkout {number} failed")));
    }
    Ok(())
}

/// IDs of the Location's Sessions linked to `number`, most recent first.
pub async fn linked_sessions(
    ctx: &Context,
    number: u64,
    limit: u32,
) -> Result<Vec<String>, CliError> {
    let info = super::serve::start(ctx).await?;
    let client = Client::http(&info.registration.url, Some(info.password))
        .at(&ctx.location.display().to_string());
    let list = client
        .get(&format!(
            "/sessions?limit={limit}&pr={number}&children=true"
        ))
        .await
        .map_err(|error| CliError::runtime(error.to_string()))?;
    Ok(list["data"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| session["id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>())
}

/// `cyber --from-pr <number|url> --format json`: the linked Sessions instead of the picker.
pub fn report_linked(ctx: &Context, number: u64) -> Result<(), CliError> {
    let sessions = super::serve::runtime()?.block_on(linked_sessions(ctx, number, 100))?;
    output::json(
        &json!({ "pr": number, "sessions": sessions.into_iter().map(Value::String).collect::<Vec<_>>() }),
    )
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn numbers_and_pull_urls_parse() {
        assert_eq!(parse("42").unwrap(), 42);
        assert_eq!(parse("#42").unwrap(), 42);
        assert_eq!(
            parse("https://github.com/acme/app/pull/42/files").unwrap(),
            42
        );
        assert!(parse("0").is_err());
        assert!(parse("main").is_err());
    }
}
