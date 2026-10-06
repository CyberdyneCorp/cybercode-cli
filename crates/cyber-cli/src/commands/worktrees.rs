//! Managed worktree startup shared by exec and TUI clients.

use std::io::Write;

use base64::Engine as _;
use cyber_client::{Client, Event};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::error::CliError;

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
