//! `notify` and `send_file` (`background-tasks` → Notify tool, Send file to user): reach
//! the user through their connected clients. Both record a durable Session event the
//! clients act on (a desktop notification, a downloadable attachment); push to paired
//! Devices belongs to remote control.

use std::path::{Path, PathBuf};

use cyber_server::runtime::{FilesSent, Notification, RetrySafety, SentFile, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::permissions::{Mode, Request};

const MAX_TITLE: usize = 80;
const MAX_BODY: usize = 300;
const MAX_CAPTION: usize = 1000;
pub(crate) const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 10;

pub(crate) struct Notify;
pub(crate) struct SendFile;

impl Tool for Notify {
    fn def(&self) -> ToolDef {
        def(
            "notify",
            "Notify the user on their desktop (and their paired devices) that something needs their attention, such as a long task finishing. title is cut to 80 characters and body to 300. At most 10 notifications per session per hour; use it sparingly.",
            json!({"type": "object", "required": ["title"], "additionalProperties": false, "properties": {
                "title": {"type": "string"}, "body": {"type": "string"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(notify(ctx))
    }
}

impl Tool for SendFile {
    fn def(&self) -> ToolDef {
        def(
            "send_file",
            "Deliver files from the project to the user as downloadable attachments, with an optional caption: at most 10 files per call and 25 MB per file. Reading each file needs the read permission.",
            json!({"type": "object", "required": ["paths"], "additionalProperties": false, "properties": {
                "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_FILES},
                "caption": {"type": "string"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(send(ctx))
    }
}

/// `text` without control characters or bidirectional overrides, cut to `max`
/// characters with an ellipsis.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|&c| {
            !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() <= max {
        return clean.to_string();
    }
    let cut: String = clean.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

fn refused(error: cyber_server::runtime::RuntimeError) -> ToolError {
    super::background::refusal(error)
}

async fn notify(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let title = clip(text(&ctx.inv.input, "title"), MAX_TITLE);
    let body = clip(text(&ctx.inv.input, "body"), MAX_BODY);
    if title.is_empty() {
        return Err(failed("A notification needs a title"));
    }
    let config = ctx.host.config_for(&ctx.location);
    if ctx.policy.mode == Mode::Plan
        && config.pointer("/background/deny_notify_in_plan") == Some(&Value::Bool(true))
    {
        return Err(failed(
            "notify is denied in plan mode (background.deny_notify_in_plan)",
        ));
    }
    // It changes nothing in the project, so plan mode's read-only ceiling allows it.
    let req = Request {
        action: "notify".into(),
        resources: vec![title.clone()],
        read_only: true,
        ..Request::default()
    };
    ctx.authorize(req, vec!["*".into()], json!({"title": title, "body": body}))
        .await?;
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Notifications require the runtime"))?;
    runtime
        .record_notification(
            &ctx.inv.session_id,
            Notification {
                call_id: ctx.inv.call_id.clone(),
                title: title.clone(),
                body: body.clone(),
            },
        )
        .await
        .map_err(refused)?;
    Ok(json!({"title": title, "body": body, "message": "The user was notified."}).to_string())
}

/// A file checked for sending.
struct Outgoing {
    path: PathBuf,
    name: String,
    size: u64,
}

fn paths(input: &Value) -> Result<Vec<&str>, ToolError> {
    let list = input
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| failed("paths must be an array of file paths"))?;
    if list.is_empty() || list.len() > MAX_FILES {
        return Err(failed(format!(
            "Send 1 to {MAX_FILES} files per call, got {}",
            list.len()
        )));
    }
    list.iter()
        .map(|p| p.as_str().filter(|p| !p.trim().is_empty()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| failed("paths must be non-empty strings"))
}

/// The `read` permission, the external-directory check and `deny_read`, as for `read`,
/// then the size cap.
async fn check(ctx: &Ctx<'_>, raw: &str) -> Result<Outgoing, ToolError> {
    let path = ctx.resolve(raw);
    ctx.check_external(std::slice::from_ref(&path)).await?;
    let resource = ctx.resource(&path);
    let req = Request {
        action: "read".into(),
        resources: vec![resource.clone()],
        read_only: true,
        ..Request::default()
    };
    ctx.authorize(req, vec![resource], Value::Null).await?;
    if crate::sandboxing::hidden(ctx).denies(&path) {
        return Err(failed(crate::sandboxing::hidden_read(ctx, &path).await));
    }
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| failed(format!("No such file: {}", path.display())))?;
    if !meta.is_file() {
        return Err(failed(format!("Not a file: {}", path.display())));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(failed(format!(
            "{} is {} bytes; files are limited to 25 MB",
            path.display(),
            meta.len()
        )));
    }
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    Ok(Outgoing {
        name,
        path,
        size: meta.len(),
    })
}

async fn send(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let input = &ctx.inv.input;
    let caption = Some(clip(text(input, "caption"), MAX_CAPTION)).filter(|c| !c.is_empty());
    let mut outgoing = Vec::new();
    for raw in paths(input)? {
        outgoing.push(check(ctx, raw).await?);
    }
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Sending files requires the runtime"))?;
    let bytes = outgoing.iter().map(|file| file.size).sum();
    runtime
        .admit_sent_files(&ctx.inv.session_id, outgoing.len(), bytes)
        .await
        .map_err(refused)?;
    let store = ctx
        .host
        .opts
        .tool_output_dir
        .parent()
        .ok_or_else(|| failed("Missing the data directory for sent files"))?
        .join("sent");
    let mut files = Vec::new();
    for file in outgoing {
        match keep(&store, file).await {
            Ok(kept) => files.push(kept),
            Err(error) => {
                discard(&store, &files).await;
                return Err(error);
            }
        }
    }
    let summary: Vec<Value> = files
        .iter()
        .map(|f| json!({"id": f.id, "name": f.name, "size": f.size, "media_type": f.media_type}))
        .collect();
    let sent = FilesSent {
        call_id: ctx.inv.call_id.clone(),
        message_id: ctx.inv.message_id.clone(),
        caption: caption.clone(),
        files,
    };
    if let Err(error) = runtime
        .record_sent_files(&ctx.inv.session_id, sent.clone())
        .await
    {
        // An unrecorded copy has no owner to delete it later.
        discard(&store, &sent.files).await;
        return Err(refused(error));
    }
    Ok(json!({"files": summary, "caption": caption, "message": "The files were delivered to the user."}).to_string())
}

async fn discard(store: &Path, files: &[SentFile]) {
    for file in files {
        let _ = tokio::fs::remove_dir_all(store.join(&file.id)).await;
    }
}

/// Copy `file` to `<data>/sent/<file_id>/`, so the delivery stays what was sent.
async fn keep(store: &Path, file: Outgoing) -> Result<SentFile, ToolError> {
    let id = cyber_core::ids::new_id("file");
    let dir = store.join(&id);
    let stored = dir.join(&file.name);
    let copy = async {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::copy(&file.path, &stored).await?;
        tokio::fs::read(&stored).await
    };
    let bytes = copy
        .await
        .map_err(|e| failed(format!("Could not send {}: {e}", file.path.display())))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(failed(format!(
            "{} grew past 25 MB while it was sent",
            file.path.display()
        )));
    }
    Ok(SentFile {
        id,
        media_type: media_type(&file.name).into(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        size: bytes.len() as u64,
        source: file.path.display().to_string(),
        stored_path: stored.display().to_string(),
        name: file.name,
    })
}

fn media_type(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_strips_controls_and_marks_the_cut() {
        assert_eq!(clip("  Done\n\u{1b}[2J", 80), "Done [2J");
        let long = clip(&"x".repeat(500), MAX_BODY);
        assert_eq!(long.chars().count(), MAX_BODY);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn paths_are_bounded() {
        assert!(paths(&json!({"paths": []})).is_err());
        assert!(paths(&json!({"paths": vec!["a"; 11]})).is_err());
        assert!(paths(&json!({"paths": [""]})).is_err());
        assert_eq!(paths(&json!({"paths": ["a", "b"]})).unwrap(), ["a", "b"]);
    }

    #[test]
    fn media_types_come_from_extensions() {
        assert_eq!(media_type("report.PDF"), "application/pdf");
        assert_eq!(media_type("data"), "application/octet-stream");
    }
}
