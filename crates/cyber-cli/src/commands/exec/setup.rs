//! Backend, Session selection and prompt assembly for `cyber exec`.

use std::io::{IsTerminal, Read};

use base64::Engine;
use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_core::paths::DatabaseLocation;
use serde_json::{Value, json};

use super::ExecArgs;
use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;

/// The client plus anything that must outlive it (an in-process server).
pub struct Backend {
    pub client: Client,
    _app: Option<App>,
}

/// Positional words joined by spaces, then stdin when it is not a terminal.
pub fn prompt(words: &[String]) -> Result<String, CliError> {
    let mut prompt = words.join(" ");
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut piped = String::new();
        stdin.lock().read_to_string(&mut piped)?;
        let piped = piped.trim_end();
        if !piped.is_empty() {
            prompt = if prompt.is_empty() {
                piped.to_string()
            } else {
                format!("{prompt}\n\n{piped}")
            };
        }
    }
    Ok(prompt)
}

pub fn validate(args: &ExecArgs) -> Result<(), CliError> {
    if args.worktree.is_some()
        && (args.ephemeral || args.resume_last || args.session.is_some() || args.fork)
    {
        return Err(CliError::usage(
            "--worktree starts a fresh persistent Session and cannot be combined with --ephemeral, --continue, --session or --fork",
        ));
    }
    if let Some(name) = args.worktree.as_deref().filter(|name| !name.is_empty()) {
        cyber_core::worktrees::Name::parse(name).map_err(CliError::usage)?;
    }
    if args.ephemeral && (args.resume_last || args.session.is_some() || args.fork) {
        return Err(CliError::usage(
            "--ephemeral cannot be combined with --continue, --session or --fork",
        ));
    }
    if args.fork && !(args.resume_last || args.session.is_some()) {
        return Err(CliError::usage("--fork needs --continue or --session"));
    }
    if args.attach.is_some() && (args.embedded || args.ephemeral) {
        return Err(CliError::usage(
            "--attach cannot be combined with --embedded or --ephemeral",
        ));
    }
    Ok(())
}

pub async fn backend(args: &ExecArgs, ctx: &Context) -> Result<Backend, CliError> {
    if let Some(url) = &args.attach {
        let password = args
            .password
            .clone()
            .or_else(|| std::env::var("CYBER_SERVER_PASSWORD").ok());
        return Ok(Backend {
            client: Client::http(url, password),
            _app: None,
        });
    }
    if args.embedded || args.ephemeral {
        let database = if args.ephemeral {
            DatabaseLocation::Memory
        } else {
            let dir = ctx.paths.data.join("embedded");
            std::fs::create_dir_all(&dir)?;
            DatabaseLocation::File(dir.join(format!(
                "{}.db",
                ulid::Ulid::new().to_string().to_lowercase()
            )))
        };
        let app = App::build(AppOptions {
            paths: ctx.paths.clone(),
            home: ctx.home.clone(),
            database,
            default_directory: ctx.location.clone(),
            sandbox_policy: args.sandbox.clone(),
            snapshots: !args.ephemeral,
            interactive: false,
            password: None,
        })
        .await
        .map_err(CliError::runtime)?;
        return Ok(Backend {
            client: Client::embedded(app.embedded()),
            _app: Some(app),
        });
    }
    let info = crate::commands::serve::start(ctx).await?;
    Ok(Backend {
        client: Client::http(&info.registration.url, Some(info.password)),
        _app: None,
    })
}

fn mode(args: &ExecArgs, global: &GlobalArgs) -> String {
    if args.yolo {
        eprintln!("warning: --yolo runs every action without approval (bypass mode)");
        return "bypass".into();
    }
    if args.auto {
        return "auto".into();
    }
    global.mode.clone().unwrap_or_else(|| "dont-ask".into())
}

/// Never wait for a human: deny the tools that ask.
fn never_ask() -> Value {
    json!({ "question": "deny", "plan_enter": "deny", "plan_exit": "deny" })
}

pub async fn session(
    client: &Client,
    args: &ExecArgs,
    global: &GlobalArgs,
) -> Result<Value, CliError> {
    if let Some(agent) = &args.agent {
        let agents = client.get("/agents").await.map_err(api)?;
        let known = agents["data"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|a| a["name"] == agent.as_str() && a["mode"] != "subagent");
        if !known {
            return Err(CliError::usage(format!(
                "unknown or subagent-only agent {agent:?}"
            )));
        }
    }
    let reused = reuse(client, args).await?;
    let Some(existing) = reused else {
        let body = json!({
            "model": global.model, "agent": args.agent, "mode": mode(args, global),
            "title": args.name, "rules": never_ask(), "max_steps": args.max_turns,
        });
        if let Some(name) = &args.worktree {
            return crate::commands::worktrees::start(client, name, body, args.quiet).await;
        }
        let created = client.post("/sessions", body).await.map_err(api)?;
        return Ok(created["data"].clone());
    };
    let mut id = existing["id"].as_str().unwrap_or_default().to_string();
    if args.fork {
        let forked = client
            .post(&format!("/sessions/{id}/fork"), json!({}))
            .await
            .map_err(api)?;
        id = forked["data"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    }
    if args.non_interactive_rules {
        eprintln!("warning: --non-interactive-rules cannot change a reused Session in this build");
    }
    if let Some(model) = &global.model {
        client
            .post(&format!("/sessions/{id}/model"), json!({ "model": model }))
            .await
            .map_err(api)?;
    }
    if args.yolo || args.auto || global.mode.is_some() {
        client
            .post(
                &format!("/sessions/{id}/mode"),
                json!({ "mode": mode(args, global) }),
            )
            .await
            .map_err(api)?;
    }
    let s = client.get(&format!("/sessions/{id}")).await.map_err(api)?;
    Ok(s["data"].clone())
}

async fn reuse(client: &Client, args: &ExecArgs) -> Result<Option<Value>, CliError> {
    if args.resume_last {
        let list = client.get("/sessions?limit=1").await.map_err(api)?;
        let first = list["data"]["data"]
            .get(0)
            .cloned()
            .ok_or_else(|| CliError::runtime("no Session to continue in this Location"))?;
        return Ok(Some(first));
    }
    let Some(wanted) = &args.session else {
        return Ok(None);
    };
    if let Ok(s) = client.get(&format!("/sessions/{wanted}")).await {
        return Ok(Some(s["data"].clone()));
    }
    let list = client
        .get(&format!("/sessions?limit=200&search={}", urlencode(wanted)))
        .await
        .map_err(api)?;
    let found = list["data"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["title"] == wanted.as_str())
        .cloned();
    found
        .map(Some)
        .ok_or_else(|| CliError::runtime("Session not found"))
}

/// Admit the prompt, or run `--command` with the prompt as its arguments.
pub async fn send(
    client: &Client,
    id: &str,
    args: &ExecArgs,
    prompt: String,
) -> Result<(), CliError> {
    if let Some(name) = &args.command {
        client
            .post(
                &format!("/sessions/{id}/command"),
                json!({ "name": name.trim_start_matches('/'), "arguments": prompt }),
            )
            .await
            .map_err(api)?;
        return Ok(());
    }
    let mut parts = vec![json!({ "type": "text", "text": prompt })];
    for file in &args.files {
        parts.push(file_part(file)?);
    }
    client
        .post(&format!("/sessions/{id}/prompt"), json!({ "parts": parts }))
        .await
        .map_err(api)?;
    Ok(())
}

pub(super) fn file_part(path: &std::path::Path) -> Result<Value, CliError> {
    let bytes = std::fs::read(path)
        .map_err(|_| CliError::usage(format!("--file {}: no such file", path.display())))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media = match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    };
    if let Some(media_type) = media {
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        return Ok(json!({ "type": "image", "media_type": media_type, "data": data }));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        CliError::usage(format!(
            "--file {}: not a text or image file",
            path.display()
        ))
    })?;
    Ok(
        json!({ "type": "text", "text": format!("<file path=\"{}\">\n{text}\n</file>", path.display()) }),
    )
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn api(e: cyber_client::ClientError) -> CliError {
    match e.tag() {
        Some("InvalidRequestError") => CliError::usage(e.to_string()),
        _ => CliError::runtime(e.to_string()),
    }
}
