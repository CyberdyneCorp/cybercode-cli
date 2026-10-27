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
#[command(group(clap::ArgGroup::new("autofix_action").args(["autofix", "autofix_status", "autofix_pause", "autofix_resume", "autofix_off"]).multiple(false)))]
pub struct PrArgs {
    /// The pull request number or URL.
    #[arg(value_name = "NUMBER|URL")]
    pub pr: String,
    /// Enable bounded self-hosted auto-fix without checking out or opening the TUI.
    #[arg(long)]
    pub autofix: bool,
    /// Show durable subscription status and attempt evidence.
    #[arg(long)]
    pub autofix_status: bool,
    /// Pause the subscription; closing the terminal does not pause it.
    #[arg(long)]
    pub autofix_pause: bool,
    /// Resume a paused subscription with no unresolved attempt.
    #[arg(long)]
    pub autofix_resume: bool,
    /// Disable the subscription; already dispatched actions are reconciled visibly.
    #[arg(long)]
    pub autofix_off: bool,
    /// GitHub owner/repository (also accepted from a github.com PR URL).
    #[arg(long, value_name = "OWNER/REPO")]
    pub autofix_repository: Option<String>,
    /// Existing owner Session in the current Location; required on first enable.
    #[arg(long, value_name = "SESSION_ID")]
    pub autofix_session: Option<String>,
    /// Model for isolated fix attempts; required on first enable.
    #[arg(long, value_name = "PROVIDER/MODEL")]
    pub autofix_model: Option<String>,
    /// Pinned verification argv as a JSON array; repeat for multiple checks.
    #[arg(long, value_name = "JSON_ARGV")]
    pub autofix_check: Vec<String>,
    /// Required CI identity (check:APP_ID:NAME or status:CREATOR_ID:CONTEXT); repeat.
    #[arg(long, value_name = "CI_ID")]
    pub autofix_required_ci: Vec<String>,
    /// Repository-relative verification sources to protect from edits; repeat.
    #[arg(long, value_name = "PATH")]
    pub autofix_verification_path: Vec<String>,
    /// Maximum consecutive failed attempts, 1..10.
    #[arg(long, default_value_t = 3)]
    pub autofix_max_attempts: u32,
    /// Enforceable shared token cap per attempt; required on first enable.
    #[arg(long, value_name = "TOKENS")]
    pub autofix_max_tokens: Option<u64>,
    /// Maximum elapsed seconds per attempt, 1..7200; required on first enable.
    #[arg(long, value_name = "SECONDS")]
    pub autofix_max_seconds: Option<u32>,
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
    if args.autofix
        || args.autofix_status
        || args.autofix_pause
        || args.autofix_resume
        || args.autofix_off
    {
        return super::serve::runtime()?.block_on(autofix(&args, ctx, global, number));
    }
    if args.autofix_repository.is_some()
        || args.autofix_session.is_some()
        || args.autofix_model.is_some()
        || !args.autofix_check.is_empty()
        || !args.autofix_required_ci.is_empty()
        || !args.autofix_verification_path.is_empty()
        || args.autofix_max_tokens.is_some()
        || args.autofix_max_seconds.is_some()
    {
        return Err(CliError::usage(
            "auto-fix options require an --autofix action",
        ));
    }
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

fn autofix_repository(args: &PrArgs) -> Result<String, CliError> {
    let from_url = args
        .pr
        .strip_prefix("https://github.com/")
        .and_then(|rest| rest.split_once("/pull/"))
        .map(|(repo, _)| repo.to_owned());
    args.autofix_repository.clone().or(from_url).ok_or_else(|| {
        CliError::usage(
            "provide --autofix-repository OWNER/REPO or a https://github.com/OWNER/REPO/pull/N URL",
        )
    })
}
fn autofix_config(
    args: &PrArgs,
    number: u64,
    repository: String,
) -> Result<cyber_core::autofix::Config, CliError> {
    let required = |value: &Option<String>, flag: &str| {
        value
            .clone()
            .ok_or_else(|| CliError::usage(format!("{flag} is required when enabling auto-fix")))
    };
    let checks = args
        .autofix_check
        .iter()
        .map(|text| {
            serde_json::from_str::<Vec<String>>(text).map_err(|_| {
                CliError::usage(
                    "--autofix-check expects a JSON argv array, for example '[\"cargo\",\"test\"]'",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let config = cyber_core::autofix::Config {
        repository,
        pr: number,
        session_id: required(&args.autofix_session, "--autofix-session")?,
        model: required(&args.autofix_model, "--autofix-model")?,
        checks,
        required_ci: args.autofix_required_ci.clone(),
        verification_paths: args.autofix_verification_path.clone(),
        max_attempts: args.autofix_max_attempts,
        max_tokens: args
            .autofix_max_tokens
            .ok_or_else(|| CliError::usage("--autofix-max-tokens is required"))?,
        max_seconds: args
            .autofix_max_seconds
            .ok_or_else(|| CliError::usage("--autofix-max-seconds is required"))?,
    };
    config.validate().map_err(CliError::usage)?;
    Ok(config)
}
async fn autofix(
    args: &PrArgs,
    ctx: &Context,
    global: &GlobalArgs,
    number: u64,
) -> Result<(), CliError> {
    let repository = autofix_repository(args)?;
    // Validate enable arguments before starting a server or touching the checkout.
    let config = args
        .autofix
        .then(|| autofix_config(args, number, repository.clone()))
        .transpose()?;
    let info = super::serve::start(ctx).await?;
    let client = Client::http(&info.registration.url, Some(info.password))
        .at(&ctx.location.display().to_string());
    let response = if let Some(config) = config {
        client
            .post(
                "/autofix/subscriptions",
                serde_json::to_value(config).map_err(|e| CliError::usage(e.to_string()))?,
            )
            .await
    } else {
        let list = client
            .get("/autofix/subscriptions")
            .await
            .map_err(|e| CliError::runtime(e.to_string()))?;
        let records = list["data"]
            .as_array()
            .ok_or_else(|| CliError::runtime("server returned no subscription list"))?;
        let selected = records
            .iter()
            .filter(|s| {
                s["config"]["repository"]
                    .as_str()
                    .is_some_and(|r| r.eq_ignore_ascii_case(&repository))
                    && s["config"]["pr"].as_u64() == Some(number)
            })
            .max_by_key(|s| {
                (
                    !matches!(s["status"].as_str(), Some("disabled" | "closed")),
                    s["created_ms"].as_i64().unwrap_or_default(),
                )
            })
            .ok_or_else(|| {
                CliError::runtime("no auto-fix subscription for this PR in the current Location")
            })?;
        if args.autofix_status {
            return report_autofix(global, selected);
        }
        let id = selected["id"]
            .as_str()
            .ok_or_else(|| CliError::runtime("subscription has no ID"))?;
        let action = if args.autofix_pause {
            "pause"
        } else if args.autofix_resume {
            "resume"
        } else {
            "disable"
        };
        client
            .post(
                &format!("/autofix/subscriptions/{id}/control"),
                json!({"expected_revision":selected["revision"],"action":action}),
            )
            .await
    }
    .map_err(|e| CliError::runtime(e.to_string()))?;
    report_autofix(global, &response["data"])
}
fn report_autofix(global: &GlobalArgs, subscription: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(subscription);
    }
    println!(
        "{}: {} PR #{} — {} ({} consecutive failures)",
        subscription["id"].as_str().unwrap_or(""),
        subscription["config"]["repository"].as_str().unwrap_or(""),
        subscription["config"]["pr"],
        subscription["status"].as_str().unwrap_or("unknown"),
        subscription["failures"]
    );
    if let Some(reason) = subscription["reason"].as_str() {
        println!("{reason}");
    }
    if let Some(attempt) = subscription["attempts"].as_array().and_then(|a| a.last()) {
        println!(
            "Last attempt: {} — {}",
            attempt["id"].as_str().unwrap_or(""),
            attempt["stage"].as_str().unwrap_or("unknown")
        );
    }
    println!(
        "Runs while this self-hosted server is available. Use --format json for complete limits, checks and usage evidence."
    );
    Ok(())
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

    fn args(input: &[&str]) -> super::PrArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Command {
            #[command(flatten)]
            args: super::PrArgs,
        }
        Command::parse_from(input).args
    }
    #[test]
    fn autofix_requires_explicit_limits_and_preserves_argv() {
        let mut parsed = args(&[
            "pr",
            "3",
            "--autofix",
            "--autofix-repository",
            "team/repo",
            "--autofix-session",
            "ses_owner",
            "--autofix-model",
            "test/main",
            "--autofix-check",
            r#"["cargo","test","name with spaces"]"#,
            "--autofix-required-ci",
            "check:9:test",
            "--autofix-verification-path",
            "tests",
            "--autofix-max-tokens",
            "1000",
            "--autofix-max-seconds",
            "60",
        ]);
        let config =
            super::autofix_config(&parsed, 3, super::autofix_repository(&parsed).unwrap()).unwrap();
        assert_eq!(config.checks[0], vec!["cargo", "test", "name with spaces"]);
        assert_eq!(config.max_attempts, 3);
        parsed.autofix_max_tokens = None;
        assert!(super::autofix_config(&parsed, 3, "team/repo".into()).is_err());
    }
    #[test]
    fn autofix_repository_uses_only_explicit_repo_or_github_https_url() {
        assert_eq!(
            super::autofix_repository(&args(&[
                "pr",
                "https://github.com/team/repo/pull/3",
                "--autofix-status"
            ]))
            .unwrap(),
            "team/repo"
        );
        assert!(
            super::autofix_repository(&args(&[
                "pr",
                "https://attacker.invalid/team/repo/pull/3",
                "--autofix-status"
            ]))
            .is_err()
        );
        assert!(super::autofix_repository(&args(&["pr", "3", "--autofix-status"])).is_err());
    }

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
