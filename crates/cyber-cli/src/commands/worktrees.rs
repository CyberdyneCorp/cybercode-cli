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
    /// Remove a managed worktree and its branch.
    Remove {
        name: String,
        /// Remove even with uncommitted changes or commits beyond its base.
        #[arg(long)]
        force: bool,
    },
    /// Remove clean managed worktrees older than a number of days.
    Prune {
        #[arg(long, value_name = "DAYS", default_value_t = 14)]
        older_than: u64,
    },
}

pub fn run(
    cmd: WorktreeCmd,
    ctx: &crate::context::Context,
    global: &crate::cli::GlobalArgs,
) -> Result<(), CliError> {
    if let WorktreeCmd::Remove { name, .. } = &cmd {
        cyber_core::worktrees::Name::parse(name).map_err(CliError::usage)?;
    }
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        match cmd {
            WorktreeCmd::List => list(&client, global).await,
            WorktreeCmd::Remove { name, force } => remove(&client, global, &name, force).await,
            WorktreeCmd::Prune { older_than } => prune(&client, global, older_than).await,
        }
    })
}

fn api(error: cyber_client::ClientError) -> CliError {
    CliError::runtime(error.to_string())
}

async fn list(client: &Client, global: &crate::cli::GlobalArgs) -> Result<(), CliError> {
    let result = client.get("/worktrees").await.map_err(api)?;
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

async fn remove(
    client: &Client,
    global: &crate::cli::GlobalArgs,
    name: &str,
    force: bool,
) -> Result<(), CliError> {
    let path = format!(
        "/worktrees/{name}{}",
        if force { "?force=true" } else { "" }
    );
    let result = client.delete(&path).await.map_err(api)?;
    if crate::output::is_json(global.format) {
        return crate::output::json(&result["data"]);
    }
    let worktree = &result["data"]["worktree"];
    println!(
        "removed {}  {}  {}",
        worktree["name"].as_str().unwrap_or(name),
        worktree["path"].as_str().unwrap_or("?"),
        worktree["branch"].as_str().unwrap_or("?")
    );
    Ok(())
}

/// Ready, clean, not ahead and created before `cutoff_ms`. Removal re-checks
/// cleanliness and refuses a checkout a running Session uses.
fn prunable(entry: &Value, cutoff_ms: i64) -> Option<&str> {
    let clean = entry["status"] == "ready"
        && entry["dirty"] == false
        && entry["ahead"].as_u64() == Some(0)
        && entry["created_ms"]
            .as_i64()
            .is_some_and(|ms| ms < cutoff_ms);
    clean.then(|| entry["worktree"]["name"].as_str()).flatten()
}

async fn prune(
    client: &Client,
    global: &crate::cli::GlobalArgs,
    days: u64,
) -> Result<(), CliError> {
    let listed = client.get("/worktrees").await.map_err(api)?;
    let age = i64::try_from(days.saturating_mul(86_400_000)).unwrap_or(i64::MAX);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    let cutoff = now.saturating_sub(age);
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    let entries = listed["data"].as_array().cloned().unwrap_or_default();
    for name in entries.iter().filter_map(|entry| prunable(entry, cutoff)) {
        match client.delete(&format!("/worktrees/{name}")).await {
            Ok(_) => removed.push(name.to_owned()),
            Err(error) => skipped.push(json!({"name": name, "error": error.to_string()})),
        }
    }
    if crate::output::is_json(global.format) {
        return crate::output::json(&json!({"removed": removed, "skipped": skipped}));
    }
    for name in &removed {
        println!("removed {name}");
    }
    for entry in &skipped {
        eprintln!(
            "kept {}: {}",
            entry["name"].as_str().unwrap_or("?"),
            entry["error"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}

/// The exit cleanup of a Session started in a managed worktree: a clean checkout is
/// removed with its branch, a kept one is reported with its path and branch.
pub async fn cleanup(client: &Client, session_id: &str) {
    match client
        .post(
            &format!("/sessions/{session_id}/worktree/cleanup"),
            json!({}),
        )
        .await
    {
        Ok(result) => {
            let data = &result["data"];
            if data["kept"] != false {
                eprintln!(
                    "worktree kept at {} (branch {}): {}",
                    data["worktree"]["path"].as_str().unwrap_or("?"),
                    data["worktree"]["branch"].as_str().unwrap_or("?"),
                    data["reason"].as_str().unwrap_or("kept")
                );
            }
        }
        Err(error) => eprintln!("worktree cleanup failed; the worktree is kept: {error}"),
    }
}

/// The exit cleanup of a Session that ends in a worktree it entered with
/// `enter_worktree`: it returns to the Location it entered from, and a clean checkout it
/// created is removed. Nothing happens for a Session that is not in an entered worktree.
pub async fn cleanup_entered(client: &Client, session_id: &str) {
    let entered = client
        .get(&format!("/sessions/{session_id}"))
        .await
        .is_ok_and(|session| session["data"]["worktree_origin"].is_string());
    if entered {
        cleanup(client, session_id).await;
    }
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
