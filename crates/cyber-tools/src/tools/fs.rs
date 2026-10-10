//! read, write, edit and list (`builtin-tools`).

use std::path::{Path, PathBuf};

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

const PAGE_LINES: usize = 2000;
const PAGE_BYTES: usize = 50 * 1024;
const LONG_LINE: usize = 2000;
const BOM: &str = "\u{feff}";

pub(crate) struct Read;
pub(crate) struct Write;
pub(crate) struct Edit;
pub(crate) struct List;

impl Tool for Read {
    fn def(&self) -> ToolDef {
        def(
            "read",
            "Read a text file as numbered lines (paged with offset/limit) or list a directory.",
            json!({"type": "object", "required": ["path"], "properties": {
                "path": {"type": "string"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let path = ctx.resolve(text(&ctx.inv.input, "path"));
            ctx.check_external(std::slice::from_ref(&path)).await?;
            let resource = ctx.resource(&path);
            let req = Request {
                action: "read".into(),
                resources: vec![resource.clone()],
                read_only: true,
                ..Request::default()
            };
            ctx.authorize(req, vec![resource], Value::Null).await?;
            let meta = tokio::fs::metadata(&path)
                .await
                .map_err(|_| failed(not_found(&path)))?;
            if meta.is_dir() {
                return Ok(list_dir(&path, 1));
            }
            let origin = ctx
                .host
                .lsp
                .get()
                .and_then(|_| crate::lsp::read_origin(&ctx.location, &path).ok());
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| failed(format!("Could not read {}: {e}", path.display())))?;
            if let Some(kind) = media_kind(&path) {
                return Err(failed(format!(
                    "{kind} files are returned as media from a later milestone; this build reads text only"
                )));
            }
            if is_binary(&bytes) {
                return Err(failed("Binary file: application/octet-stream"));
            }
            ctx.host.mark_read(&ctx.inv.session_id, &path);
            if let Some(locations) = ctx.host.lsp.get()
                && bytes.len() <= crate::lsp::MAX_DOCUMENT_BYTES
                && let Ok(content) = std::str::from_utf8(&bytes)
                && let Some(origin) = origin
            {
                let _ = locations.warm_observed(
                    &ctx.location,
                    path.clone(),
                    content.to_owned(),
                    origin,
                );
            }
            let offset = number(&ctx.inv.input, "offset").unwrap_or(1).max(1) as usize;
            let limit = number(&ctx.inv.input, "limit")
                .map_or(PAGE_LINES, |l| (l as usize).min(PAGE_LINES));
            Ok(page(&String::from_utf8_lossy(&bytes), offset, limit))
        })
    }
}

/// `<n>: <line>` from `offset` (1-based); at most `limit` lines and 50 KiB.
fn page(content: &str, offset: usize, limit: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return "(empty file)".into();
    }
    let mut out = String::new();
    let mut next = None;
    for (index, line) in lines.iter().enumerate().skip(offset - 1) {
        let shown = if line.chars().count() > LONG_LINE {
            format!(
                "{}…[truncated]",
                line.chars().take(LONG_LINE).collect::<String>()
            )
        } else {
            (*line).to_string()
        };
        let row = format!("{}: {shown}\n", index + 1);
        if index + 1 - offset >= limit || out.len() + row.len() > PAGE_BYTES {
            next = Some(index + 1);
            break;
        }
        out.push_str(&row);
    }
    if offset > lines.len() {
        return format!(
            "(offset {offset} is past the end of the file: {} lines)",
            lines.len()
        );
    }
    match next {
        Some(n) => format!("{out}(next_offset: {n})"),
        None => out.trim_end().to_string(),
    }
}

fn media_kind(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Some("Image"),
        "pdf" => Some("PDF"),
        _ => None,
    }
}

pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

/// `File not found` plus up to three similar names from the same directory.
fn not_found(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut similar: Vec<(f64, String)> = path
        .parent()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .map(|candidate| (strsim::jaro_winkler(&name, &candidate), candidate))
        .filter(|(score, _)| *score > 0.75)
        .collect();
    similar.sort_by(|a, b| b.0.total_cmp(&a.0));
    let names: Vec<String> = similar.into_iter().take(3).map(|(_, n)| n).collect();
    if names.is_empty() {
        format!("File not found: {}", path.display())
    } else {
        format!(
            "File not found: {}. Did you mean: {}?",
            path.display(),
            names.join(", ")
        )
    }
}

pub(crate) fn list_dir(root: &Path, depth: usize) -> String {
    let mut lines = Vec::new();
    walk(root, root, depth, &mut lines);
    if lines.is_empty() {
        "(empty directory)".into()
    } else {
        lines.join("\n")
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    let mut entries: Vec<(bool, PathBuf)> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.path() != dir)
        .map(|e| (e.file_type().is_some_and(|t| t.is_dir()), e.into_path()))
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let indent = "  ".repeat(dir.strip_prefix(root).map_or(0, |p| p.components().count()));
    for (is_dir, path) in entries {
        if out.len() >= 500 {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(format!("{indent}{name}{}", if is_dir { "/" } else { "" }));
        if is_dir && depth > 1 {
            walk(root, &path, depth - 1, out);
        }
    }
}

impl Tool for List {
    fn def(&self) -> ToolDef {
        def(
            "list",
            "List a directory tree (directories first), respecting .gitignore. depth 1-3.",
            json!({"type": "object", "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let raw = text(&ctx.inv.input, "path");
            let path = ctx.resolve(if raw.is_empty() { "." } else { raw });
            ctx.check_external(std::slice::from_ref(&path)).await?;
            let resource = ctx.resource(&path);
            let req = Request {
                action: "list".into(),
                resources: vec![resource.clone()],
                read_only: true,
                ..Request::default()
            };
            ctx.authorize(req, vec![resource], Value::Null).await?;
            if !path.is_dir() {
                return Err(failed(format!("Not a directory: {}", path.display())));
            }
            let depth = number(&ctx.inv.input, "depth").unwrap_or(1).clamp(1, 3) as usize;
            Ok(list_dir(&path, depth))
        })
    }
}

/// Bytes on disk, decoded for editing: content without BOM, CRLF normalized to LF.
struct TextFile {
    bom: bool,
    crlf: bool,
    content: String,
}

fn decode(bytes: &[u8], path: &Path) -> Result<TextFile, ToolError> {
    if is_binary(bytes) {
        return Err(failed(format!("{} is a binary file", path.display())));
    }
    let raw = String::from_utf8(bytes.to_vec())
        .map_err(|_| failed(format!("{} is not UTF-8 text", path.display())))?;
    let bom = raw.starts_with(BOM);
    let body = raw.trim_start_matches(BOM);
    Ok(TextFile {
        bom,
        crlf: body.contains("\r\n"),
        content: body.replace("\r\n", "\n"),
    })
}

fn encode(file: &TextFile, content: &str) -> Vec<u8> {
    let body = if file.crlf {
        content.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        content.to_string()
    };
    let mut out = String::new();
    if file.bom {
        out.push_str(BOM);
    }
    out.push_str(&body);
    out.into_bytes()
}

pub(crate) fn unified_diff(path: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// Ask for an edit, then write only if the file is unchanged since it was read for approval.
pub(crate) async fn guarded_write(
    ctx: &Ctx<'_>,
    path: &Path,
    before: Option<&[u8]>,
    after: &[u8],
    diff: String,
) -> Result<String, ToolError> {
    ctx.check_external(&[path.to_path_buf()]).await?;
    authorize_edit(ctx, path, diff).await?;
    let origin = super::intelligence::capture(ctx, path);
    let lock = ctx.host.path_lock(path);
    let _guard = lock.lock().await;
    let current = tokio::fs::read(path).await.ok();
    if current.as_deref() != before {
        return Err(failed(
            "File changed after permission approval. Read it again before editing.",
        ));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| failed(format!("Could not create {}: {e}", parent.display())))?;
    }
    tokio::fs::write(path, after)
        .await
        .map_err(|e| failed(format!("Could not write {}: {e}", path.display())))?;
    ctx.host.mark_read(&ctx.inv.session_id, path);
    drop(_guard);
    Ok(super::intelligence::feedback(ctx, path, after, origin).await)
}

pub(crate) async fn authorize_edit(
    ctx: &Ctx<'_>,
    path: &Path,
    diff: String,
) -> Result<(), ToolError> {
    let resource = ctx.resource(path);
    let req = Request {
        action: "edit".into(),
        resources: vec![resource.clone()],
        mutates: vec![path.to_path_buf()],
        file_edit: true,
        ..Request::default()
    };
    ctx.authorize(req, vec![resource], json!({ "diff": diff }))
        .await
}

impl Tool for Write {
    fn def(&self) -> ToolDef {
        def(
            "write",
            "Create or overwrite a file. Read an existing file before overwriting it.",
            json!({"type": "object", "required": ["path", "content"], "properties": {
                "path": {"type": "string"}, "content": {"type": "string"}}}),
            RetrySafety::Reconcile,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let path = ctx.resolve(text(&ctx.inv.input, "path"));
            let content = text(&ctx.inv.input, "content");
            let before = tokio::fs::read(&path).await.ok();
            if before.is_some() && !ctx.host.was_read(&ctx.inv.session_id, &path) {
                return Err(failed("Read the file before overwriting it."));
            }
            let file = match &before {
                Some(bytes) => decode(bytes, &path)?,
                None => TextFile {
                    bom: false,
                    crlf: false,
                    content: String::new(),
                },
            };
            let diff = unified_diff(&ctx.resource(&path), &file.content, content);
            let feedback =
                guarded_write(ctx, &path, before.as_deref(), &encode(&file, content), diff).await?;
            let output = match before {
                None => format!("Created {}", ctx.resource(&path)),
                Some(_) => format!(
                    "Wrote {} ({} lines)",
                    ctx.resource(&path),
                    content.lines().count()
                ),
            };
            Ok(format!("{output}{feedback}"))
        })
    }
}

impl Tool for Edit {
    fn def(&self) -> ToolDef {
        def(
            "edit",
            "Replace an exact, unique string in a file (or every occurrence with replace_all).",
            json!({"type": "object", "required": ["path", "old_string", "new_string"], "properties": {
                "path": {"type": "string"}, "old_string": {"type": "string"}, "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"}}}),
            RetrySafety::Reconcile,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let path = ctx.resolve(text(input, "path"));
            let (old, new) = (
                text(input, "old_string").replace("\r\n", "\n"),
                text(input, "new_string").replace("\r\n", "\n"),
            );
            if old.is_empty() || old == new {
                return Err(failed(
                    "old_string must be non-empty and different from new_string",
                ));
            }
            let before = tokio::fs::read(&path)
                .await
                .map_err(|_| failed(not_found(&path)))?;
            let file = decode(&before, &path)?;
            let count = file.content.matches(&old).count();
            let replace_all = input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match count {
                0 => {
                    return Err(failed(format!(
                        "old_string was not found in {}",
                        ctx.resource(&path)
                    )));
                }
                n if n > 1 && !replace_all => {
                    return Err(failed(format!(
                        "Found {n} matches; provide more context or set replace_all"
                    )));
                }
                _ => {}
            }
            let updated = if replace_all {
                file.content.replace(&old, &new)
            } else {
                file.content.replacen(&old, &new, 1)
            };
            let diff = unified_diff(&ctx.resource(&path), &file.content, &updated);
            let feedback = guarded_write(
                ctx,
                &path,
                Some(&before),
                &encode(&file, &updated),
                diff.clone(),
            )
            .await?;
            let replaced = if replace_all { count } else { 1 };
            Ok(format!(
                "Edited {} ({replaced} replacement{})\n{diff}{feedback}",
                ctx.resource(&path),
                if replaced == 1 { "" } else { "s" }
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::page;

    #[test]
    fn paging_numbers_lines_and_reports_next_offset() {
        let content = (1..=5)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(page(&content, 1, 2), "1: l1\n2: l2\n(next_offset: 3)");
        assert_eq!(page(&content, 4, 10), "4: l4\n5: l5");
        assert!(page(&"x".repeat(3000), 1, 10).ends_with("…[truncated]"));
    }
}
