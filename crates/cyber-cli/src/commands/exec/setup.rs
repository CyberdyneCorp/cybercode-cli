//! Backend, Session selection and prompt assembly for `cyber exec`.

use std::io::{IsTerminal, Read};
use std::path::Path;

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
    super::style::validate(args)?;
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
            "messaging": inbound(args), "output_style": args.output_style,
            // `exec-mode` → Usage limit waits in exec: nobody attends this Session.
            "unattended": true,
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
    reselect(client, &id, args, global).await?;
    let s = client.get(&format!("/sessions/{id}")).await.map_err(api)?;
    if let Some(warning) = reused_inbound_warning(&s["data"], args.accept_messages)
        && !args.fork
        && !args.quiet
    {
        eprintln!("warning: {warning}");
    }
    Ok(s["data"].clone())
}

/// Apply this run's model, output style and mode to a reused Session.
async fn reselect(
    client: &Client,
    id: &str,
    args: &ExecArgs,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    if let Some(model) = &global.model {
        client
            .post(&format!("/sessions/{id}/model"), json!({ "model": model }))
            .await
            .map_err(api)?;
    }
    if let Some(style) = &args.output_style {
        client
            .post(
                &format!("/sessions/{id}/output-style"),
                json!({ "style": style }),
            )
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
    Ok(())
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
        parts.push(file_part(file, args.attach.is_none())?);
    }
    Ok((format!("/sessions/{id}/prompt"), json!({ "parts": parts })))
}

/// One `--file` part. An image or a PDF, judged by its first bytes, goes to a server on
/// this machine as a `file` part that the server reads under the `read` permission and
/// the run's mode; a remote server (`--attach`) gets the bytes inline. Anything else is
/// attached as text.
pub(super) fn file_part(path: &Path, local: bool) -> Result<Value, CliError> {
    let missing = || CliError::usage(format!("--file {}: no such file", path.display()));
    if std::fs::metadata(path).map_err(|_| missing())?.is_dir() {
        return Err(CliError::usage(format!(
            "--file {}: is a directory",
            path.display()
        )));
    }
    let name = path.file_name().map(|name| name.to_string_lossy());
    if let Some(media_type) = media_type(path)? {
        if local {
            let absolute = std::path::absolute(path)?;
            return Ok(json!({ "type": "file", "path": absolute, "name": name }));
        }
        let data = base64::engine::general_purpose::STANDARD.encode(std::fs::read(path)?);
        let kind = if media_type == "application/pdf" {
            "document"
        } else {
            "image"
        };
        return Ok(json!({ "type": kind, "media_type": media_type, "data": data, "name": name }));
    }
    let bytes = std::fs::read(path).map_err(|_| missing())?;
    let text = String::from_utf8(bytes).map_err(|_| {
        CliError::usage(format!(
            "--file {}: not a text, image or PDF file",
            path.display()
        ))
    })?;
    Ok(
        json!({ "type": "text", "text": format!("<file path=\"{}\">\n{text}\n</file>", path.display()) }),
    )
}

/// The image or PDF media type of the file, from its magic bytes.
fn media_type(path: &Path) -> Result<Option<&'static str>, CliError> {
    let mut head = Vec::with_capacity(16);
    std::fs::File::open(path)?.take(16).read_to_end(&mut head)?;
    Ok(cyber_server::runtime::attachments::sniff(&head))
}

/// The server tags of a prompt it refused before admitting it.
const REFUSALS: [&str; 5] = [
    "InvalidRequestError",
    "AttachmentTooLargeError",
    "UnsupportedAttachmentError",
    "UnsupportedModalityError",
    "PermissionDeniedError",
];

/// A refused prompt exits 2 with the server's message. A denied image or PDF outside the
/// Session's Location gets a hint to add its directory.
pub(super) fn refusal(
    error: cyber_client::ClientError,
    args: &ExecArgs,
    session: &Value,
) -> CliError {
    if !error.tag().is_some_and(|tag| REFUSALS.contains(&tag)) {
        return api(error);
    }
    let refused = CliError::usage(error.to_string());
    if error.tag() != Some("PermissionDeniedError") {
        return refused;
    }
    let location = session["directory"].as_str().map(std::fs::canonicalize);
    let Some(Ok(location)) = location else {
        return refused;
    };
    match args
        .files
        .iter()
        .find_map(|file| outside_dir(file, &location))
    {
        Some(dir) => refused.with_hint(format!("add --add-dir {}", dir.display())),
        None => refused,
    }
}

/// The directory of an image or PDF `--file` that lies outside `location`.
fn outside_dir(file: &Path, location: &Path) -> Option<std::path::PathBuf> {
    let canonical = std::fs::canonicalize(file).ok()?;
    if canonical.starts_with(location) || !matches!(media_type(file), Ok(Some(_))) {
        return None;
    }
    std::path::absolute(file)
        .ok()?
        .parent()
        .map(Path::to_path_buf)
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

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn media_files_are_sniffed_and_sent_by_path_or_inline() {
        let dir = tempfile::tempdir().unwrap();
        // The extension does not decide: the bytes do.
        let shot = dir.path().join("shot.txt");
        std::fs::write(&shot, PNG).unwrap();
        let local = file_part(&shot, true).unwrap();
        assert_eq!(local["type"], "file");
        assert_eq!(local["name"], "shot.txt");
        assert!(Path::new(local["path"].as_str().unwrap()).is_absolute());
        let inline = file_part(&shot, false).unwrap();
        assert_eq!(inline["type"], "image");
        assert_eq!(inline["media_type"], "image/png");
        let data = base64::engine::general_purpose::STANDARD
            .decode(inline["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(data, PNG);

        let pdf = dir.path().join("spec.pdf");
        std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
        assert_eq!(file_part(&pdf, false).unwrap()["type"], "document");
        assert_eq!(file_part(&pdf, true).unwrap()["type"], "file");

        let note = dir.path().join("note.png");
        std::fs::write(&note, "plain text").unwrap();
        let text = file_part(&note, true).unwrap();
        assert_eq!(text["type"], "text");
        assert!(
            text["text"]
                .as_str()
                .unwrap()
                .ends_with("plain text\n</file>")
        );
    }

    #[test]
    fn directories_missing_and_binary_files_are_usage_errors() {
        let dir = tempfile::tempdir().unwrap();
        let error = file_part(dir.path(), true).unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        assert!(
            error.message.ends_with("is a directory"),
            "{}",
            error.message
        );
        let missing = file_part(&dir.path().join("gone.png"), true).unwrap_err();
        assert!(missing.message.ends_with("no such file"));
        let zip = dir.path().join("a.zip");
        std::fs::write(&zip, b"PK\x03\x04\xff\xfe").unwrap();
        let binary = file_part(&zip, true).unwrap_err();
        assert!(binary.message.ends_with("not a text, image or PDF file"));
    }

    #[test]
    fn attachment_refusals_exit_2_and_a_denied_outside_file_gets_the_add_dir_hint() {
        let root = tempfile::tempdir().unwrap();
        let (location, outside) = (root.path().join("repo"), root.path().join("shots"));
        std::fs::create_dir_all(&location).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let shot = outside.join("shot.png");
        std::fs::write(&shot, PNG).unwrap();
        let session = json!({ "directory": location });
        let args = ExecArgs {
            files: vec![shot],
            ..ExecArgs::default()
        };
        let api_error = |tag: &str| cyber_client::ClientError::Api {
            status: 400,
            tag: tag.into(),
            message: "refused".into(),
            body: Value::Null,
        };
        let denied = refusal(api_error("PermissionDeniedError"), &args, &session);
        assert_eq!(denied.code, crate::error::EXIT_USAGE);
        assert_eq!(denied.message, "PermissionDeniedError: refused");
        let hint = denied.hint.unwrap();
        assert!(hint.starts_with("add --add-dir "), "{hint}");
        assert!(hint.ends_with("shots"), "{hint}");
        for tag in [
            "AttachmentTooLargeError",
            "UnsupportedAttachmentError",
            "UnsupportedModalityError",
        ] {
            let refused = refusal(api_error(tag), &args, &session);
            assert_eq!(
                (refused.code, refused.hint),
                (crate::error::EXIT_USAGE, None)
            );
        }
        let other = refusal(api_error("ConflictError"), &args, &session);
        assert_eq!(other.code, crate::error::EXIT_RUNTIME);
        let inside = json!({ "directory": root.path() });
        assert!(
            refusal(api_error("PermissionDeniedError"), &args, &inside)
                .hint
                .is_none()
        );
    }
}
