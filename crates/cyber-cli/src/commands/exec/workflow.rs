//! `cyber exec --workflow <name|path> --args <json>` (`exec-mode` → Workflow exec): start a
//! Workflow Run owned by the exec Session, report `workflow_progress` while it runs, and
//! print its return value as the result. Exit codes: 0 completed, 1 failed or stopped,
//! 4 budget exceeded, 130 interrupted (the run is stopped).

use std::time::{Duration, Instant};

use cyber_client::Client;
use serde_json::{Value, json};

use super::report::Out;
use super::{EXIT_INTERRUPTED, ExecArgs, api};
use crate::commands::workflows::{self, Progress, RunArgs};
use crate::context::Context;
use crate::error::CliError;

const POLL: Duration = Duration::from_millis(200);

/// The `cyber workflows run` arguments an exec run stands for.
fn run_args(args: &ExecArgs, target: &str) -> RunArgs {
    RunArgs {
        target: target.to_owned(),
        args: args.workflow_args.clone(),
        max_cost: args.max_cost,
        max_turns: args.max_turns.map(u64::from),
        max_tokens: args.max_tokens,
        timeout: args.timeout.clone(),
        max_agents: None,
        max_concurrent: None,
        runner: None,
        wait: true,
    }
}

/// Start the run for `session` and follow it until it ends.
pub(super) async fn execute(
    client: &Client,
    ctx: &Context,
    session: &Value,
    args: &ExecArgs,
    target: &str,
    out: Out,
) -> Result<u8, CliError> {
    let started = Instant::now();
    let id = session["id"].as_str().unwrap_or_default();
    let body = workflows::start_body(ctx, &run_args(args, target), id)?;
    out.init(client, session).await;
    let run = client.post("/workflows/runs", body).await.map_err(api)?;
    let run_id = run["data"]["id"].as_str().unwrap_or_default();
    let (out, run, code) = follow(client, run_id, args, out).await?;
    Ok(out.workflow_finish(&run, code, started, args, session))
}

/// Follow run `run_id` until it ends, reporting progress; returns the final run and its
/// exit code. Ctrl-C stops the run and exits 130.
pub(super) async fn follow(
    client: &Client,
    run_id: &str,
    args: &ExecArgs,
    out: Out,
) -> Result<(Out, Value, u8), CliError> {
    let path = format!("/workflows/runs/{run_id}");
    let mut follow = Follow {
        out,
        progress: Progress::default(),
        last: Value::Null,
        text: !args.quiet,
    };
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        let detail = tokio::select! {
            biased;
            _ = &mut interrupt => return Ok(follow.interrupted(client, &path).await),
            detail = client.get(&path) => detail.map_err(api)?,
        };
        let run = &detail["data"];
        follow.observe(run);
        if let Some(code) = workflows::ended(run) {
            return Ok((follow.out, run.clone(), code));
        }
        tokio::select! {
            biased;
            _ = &mut interrupt => return Ok(follow.interrupted(client, &path).await),
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

struct Follow {
    out: Out,
    progress: Progress,
    /// The last progress reported, so each change is reported once.
    last: Value,
    /// Progress lines on stderr (text format, not `--quiet`).
    text: bool,
}

impl Follow {
    fn observe(&mut self, run: &Value) {
        if self.text && self.out.is_text() {
            self.progress.report(run);
        }
        let progress = progress(run);
        if progress != self.last {
            self.out.workflow_progress(&progress);
            self.last = progress;
        }
    }

    /// Ctrl-C: stop the run and report it interrupted.
    async fn interrupted(self, client: &Client, path: &str) -> (Out, Value, u8) {
        let stopped = client.post(&format!("{path}/stop"), json!({})).await;
        let run = stopped
            .ok()
            .map(|stopped| stopped["data"].clone())
            .unwrap_or_else(|| json!({"status": "stopped"}));
        (self.out, run, EXIT_INTERRUPTED)
    }
}

/// A `workflow_progress` event's payload: the run's status, phase and agent counts.
fn progress(run: &Value) -> Value {
    json!({
        "run_id": run["id"],
        "name": run["name"],
        "status": run["status"],
        "phase": run["phase"],
        "agents": run["agents"],
        "queue_position": run["queue_position"],
        "tokens": run["tokens"],
        "cost_usd": run["cost_usd"],
    })
}
