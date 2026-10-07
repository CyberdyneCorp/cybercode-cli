//! Exec owns durable admission before waiting for a child Job.
mod admission;
use super::{
    ExecArgs, api,
    report::{Out, Run},
    setup,
};
use crate::error::CliError;
use cyber_client::Client;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) async fn target(
    client: &Client,
    args: &ExecArgs,
    text: &str,
) -> Result<Option<(String, String)>, CliError> {
    if args.command.is_some() {
        return Ok(None);
    }
    let Some((agent, prompt)) = cyber_core::agent_mentions::parse(text) else {
        return Ok(None);
    };
    let catalogue = client.get("/agents").await.map_err(api)?;
    let eligible = catalogue["data"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|profile| {
            profile["name"] == agent && matches!(profile["mode"].as_str(), Some("subagent" | "all"))
        });
    if !eligible {
        return Ok(None);
    }
    if prompt.trim().is_empty() {
        return Err(CliError::usage(format!("Usage: @{agent} <prompt>")));
    }
    Ok(Some((agent, prompt.into())))
}

struct Scope {
    parent: String,
    job: String,
    child: String,
}
impl Scope {
    fn new(parent: &str, job: &Value) -> Result<Self, CliError> {
        let scope = Self {
            parent: parent.into(),
            job: job["id"].as_str().unwrap_or_default().into(),
            child: job["child_id"].as_str().unwrap_or_default().into(),
        };
        if !safe_id(&scope.job, "job") || !safe_id(&scope.child, "ses") || scope.child == parent {
            return Err(CliError::runtime("Invalid delegated Job identity"));
        }
        scope.check(job)?;
        Ok(scope)
    }
    fn check(&self, job: &Value) -> Result<(), CliError> {
        if job["id"] != self.job
            || job["session_id"] != self.parent
            || job["child_id"] != self.child
        {
            return Err(CliError::runtime("Delegated Job ownership changed"));
        }
        match job["status"].as_str() {
            Some("running" | "completed" | "error" | "cancelled" | "interrupted") => Ok(()),
            _ => Err(CliError::runtime("Invalid delegated Job status")),
        }
    }
    async fn snapshot(&self, client: &Client) -> Result<Value, CliError> {
        let response = client
            .get(&format!("/sessions/{}", self.child))
            .await
            .map_err(api)?;
        let session = response["data"].clone();
        if session["id"] != self.child
            || session["parent_id"] != self.parent
            || serde_json::from_value::<cyber_server::runtime::Totals>(session["totals"].clone())
                .is_err()
        {
            return Err(CliError::runtime("Invalid delegated child snapshot"));
        }
        Ok(session)
    }
}
fn safe_id(id: &str, prefix: &str) -> bool {
    cyber_core::ids::has_prefix(id, prefix)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

enum End {
    Settled(Value),
    Budget,
    Stopped(&'static str),
}

pub(super) async fn execute(
    client: &Client,
    parent: &Value,
    args: &ExecArgs,
    agent: String,
    prompt: String,
    timeout: Option<Duration>,
    mut out: Out,
) -> Result<u8, CliError> {
    let parent_id = parent["id"]
        .as_str()
        .ok_or_else(|| CliError::runtime("Session has no ID"))?;
    let attachments = args
        .files
        .iter()
        .map(|path| setup::file_part(path))
        .collect::<Result<Vec<_>, _>>()?;
    negotiate(client, !attachments.is_empty(), args.max_turns.is_some()).await?;
    let admission = admission::Admission::new(parent_id)?;
    out.admission(parent_id, &admission.id);
    let body =
        json!({"agent":agent,"prompt":prompt,"attachments":attachments,"max_steps":args.max_turns});
    let mut scope = None;
    let mut run = Run::new(args, Instant::now());
    let mut cursor = -1;
    let end = {
        let work = async {
            let owned = admission.start(client, body).await?;
            out.delegated(&owned.parent, &owned.job, &owned.child);
            scope = Some(owned);
            follow(
                client,
                scope.as_ref().expect("admitted scope"),
                &mut cursor,
                &mut run,
                &mut out,
            )
            .await
        };
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(u64::MAX / 4))) => {
                Ok(End::Stopped("timeout"))
            },
            _ = tokio::signal::ctrl_c() => Ok(End::Stopped("interrupted")),
        }
    };
    match end {
        Ok(End::Settled(job)) => apply_job(&mut run, &job),
        Ok(End::Budget) => {
            run.stop_reason = Some("budget_exceeded".into());
            stop_owned(client, &admission, &mut scope, &mut out, &mut run).await;
        }
        Ok(End::Stopped(reason)) => {
            run.stop_reason = Some(reason.into());
            stop_owned(client, &admission, &mut scope, &mut out, &mut run).await;
        }
        Err(error) => {
            run.error = Some(error.message);
            stop_owned(client, &admission, &mut scope, &mut out, &mut run).await;
        }
    }
    if let Some(scope) = &scope {
        catch_up(client, scope, &mut cursor, &mut run, &mut out).await;
    }
    if let Some(error) = &run.error {
        out.error(error);
    }
    Ok(out.finish(&run, args, parent))
}
async fn stop_owned(
    client: &Client,
    admission: &admission::Admission,
    scope: &mut Option<Scope>,
    out: &mut Out,
    run: &mut Run,
) {
    if let Some(scope) = scope {
        cancel(client, scope, run).await;
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(3), admission.stop(client, scope)).await;
    if let Some(owned) = scope {
        out.delegated(&owned.parent, &owned.job, &owned.child);
    }
    let detail = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error.message,
        Err(_) => "Cancellation acknowledgement timed out".into(),
    };
    let prior = run
        .error
        .take()
        .map_or(String::new(), |error| format!("{error}; "));
    run.error = Some(format!(
        "{prior}Delegation cancellation was not acknowledged; inspect {}: {detail}",
        admission.id
    ));
}

async fn catch_up(client: &Client, scope: &Scope, cursor: &mut i64, run: &mut Run, out: &mut Out) {
    // Terminal acknowledgement precedes the final catch-up: no early response can
    // hide a late tool denial, usage event or completed assistant result.
    let final_read = tokio::time::timeout(Duration::from_secs(3), async {
        history(client, scope, cursor, run, out).await?;
        let snapshot = scope.snapshot(client).await?;
        run.totals(&snapshot["totals"]);
        Ok::<_, CliError>(())
    })
    .await;
    let failure = match final_read {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.message),
        Err(_) => Some("Final delegated output read timed out".into()),
    };
    if let Some(failure) = failure {
        run.error = Some(match run.error.take() {
            Some(original) => format!("{original}; {failure}"),
            None => failure,
        });
    }
}

async fn negotiate(client: &Client, attachments: bool, steps: bool) -> Result<(), CliError> {
    let document = client.get("/openapi.json").await.map_err(api)?;
    let path = "/api/v1/sessions/{sessionID}/delegations/{requestID}";
    if document["paths"][path]["post"].is_null()
        || document["paths"][path]["get"].is_null()
        || document["paths"][format!("{path}/stop")]["post"].is_null()
    {
        return Err(
            CliError::usage("Server does not support durable delegation admission")
                .with_hint("Update the attached server before using named delegation"),
        );
    }
    let properties = &document["components"]["schemas"]["SubtaskBody"]["properties"];
    for (field, needed) in [
        ("agent", true),
        ("attachments", attachments),
        ("max_steps", steps),
    ] {
        if needed && properties.get(field).is_none() {
            return Err(
                CliError::usage(format!("Server does not support delegated {field}"))
                    .with_hint("Update the attached server before using named delegation"),
            );
        }
    }
    Ok(())
}

async fn follow(
    client: &Client,
    scope: &Scope,
    cursor: &mut i64,
    run: &mut Run,
    out: &mut Out,
) -> Result<End, CliError> {
    let initial = scope.snapshot(client).await?;
    out.init(client, &initial).await;
    loop {
        let response = client
            .get(&format!("/jobs/{}", scope.job))
            .await
            .map_err(api)?;
        let job = &response["data"];
        scope.check(job)?;
        history(client, scope, cursor, run, out).await?;
        let snapshot = scope.snapshot(client).await?;
        run.totals(&snapshot["totals"]);
        if run.over_budget() {
            return Ok(End::Budget);
        }
        if job["status"] != "running" {
            return Ok(End::Settled(job.clone()));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
async fn history(
    client: &Client,
    scope: &Scope,
    cursor: &mut i64,
    run: &mut Run,
    out: &mut Out,
) -> Result<(), CliError> {
    loop {
        let page = client
            .get(&format!(
                "/sessions/{}/history?after={cursor}&limit=500",
                scope.child
            ))
            .await
            .map_err(api)?;
        let events = page["data"]
            .as_array()
            .ok_or_else(|| CliError::runtime("Invalid child history"))?;
        for event in events {
            let seq = event["durable"]["seq"]
                .as_i64()
                .ok_or_else(|| CliError::runtime("Child event has no cursor"))?;
            if event["durable"]["aggregateID"] != scope.child || seq <= *cursor {
                return Err(CliError::runtime(
                    "Child history identity or cursor changed",
                ));
            }
            *cursor = seq;
            out.durable(
                run,
                event["type"].as_str().unwrap_or_default(),
                &event["data"],
            );
        }
        match page["hasMore"].as_bool() {
            Some(false) => return Ok(()),
            Some(true) => {}
            None => return Err(CliError::runtime("Child history has no pagination state")),
        }
        if events.is_empty() {
            return Err(CliError::runtime("Child history page did not advance"));
        }
    }
}
async fn cancel(client: &Client, scope: &Scope, run: &mut Run) {
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let response = client
            .post(&format!("/jobs/{}/stop", scope.job), json!({}))
            .await
            .map_err(api)?;
        scope.check(&response["data"])?;
        if response["data"]["status"] == "running" {
            return Err(CliError::runtime("Job is still running"));
        }
        Ok::<_, CliError>(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        let original = run.error.take().unwrap_or_default();
        run.error = Some(format!(
            "{original}{}Delegated Job cancellation was not acknowledged; inspect {}",
            if original.is_empty() { "" } else { "; " },
            scope.job
        ));
    }
}
fn apply_job(run: &mut Run, job: &Value) {
    if job["status"] != "completed" {
        run.error = Some(
            job["error"]
                .as_str()
                .unwrap_or("Delegated task did not complete")
                .into(),
        );
        if job["status"] == "interrupted" {
            run.stop_reason = Some("interrupted".into());
        }
    }
}
