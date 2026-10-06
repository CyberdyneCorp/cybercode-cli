//! `cyber exec` (`exec-mode`): one non-interactive run against a Session.

mod report;
pub(crate) mod setup;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args;
use cyber_client::{Client, Event};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::cli::{Format, GlobalArgs};
use crate::context::Context;
use crate::error::CliError;
use report::{Out, Run};

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Start a new Session in a managed worktree; omit NAME to generate one.
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "NAME")]
    pub worktree: Option<String>,
    /// The prompt; stdin is appended when it is not a terminal.
    pub prompt: Vec<String>,
    /// Reuse the most recent root Session of the Location.
    #[arg(long = "continue")]
    pub resume_last: bool,
    /// Reuse a Session by ID or name.
    #[arg(long)]
    pub session: Option<String>,
    /// Fork the selected Session first (needs --continue or --session).
    #[arg(long)]
    pub fork: bool,
    /// Name the new Session.
    #[arg(long)]
    pub name: Option<String>,
    /// Run in memory: nothing is written to the shared database or snapshots.
    #[arg(long)]
    pub ephemeral: bool,
    /// Run a private in-process server instead of the background service.
    #[arg(long)]
    pub embedded: bool,
    /// Target a remote server URL.
    #[arg(long)]
    pub attach: Option<String>,
    /// Password for --attach (else CYBER_SERVER_PASSWORD).
    #[arg(long)]
    pub password: Option<String>,
    #[arg(long)]
    pub agent: Option<String>,
    /// Use `auto` mode.
    #[arg(long)]
    pub auto: bool,
    /// Use `bypass` mode: everything runs without approval.
    #[arg(long)]
    pub yolo: bool,
    /// Add the never-ask rules to a reused Session too.
    #[arg(long)]
    pub non_interactive_rules: bool,
    /// Run a skill or custom command with the prompt as its arguments.
    #[arg(long)]
    pub command: Option<String>,
    /// Attach a file (repeatable).
    #[arg(long = "file")]
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub max_turns: Option<u32>,
    #[arg(long)]
    pub max_tokens: Option<u64>,
    /// Stop after this cost in USD.
    #[arg(long)]
    pub max_cost: Option<f64>,
    /// Stop after this long, e.g. 30m, 90s.
    #[arg(long)]
    pub timeout: Option<String>,
    /// Print tool progress on stderr (text format).
    #[arg(long)]
    pub verbose: bool,
    /// No non-result output on stderr.
    #[arg(long)]
    pub quiet: bool,
    /// Include reasoning events (stream-json).
    #[arg(long)]
    pub thinking: bool,
    /// Sandbox policy: read-only, workspace-write or full-access (embedded runs).
    #[arg(long)]
    pub sandbox: Option<String>,
    /// Exit with code 5 when any action was denied.
    #[arg(long)]
    pub fail_on_deny: bool,
}

pub const EXIT_ERROR: u8 = 1;
pub const EXIT_BUDGET: u8 = 4;
pub const EXIT_DENIED: u8 = 5;
pub const EXIT_INTERRUPTED: u8 = 130;

pub fn run(args: ExecArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let prompt = setup::prompt(&args.prompt)?;
    if prompt.is_empty() && args.command.is_none() {
        return Err(
            CliError::usage("no prompt: pass one as arguments or on stdin")
                .with_hint("cyber exec \"fix the failing tests\""),
        );
    }
    setup::validate(&args)?;
    let timeout = args.timeout.as_deref().map(parse_duration).transpose()?;
    let format = global.format.unwrap_or(Format::Text);
    if !matches!(format, Format::Text | Format::Json | Format::StreamJson) {
        return Err(CliError::usage(
            "exec supports --format text, json or stream-json",
        ));
    }
    let code = super::serve::runtime()?.block_on(async {
        let backend = setup::backend(&args, ctx).await?;
        let client = backend.client.at(&ctx.location.display().to_string());
        let session = setup::session(&client, &args, global).await?;
        let client = if args.worktree.is_some() {
            client.at(session["directory"]
                .as_str()
                .ok_or_else(|| CliError::runtime("Worktree Session has no Location"))?)
        } else {
            client
        };
        let out = Out::new(format, &args, &session);
        execute(&client, &session, &args, prompt, timeout, out).await
    })?;
    if code == 0 {
        Ok(())
    } else {
        Err(CliError::silent(code))
    }
}

/// Send the prompt and follow the Session until its Drain goes idle.
async fn execute(
    client: &Client,
    session: &Value,
    args: &ExecArgs,
    prompt: String,
    timeout: Option<Duration>,
    mut out: Out,
) -> Result<u8, CliError> {
    let id = session["id"].as_str().unwrap_or_default().to_string();
    out.init(client, session).await;
    let mut events =
        Box::pin(
            client.events(true).await.map_err(api)?.filter(move |e| {
                futures::future::ready(e.session_id.as_deref() == Some(id.as_str()))
            }),
        );
    let id = session["id"].as_str().unwrap_or_default();
    setup::send(client, id, args, prompt).await?;
    let mut run = Run::new(args, Instant::now());
    let deadline = tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(u64::MAX / 4)));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = events.next() => match event {
                Some(event) => {
                    if handle(&mut run, &mut out, client, id, event).await {
                        break;
                    }
                }
                None => {
                    run.error = Some("the server closed the event stream".into());
                    break;
                }
            },
            _ = &mut deadline => {
                stop(client, id, &mut run, "timeout").await;
            }
            _ = tokio::signal::ctrl_c() => {
                stop(client, id, &mut run, "interrupted").await;
            }
        }
    }
    Ok(out.finish(&run, args, session))
}

/// Interrupt the Drain; the run ends when it reports idle.
async fn stop(client: &Client, id: &str, run: &mut Run, reason: &str) {
    if run.stop_reason.is_none() {
        run.stop_reason = Some(reason.into());
        let _ = client
            .post(&format!("/sessions/{id}/interrupt"), json!({}))
            .await;
    }
}

/// Apply one event; true when the run is over.
async fn handle(run: &mut Run, out: &mut Out, client: &Client, id: &str, event: Event) -> bool {
    match event.kind.as_str() {
        "session.idle" => return true,
        "session.error" => {
            let message = event.data["message"]
                .as_str()
                .unwrap_or("error")
                .to_string();
            out.error(&message);
            run.error = Some(message);
        }
        "session.deleted" => {
            run.error = Some("the Session was deleted".into());
            return true;
        }
        kind => {
            out.durable(run, kind, &event.data);
            if kind.starts_with("session.step.ended") && run.over_budget() {
                stop(client, id, run, "budget_exceeded").await;
            }
        }
    }
    false
}

fn api(e: cyber_client::ClientError) -> CliError {
    CliError::runtime(e.to_string())
}

/// `30m`, `90s`, `1h`, `500ms` or bare seconds.
pub fn parse_duration(s: &str) -> Result<Duration, CliError> {
    let s = s.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num
        .parse()
        .map_err(|_| CliError::usage(format!("invalid duration {s:?}")))?;
    let secs = match unit {
        "" | "s" => n,
        "ms" => n / 1000.0,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => {
            return Err(CliError::usage(format!(
                "invalid duration unit in {s:?}; use ms, s, m or h"
            )));
        }
    };
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("2").unwrap(), Duration::from_secs(2));
        assert!(parse_duration("3 weeks").is_err());
    }
}
