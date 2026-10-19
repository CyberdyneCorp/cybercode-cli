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

pub fn validate(args: &ExecArgs, global: &GlobalArgs) -> Result<(), CliError> {
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
    if args.dangerously_bypass_permissions && !(args.embedded || args.ephemeral) {
        return Err(CliError::usage(
            "--dangerously-bypass-permissions needs --embedded or --ephemeral: a shared server cannot take it for one run",
        ));
    }
    if args.dangerously_skip_permissions {
        skip_permissions(
            global.mode.as_deref(),
            args.embedded || args.ephemeral,
            "--embedded or --ephemeral",
        )?;
    }
    if args.trust_project_hooks && !(args.embedded || args.ephemeral) {
        return Err(CliError::usage(
            "--trust-project-hooks needs --embedded or --ephemeral: a shared server cannot take an approval for one run",
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
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: isolation(args.bypass_override()),
        })
        .await
        .map_err(CliError::runtime)?;
        if args.trust_project_hooks {
            app.host
                .attach_invocation_hook_trust(invocation_hook_trust(ctx)?)
                .map_err(CliError::runtime)?;
        }
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

/// The `bypass` gate inputs of an in-process server: the probe alone, or with the
/// explicit `--dangerously-bypass-permissions`.
pub(crate) fn isolation(dangerously: bool) -> Option<cyber_tools::IsolationOptions> {
    dangerously.then(|| cyber_tools::IsolationOptions::probe(true))
}

/// The gates of `--dangerously-skip-permissions` (exec and the TUI): it selects `bypass`,
/// so another `--mode` contradicts it, and it carries the no-sandbox override, which only
/// an in-process server can take for one run.
pub(crate) fn skip_permissions(
    mode: Option<&str>,
    in_process: bool,
    flags: &str,
) -> Result<(), CliError> {
    if let Some(mode) = mode.filter(|mode| *mode != "bypass") {
        return Err(CliError::usage(format!(
            "--dangerously-skip-permissions selects bypass mode and cannot be combined with --mode {mode}"
        )));
    }
    if !in_process {
        return Err(CliError::usage(format!(
            "--dangerously-skip-permissions needs {flags}: a shared server cannot take the no-sandbox override for one run"
        ))
        .with_hint(
            "add it for a private server, or use --mode bypass where the shared server's sandbox or a container already admits bypass",
        ));
    }
    Ok(())
}

/// Approve the project/local handler digests resolved now, for this run only.
fn invocation_hook_trust(
    ctx: &Context,
) -> Result<cyber_core::trust::HookInvocationTrust, CliError> {
    let resolved = ctx.config()?;
    if !resolved.trust.trusted {
        eprintln!(
            "warning: --trust-project-hooks approves no withheld hooks; approve the checkout configuration with `cyber trust` first"
        );
    }
    let catalog =
        cyber_core::hooks::HookCatalog::from_config(&resolved).map_err(CliError::usage)?;
    let digests: Vec<String> = catalog
        .definitions
        .iter()
        .filter(|definition| definition.scope.requires_handler_trust())
        .map(|definition| definition.digest.clone())
        .collect();
    if !digests.is_empty() {
        eprintln!(
            "approved {} project hook(s) for this run only",
            digests.len()
        );
    }
    Ok(cyber_core::trust::HookInvocationTrust::new(
        &resolved.trust.checkout_root,
        &digests,
    )?)
}

fn mode(args: &ExecArgs, global: &GlobalArgs) -> String {
    if args.dangerously_skip_permissions {
        eprintln!(
            "warning: --dangerously-skip-permissions runs every action without approval or OS confinement (bypass mode)"
        );
        return "bypass".into();
    }
    if args.yolo {
        eprintln!("warning: --yolo runs every action without approval (bypass mode)");
        return "bypass".into();
    }
    if args.auto {
        return "auto".into();
    }
    global.mode.clone().unwrap_or_else(|| "dont-ask".into())
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
            "title": args.name, "rules": cyber_core::config::never_ask_rules(), "max_steps": args.max_turns.filter(|_| args.goals.is_empty()),
            "messaging": inbound(args),
        });
        if let Some(name) = &args.worktree {
            return crate::commands::worktrees::start(client, name, body, args.quiet).await;
        }
        let created = client.post("/sessions", body).await.map_err(api)?;
        return Ok(created["data"].clone());
    };
    let mut id = existing["id"].as_str().unwrap_or_default().to_string();
    if args.fork {
        id = fork(client, &id, args).await?;
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
    if args.yolo || args.dangerously_skip_permissions || args.auto || global.mode.is_some() {
        client
            .post(
                &format!("/sessions/{id}/mode"),
                json!({ "mode": mode(args, global) }),
            )
            .await
            .map_err(api)?;
    }
    let s = client.get(&format!("/sessions/{id}")).await.map_err(api)?;
    if let Some(warning) = reused_inbound_warning(&s["data"], args.accept_messages)
        && !args.fork
        && !args.quiet
    {
        eprintln!("warning: {warning}");
    }
    Ok(s["data"].clone())
}

/// Fork `source`: the fork is a Session exec creates, so it takes this run's messaging
/// policy instead of the one it copied from its source.
async fn fork(client: &Client, source: &str, args: &ExecArgs) -> Result<String, CliError> {
    let forked = client
        .post(&format!("/sessions/{source}/fork"), json!({}))
        .await
        .map_err(api)?;
    let id = forked["data"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let messaging = match inbound(args) {
        Value::Null => json!({ "inbound": null }),
        refuse => refuse,
    };
    client
        .patch(
            &format!("/sessions/{id}"),
            json!({ "messaging": messaging }),
        )
        .await
        .map_err(api)?;
    Ok(id)
}

/// The `messaging` override of a Session exec creates: it refuses messages from other
/// Sessions (`non_interactive`) unless `--accept-messages`.
fn inbound(args: &ExecArgs) -> Value {
    if args.accept_messages {
        Value::Null
    } else {
        json!({ "inbound": "refuse", "reason": "non_interactive" })
    }
}

/// A reused Session keeps its messaging policy; say so when it differs from what this
/// run asked for.
fn reused_inbound_warning(session: &Value, accept_messages: bool) -> Option<String> {
    let id = session["id"].as_str().unwrap_or_default();
    let refuses = session["messaging"]["inbound"] == "refuse";
    match (accept_messages, refuses) {
        (true, true) => Some(format!(
            "--accept-messages cannot change the reused Session {id}; it keeps refusing messages \
             (clear its override with PATCH /api/v1/sessions/{id})"
        )),
        (false, false) => Some(format!(
            "the reused Session {id} keeps its messaging policy and may receive messages from \
             other Sessions; exec refuses them only in the Sessions it creates"
        )),
        _ => None,
    }
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
    crate::commands::sessions::resolve(client, wanted)
        .await
        .map(Some)
}

/// The admission request: the prompt, or `--command` with the prompt as its
/// arguments. Built before sending so a refusal keeps the server's typed error.
pub fn request(id: &str, args: &ExecArgs, prompt: String) -> Result<(String, Value), CliError> {
    if let Some(name) = &args.command {
        return Ok((
            format!("/sessions/{id}/command"),
            json!({ "name": name.trim_start_matches('/'), "arguments": prompt }),
        ));
    }
    let mut parts = vec![json!({ "type": "text", "text": prompt })];
    for file in &args.files {
        parts.push(file_part(file)?);
    }
    Ok((format!("/sessions/{id}/prompt"), json!({ "parts": parts })))
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

pub(super) fn urlencode(s: &str) -> String {
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

pub(super) fn api(e: cyber_client::ClientError) -> CliError {
    match e.tag() {
        Some("InvalidRequestError") => CliError::usage(e.to_string()),
        _ => CliError::runtime(e.to_string()),
    }
}
