//! `cyber exec` (`exec-mode`): one non-interactive run against a Session.

mod background;
mod delegation;
pub(crate) mod goals;
pub(crate) mod report;
mod review_run;
pub(crate) mod setup;
mod style;
mod workflow;

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
    /// Use `bypass` mode: everything runs without approval. The server refuses it unless
    /// commands run in the OS sandbox or a container, or with
    /// --dangerously-bypass-permissions.
    #[arg(long)]
    pub yolo: bool,
    /// Allow `bypass` mode without a sandbox or a container (needs --embedded or
    /// --ephemeral: a shared server cannot take it for one run).
    #[arg(long)]
    pub dangerously_bypass_permissions: bool,
    /// Claude Code's name for running without prompts: `bypass` mode plus
    /// --dangerously-bypass-permissions, so every action runs without approval and,
    /// outside a sandbox or a container, without OS confinement. Needs --embedded or
    /// --ephemeral: a shared server cannot take the override for one run (there, use
    /// --mode bypass where its sandbox or a container already admits it). An
    /// organization policy that disables `bypass` still refuses it.
    #[arg(long, conflicts_with = "auto")]
    pub dangerously_skip_permissions: bool,
    /// Add the never-ask rules to a reused Session too.
    #[arg(long)]
    pub non_interactive_rules: bool,
    /// Let other Sessions message the Session this run creates (by default it refuses
    /// them with `non_interactive`).
    #[arg(long)]
    pub accept_messages: bool,
    /// Run a skill or custom command with the prompt as its arguments.
    #[arg(long)]
    pub command: Option<String>,
    /// Use this output style for the Session (a name from `/output-style`); it must be
    /// listed at the Location, or the run exits 2 before any Session exists.
    #[arg(long, value_name = "NAME")]
    pub output_style: Option<String>,
    /// Run a Workflow (a saved name or a `.js` module path) instead of a prompt; its return
    /// value is the result.
    #[arg(long, value_name = "NAME|PATH", conflicts_with_all = ["command", "prompt"])]
    pub workflow: Option<String>,
    /// The Workflow's `args` as a JSON object.
    #[arg(long = "args", value_name = "JSON", requires = "workflow")]
    pub workflow_args: Option<String>,
    /// Attach a file (repeatable): an image or PDF as an attachment, read by a local
    /// server under the `read` permission (sent inline with --attach); others as text.
    #[arg(long = "file")]
    pub files: Vec<PathBuf>,
    /// With --goal: each Goal's `max_turns` budget; otherwise the Session's step ceiling.
    #[arg(long)]
    pub max_turns: Option<u32>,
    /// Work until this Goal is met (repeatable: a queue, in order). The exit code is 0
    /// met, 3 impossible, 4 budget exceeded, 1 otherwise, for the first non-met Goal.
    #[arg(long = "goal", value_name = "CONDITION")]
    pub goal: Vec<String>,
    /// A command that must pass for the --goal before it to count as met.
    #[arg(long = "goal-check", value_name = "COMMAND")]
    pub goal_check: Vec<String>,
    /// The Goals with their checks, paired from the raw arguments (filled by `run`).
    #[arg(skip)]
    pub goals: Vec<goals::GoalArg>,
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
    /// Run the currently resolved project/local hooks for this run only, without
    /// approving them for later runs (needs --embedded or --ephemeral).
    #[arg(long)]
    pub trust_project_hooks: bool,
}

impl ExecArgs {
    /// Whether this run takes the no-sandbox `bypass` override, under either name.
    pub(crate) fn bypass_override(&self) -> bool {
        self.dangerously_bypass_permissions || self.dangerously_skip_permissions
    }
}

pub const EXIT_ERROR: u8 = 1;
pub const EXIT_BUDGET: u8 = 4;
pub const EXIT_DENIED: u8 = 5;
pub const EXIT_INTERRUPTED: u8 = 130;

pub fn run(mut args: ExecArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if !args.goal.is_empty() {
        args.goals = goals::pair(std::env::args().skip(1))?;
        if args.goals.len() != args.goal.len() || args.goal_check.len() > args.goal.len() {
            return Err(CliError::usage(
                "pass --goal-check right after the --goal it checks",
            ));
        }
    } else if !args.goal_check.is_empty() {
        return Err(CliError::usage("--goal-check needs a --goal before it"));
    }
    if args.workflow.is_some() {
        return launch(args, ctx, global, String::new(), None);
    }
    let prompt = if args.goals.is_empty() {
        setup::prompt(&args.prompt)?
    } else {
        args.prompt.join(" ")
    };
    if prompt.is_empty() && args.command.is_none() && args.goals.is_empty() {
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
    setup::validate(&args, global)?;
    let timeout = args.timeout.as_deref().map(parse_duration).transpose()?;
    let format = global.format.unwrap_or(Format::Text);
    if !matches!(format, Format::Text | Format::Json | Format::StreamJson) {
        return Err(CliError::usage(
            "exec supports --format text, json or stream-json",
        ));
    }
    if let Some(warning) = super::upgrade::unsupported_warning(ctx) {
        eprintln!("{warning}");
    }
    let code = super::serve::runtime()?.block_on(async {
        let backend = setup::backend(&args, ctx).await?;
        let client = backend.client.at(&ctx.location.display().to_string());
        style::check(&client, &args).await?;
        let session = setup::session(&client, &args, global).await?;
        let client = if args.worktree.is_some() {
            client.at(session["directory"]
                .as_str()
                .ok_or_else(|| CliError::runtime("Worktree Session has no Location"))?)
        } else {
            client
        };
        let mut out = Out::new(format, &args, &session);
        let code = match (review, &args.workflow) {
            (_, Some(target)) => {
                workflow::execute(&client, ctx, &session, &args, target, out).await
            }
            (Some(target), None) if review_run::uses_workflow(&client).await => {
                review_run::execute(&client, ctx, &session, &args, target, out).await
            }
            (Some(target), None) => {
                out.review(target);
                execute_session(&client, &session, &args, prompt, timeout, out).await
            }
            (None, None) => execute(&client, &session, &args, prompt, timeout, out).await,
        };
        if let Some(id) = session["id"].as_str() {
            let source = backend.client.at(&ctx.location.display().to_string());
            if args.worktree.is_some() {
                crate::commands::worktrees::cleanup(&source, id).await;
            } else {
                crate::commands::worktrees::cleanup_entered(&source, id).await;
            }
        }
        code
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
    let (message, goal_ids) = match goals::submit(client, id, args, prompt).await? {
        Ok(sent) => sent,
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
        goals: !goal_ids.is_empty(),
    };
    follow.run(&mut events, timeout).await;
    goals::settle(client, id, &goal_ids, &mut run).await;
    if out.is_review() {
        match owned.fork_job(client).await {
            Ok(job) => out.review_job(owned.fork_id(), job),
            Err(error) => {
                run.error.get_or_insert(error.message);
            }
        }
    }
    if let Err(error) = refresh_usage(client, id, &mut run, FINAL_REFRESH).await {
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
    /// `--goal`: idle is final only once no Goal is active.
    goals: bool,
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
        self.run.set_timeout(timeout);
        let deadline = tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(u64::MAX / 4)));
        tokio::pin!(deadline);
        // One listener for the whole run: a `ctrl_c()` made anew in each `select!` exists
        // only while that `select!` waits, so a SIGINT arriving while an event is handled
        // has no receiver and is lost.
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
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
                _ = &mut interrupt, if self.run.stop_requested.is_none() => {
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
            Ok(Settle::Done) if self.goals && !stopping => {
                match goals::unfinished(self.client, self.id).await {
                    Ok(unfinished) => !unfinished,
                    Err(error) => {
                        self.run.error = Some(error.message);
                        true
                    }
                }
            }
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
    // The refresh only enforces a budget; without one, a slow server must not end the run.
    if !run.has_budget() {
        return false;
    }
    if let Err(error) = refresh_usage(client, id, run, MONITOR_REFRESH).await {
        run.error = Some(error.message);
        stop(client, id, run, owned, "error").await;
    } else if run.over_budget() {
        stop(client, id, run, owned, "budget_exceeded").await;
    }
    false
}

/// A periodic budget refresh that takes longer stops the run: the budget is unverifiable.
const MONITOR_REFRESH: Duration = Duration::from_secs(2);
/// The one final usage read after the run, bounded but tolerant of a loaded server.
const FINAL_REFRESH: Duration = Duration::from_secs(15);

async fn refresh_usage(
    client: &Client,
    id: &str,
    run: &mut Run,
    timeout: Duration,
) -> Result<(), CliError> {
    let snapshot = tokio::time::timeout(timeout, client.get(&format!("/sessions/{id}")))
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
        "session.retry" => out.retry(&event.data),
        "session.deleted" => {
            run.error = Some("the Session was deleted".into());
            return Step::End;
        }
        "permission.asked.1" => decline(client, &event.data).await,
        kind => {
            out.durable(run, kind, &event.data);
            if kind.starts_with("session.step.ended") && run.over_budget() {
                stop(client, id, run, owned, "budget_exceeded").await;
            } else if run.waits_past_timeout(kind, &event.data) {
                stop(client, id, run, owned, "timeout").await;
            }
        }
    }
    Step::Continue
}

/// What the model reads when exec declines a request nobody can answer.
pub(crate) const NO_APPROVER: &str = "no interactive approver is attached to cyber exec";

/// `exec-mode` → non-interactive runs: nobody answers a prompt, so a request the
/// Session still raises (a sandbox ceiling such as `sandbox.escalate`, a network ask
/// in `bypass`, or any ask in `--mode default`) is denied at once instead of
/// holding the run open forever. The reply carries a message, so the model reads
/// the denial and the Drain continues.
async fn decline(client: &Client, data: &Value) {
    let (Some(session), Some(request)) = (data["session_id"].as_str(), data["id"].as_str()) else {
        return;
    };
    if !safe_id(session, "ses") || !safe_id(request, "per") {
        return;
    }
    let body = json!({ "reply": "reject", "message": NO_APPROVER });
    let path = format!("/sessions/{session}/permissions/{request}/reply");
    if let Err(error) = client.post(&path, body).await {
        eprintln!("could not decline permission request {request}: {error}");
    }
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

    /// Regression: with no `--max-tokens` or `--max-cost`, one billing refresh slower
    /// than its timeout on a loaded server interrupted a healthy run with
    /// "Exec billing refresh timed out".
    #[tokio::test]
    async fn an_unbudgeted_run_is_not_stopped_by_a_slow_billing_refresh() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            args: ExecArgs,
        }
        // Accepts and never answers: every request times out.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let client = Client::http(&url, None);
        let mut owned = Owned::empty("ses_1");
        let args = Wrapper::parse_from(["exec", "hi"]).args;
        let mut run = Run::new(&args, Instant::now());
        let monitored = tokio::time::timeout(
            Duration::from_secs(5),
            monitor_billing(&client, "ses_1", &mut run, &mut owned),
        );
        assert!(!monitored.await.expect("the monitor waited on the server"));
        assert!(run.error.is_none(), "{:?}", run.error);
        assert!(run.stop_requested.is_none());

        let budgeted = Wrapper::parse_from(["exec", "--max-tokens", "10", "hi"]).args;
        let mut run = Run::new(&budgeted, Instant::now());
        let refresh = refresh_usage(&client, "ses_1", &mut run, Duration::from_millis(200));
        assert_eq!(
            refresh.await.unwrap_err().message,
            "Exec billing refresh timed out"
        );
        server.abort();
    }
}
