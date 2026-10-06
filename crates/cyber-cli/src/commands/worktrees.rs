//! Managed worktree startup shared by exec and TUI clients.

use std::io::Write;

use base64::Engine as _;
use cyber_client::{Client, Event};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::error::CliError;

#[derive(Debug, clap::Subcommand)]
pub enum WorktreeCmd {
    /// List owned worktrees, Git status and their Sessions.
    List,
}

pub fn run(
    _cmd: WorktreeCmd,
    ctx: &crate::context::Context,
    global: &crate::cli::GlobalArgs,
) -> Result<(), CliError> {
    let rt = super::serve::runtime()?;
    let result = rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        client
            .get("/worktrees")
            .await
            .map_err(|error| CliError::runtime(error.to_string()))
    })?;
    if crate::output::is_json(global.format) {
        return crate::output::json(&result["data"]);
    }
    for entry in result["data"]
        .as_array()
        .ok_or_else(|| CliError::runtime("Invalid worktree list response"))?
    {
        println!("{}", listing_line(entry));
    }
    Ok(())
}

fn listing_line(entry: &Value) -> String {
    let status = entry["status"].as_str().unwrap_or("invalid");
    if status == "invalid" {
        return format!(
            "{}  invalid: {}",
            entry["name"].as_str().unwrap_or("?"),
            entry["message"].as_str().unwrap_or("Unknown error")
        );
    }
    let worktree = &entry["worktree"];
    let details = if status == "ready" {
        let sessions = entry["sessions"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["id"].as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        format!(
            "ahead={} behind={} {} sessions=[{}]",
            entry["ahead"].as_u64().unwrap_or(0),
            entry["behind"].as_u64().unwrap_or(0),
            if entry["dirty"].as_bool().unwrap_or(true) {
                "dirty"
            } else {
                "clean"
            },
            sessions
        )
    } else {
        "pending; recovery required".into()
    };
    format!(
        "{}  {}  {}  {}",
        worktree["name"].as_str().unwrap_or("?"),
        worktree["path"].as_str().unwrap_or("?"),
        worktree["branch"].as_str().unwrap_or("?"),
        details
    )
}

pub async fn start(
    client: &Client,
    name: &str,
    session: Value,
    quiet: bool,
) -> Result<Value, CliError> {
    let call_id = cyber_core::ids::new_id("call");
    let mut events = Box::pin(
        client
            .events(true)
            .await
            .map_err(|error| CliError::runtime(error.to_string()))?,
    );
    let request = client.post(
        "/worktrees",
        json!({
            "name": if name.is_empty() { None } else { Some(name) },
            "call_id": call_id, "session": session,
        }),
    );
    tokio::pin!(request);
    let result = loop {
        tokio::select! {
            result = &mut request => break result.map_err(|error| CliError::runtime(error.to_string()))?,
            event = events.next() => match event {
                Some(event) => output(&event, &call_id, quiet)?,
                None => break request.await.map_err(|error| CliError::runtime(error.to_string()))?,
            }
        }
    };
    if !quiet && result["data"]["setup"]["status"] != "completed" {
        eprintln!(
            "\nworktree setup failed; Session and files retained at {}: {}",
            result["data"]["worktree"]["path"]
                .as_str()
                .unwrap_or_default(),
            result["data"]["setup"]
        );
    }
    Ok(result["data"]["session"].clone())
}

fn output(event: &Event, call_id: &str, quiet: bool) -> Result<(), CliError> {
    if quiet || event.kind != "session.worktree.setup" || event.data["call_id"] != call_id {
        return Ok(());
    }
    if let Some(encoded) = event.data["update"]["base64"].as_str() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| CliError::runtime(error.to_string()))?;
        std::io::stderr().write_all(&bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_renders_status_sessions_and_recovery_diagnostics() {
        let worktree = json!({"name":"spike", "path":"/repo/spike", "branch":"cyber/spike"});
        let ready = listing_line(
            &json!({"status":"ready", "worktree":worktree, "ahead":2,"behind":1,"dirty":true,"sessions":[{"id":"ses_one"},{"id":"ses_child"}]}),
        );
        assert_eq!(
            ready,
            "spike  /repo/spike  cyber/spike  ahead=2 behind=1 dirty sessions=[ses_one,ses_child]"
        );
        assert!(
            listing_line(&json!({"status":"pending","worktree":worktree}))
                .contains("pending; recovery required")
        );
        assert_eq!(
            listing_line(&json!({"status":"invalid","name":"broken","message":"Invalid JSON"})),
            "broken  invalid: Invalid JSON"
        );
    }
}
