//! `cyber review` through the bundled `review` Workflow (`workflows` → Bundled workflow
//! contracts): when the Location's library has a `review` Workflow and no `review` skill or
//! command overrides the bundled one, the review is a Workflow Run owned by the exec
//! Session. It exits 0 when the run completes, 4 when a budget stopped it (the partial
//! findings are printed) and 1 when it fails. Otherwise the bundled review skill runs as
//! before.

use std::time::Instant;

use cyber_client::Client;
use cyber_core::review::Target;
use serde_json::{Value, json};

use super::ExecArgs;
use super::report::{Out, Run};
use crate::commands::workflows::{self, RunArgs};
use crate::context::Context;
use crate::error::CliError;

/// Whether `cyber review` runs as the `review` Workflow here: the `review` command is the
/// built-in one and the library has a `review` Workflow.
pub(super) async fn uses_workflow(client: &Client) -> bool {
    let builtin = client.get("/commands").await.is_ok_and(|list| {
        list["data"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["name"] == "review" && c["source"] == "builtin")
    });
    builtin
        && client.get("/workflows").await.is_ok_and(|library| {
            library["workflows"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|entry| entry["name"] == "review")
        })
}

fn run_args(args: &ExecArgs, target: &Target) -> RunArgs {
    RunArgs {
        target: "review".into(),
        args: Some(json!({"target": target.normalized()}).to_string()),
        max_cost: args.max_cost,
        max_turns: args.max_turns.map(u64::from),
        max_tokens: args.max_tokens,
        timeout: args.timeout.clone(),
        wait: true,
        ..RunArgs::default()
    }
}

/// Start the `review` run for `session`, follow it and report its findings.
pub(super) async fn execute(
    client: &Client,
    ctx: &Context,
    session: &Value,
    args: &ExecArgs,
    target: Target,
    mut out: Out,
) -> Result<u8, CliError> {
    let started = Instant::now();
    out.review(target.clone());
    let id = session["id"].as_str().unwrap_or_default();
    let mut body = workflows::start_body(ctx, &run_args(args, &target), id)?;
    // Always the library entry, even when the Location has a file named `review`.
    body["name"] = json!("review");
    if let Some(object) = body.as_object_mut() {
        object.remove("script");
    }
    out.init(client, session).await;
    let run = match client.post("/workflows/runs", body).await {
        Ok(run) => run,
        Err(error) => return out.refused(error, &Run::new(args, started), args, session),
    };
    let run_id = run["data"]["id"].as_str().unwrap_or_default();
    let (out, run, code) = super::workflow::follow(client, run_id, args, out).await?;
    Ok(out.review_run_finish(&run, code, started, args, session))
}
