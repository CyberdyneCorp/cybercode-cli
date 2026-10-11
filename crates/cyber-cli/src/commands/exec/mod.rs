//! `cyber exec` (`exec-mode`): one non-interactive run against a Session.

mod background;
mod delegation;
pub(crate) mod report;
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
use background::{Owned, Settle};
use report::{Out, Run};

#[derive(Debug, Default, Args)]
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
    launch(args, ctx, global, prompt, None)
}

/// `cyber review`: the `review` command over the normalized target, reported as findings.
pub(crate) fn review(
    args: ExecArgs,
    ctx: &Context,
    global: &GlobalArgs,
    target: cyber_core::review::Target,
) -> Result<(), CliError> {
    let prompt = target.normalized();
    launch(args, ctx, global, prompt, Some(target))
}

fn launch(
    args: ExecArgs,
    ctx: &Context,
    global: &GlobalArgs,
    prompt: String,
    review: Option<cyber_core::review::Target>,
) -> Result<(), CliError> {
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
        let mut out = Out::new(format, &args, &session);
        match review {
            Some(target) => {
                out.review(target);
                execute_session(&client, &session, &args, prompt, timeout, out).await
            }
            None => execute(&client, &session, &args, prompt, timeout, out).await,
        }
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
    out: Out,
) -> Result<u8, CliError> {
    if let Some((agent, prompt)) = delegation::target(client, args, &prompt).await? {
        return delegation::execute(client, session, args, agent, prompt, timeout, out).await;
    }
    execute_session(client, session, args, prompt, timeout, out).await
}

async fn execute_session(
    client: &Client,
    session: &Value,
    args: &ExecArgs,
    prompt: String,
    timeout: Option<Duration>,
    mut out: Out,
) -> Result<u8, CliError> {
    let id = session["id"].as_str().unwrap_or_default().to_string();
    let mut run = Run::new(args, Instant::now());
    let baseline = client.get(&format!("/sessions/{id}")).await.map_err(api)?;
    run.begin_session(&baseline["data"])?;
    out.init(client, session).await;
    if run.over_budget() {
        run.stop_reason = Some("budget_exceeded".into());
        return Ok(out.finish(&run, args, session));
    }
    let mut events =
        Box::pin(
            client.events(true).await.map_err(api)?.filter(move |e| {
                futures::future::ready(e.session_id.as_deref() == Some(id.as_str()))
            }),
        );
    let id = session["id"].as_str().unwrap_or_default();
    let mut owned = Owned::capture(client, id).await?;
    let (path, body) = setup::request(id, args, prompt)?;
    let message = match client.post(&path, body).await {
        Ok(receipt) => receipt["data"]["message_id"].as_str().map(str::to_string),
        Err(error) => return out.refused(error, &run, args, session),
    };
    if let (Some(_), Some(message)) = (&args.command, &message) {
        owned.command(message)?;
    }
    let mut follow = Follow {
        client,
        id,
        run: &mut run,
        out: &mut out,
        owned: &mut owned,
    };
    follow.run(&mut events, timeout).await;
    if out.is_review() {
        match owned.fork_job(client).await {
            Ok(job) => out.review_job(owned.fork_id(), job),
            Err(error) => {
                run.error.get_or_insert(error.message);
            }
        }
    }
    if let Err(error) = refresh_usage(client, id, &mut run).await {
        run.error.get_or_insert(error.message);
    } else if run.over_budget() {
        run.stop_reason
            .get_or_insert_with(|| "budget_exceeded".into());
    }
    Ok(out.finish(&run, args, session))
}

/// What one event means for the run.
enum Step {
    Continue,
    Idle,
    End,
}

/// A run following its Session until it and the background work it owns settle.
struct Follow<'a> {
    client: &'a Client,
    id: &'a str,
    run: &'a mut Run,
    out: &'a mut Out,
    owned: &'a mut Owned,
}

impl Follow<'_> {
    async fn run<S: futures::Stream<Item = Event> + Unpin>(
        &mut self,
        events: &mut S,
        timeout: Option<Duration>,
    ) {
        let mut billing = tokio::time::interval(Duration::from_millis(200));
        billing.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut settle_poll = tokio::time::interval(Duration::from_millis(500));
        settle_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut waiting = false;
        let deadline = tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(u64::MAX / 4)));
        tokio::pin!(deadline);
        loop {
            let done = tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.event(event, &mut waiting).await,
                    None => {
                        self.run.error = Some("the server closed the event stream".into());
                        true
                    }
                },
                _ = settle_poll.tick(), if waiting => self.settled().await,
                _ = billing.tick() => monitor_billing(self.client, self.id, self.run, self.owned).await,
                _ = &mut deadline, if self.run.stop_requested.is_none() => {
                    stop(self.client, self.id, self.run, self.owned, "timeout").await;
                    false
                }
                _ = tokio::signal::ctrl_c(), if self.run.stop_requested.is_none() => {
                    stop(self.client, self.id, self.run, self.owned, "interrupted").await;
                    false
                }
            };
            if done {
                break;
            }
        }
    }

    /// Apply one event; true when the run is over.
    async fn event(&mut self, event: Event, waiting: &mut bool) -> bool {
        self.owned.observe(&event);
        match handle(self.run, self.out, self.client, self.id, self.owned, event).await {
            Step::Continue => false,
            Step::End => true,
            Step::Idle => {
                let done = self.settled().await;
                *waiting = !done;
                done
            }
        }
    }

    /// Idle is final only once the background work this run owns has settled.
    async fn settled(&mut self) -> bool {
        let stopping = self.run.stop_requested.is_some();
        if stopping {
            // Work the Drain started before it acknowledged the interrupt.
            self.owned.stop(self.client, self.run).await;
        }
        match self.owned.settle(self.client, stopping).await {
            Ok(Settle::Done) => true,
            Ok(Settle::Wait) => false,
            Err(error) => {
                self.run.error = Some(error.message);
                true
            }
        }
    }
}

async fn monitor_billing(client: &Client, id: &str, run: &mut Run, owned: &mut Owned) -> bool {
    if run
        .stop_requested
        .is_some_and(|started| started.elapsed() >= Duration::from_secs(5))
    {
        run.error.get_or_insert_with(|| {
            "Session interruption was not acknowledged; inspect the Session before retrying".into()
        });
        return true;
    }
    if let Err(error) = refresh_usage(client, id, run).await {
        run.error = Some(error.message);
        stop(client, id, run, owned, "error").await;
    } else if run.over_budget() {
        stop(client, id, run, owned, "budget_exceeded").await;
    }
    false
}

async fn refresh_usage(client: &Client, id: &str, run: &mut Run) -> Result<(), CliError> {
    let snapshot = tokio::time::timeout(
        Duration::from_secs(2),
        client.get(&format!("/sessions/{id}")),
    )
    .await
    .map_err(|_| CliError::runtime("Exec billing refresh timed out"))?
    .map_err(api)?;
    run.session_snapshot(&snapshot["data"])
}

/// Cancel the background work the run owns, then interrupt the Drain so no
/// cancellation handback runs after exit; the run ends when it reports idle.
async fn stop(client: &Client, id: &str, run: &mut Run, owned: &mut Owned, reason: &str) {
    if run.stop_requested.is_some() {
        return;
    }
    run.stop_reason.get_or_insert_with(|| reason.into());
    owned.stop(client, run).await;
    match tokio::time::timeout(
        Duration::from_secs(2),
        client.post(&format!("/sessions/{id}/interrupt"), json!({})),
    )
    .await
    {
        Ok(Ok(_)) => {}
        _ => {
            run.error
                .get_or_insert_with(|| "Session interruption request was not acknowledged".into());
        }
    }
    // The acknowledgement watchdog measures only the interrupt.
    run.stop_requested = Some(Instant::now());
}

async fn handle(
    run: &mut Run,
    out: &mut Out,
    client: &Client,
    id: &str,
    owned: &mut Owned,
    event: Event,
) -> Step {
    match event.kind.as_str() {
        "session.idle" => return Step::Idle,
        "session.hook.notice" => {
            eprintln!(
                "hook {}: {}",
                event.data["hook_id"].as_str().unwrap_or_default(),
                event.data["message"].as_str().unwrap_or_default()
            );
        }
        "session.error" => {
            if event.data["kind"] == "budget_exceeded" {
                run.stop_reason
                    .get_or_insert_with(|| "budget_exceeded".into());
            }
            let message = event.data["message"]
                .as_str()
                .unwrap_or("error")
                .to_string();
            out.error(&message);
            run.error = Some(message);
        }
        "session.deleted" => {
            run.error = Some("the Session was deleted".into());
            return Step::End;
        }
        kind => {
            out.durable(run, kind, &event.data);
            if kind.starts_with("session.step.ended") && run.over_budget() {
                stop(client, id, run, owned, "budget_exceeded").await;
            }
        }
    }
    Step::Continue
}

fn api(e: cyber_client::ClientError) -> CliError {
    CliError::runtime(e.to_string())
}

/// A server-issued ID with the expected prefix that is safe inside a URL path.
fn safe_id(id: &str, prefix: &str) -> bool {
    cyber_core::ids::has_prefix(id, prefix)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
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
