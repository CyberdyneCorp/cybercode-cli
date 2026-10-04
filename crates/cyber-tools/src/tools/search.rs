//! glob and grep (`builtin-tools` → glob, grep and list tools).
//!
//! Both walk with ripgrep's `ignore` crate, so `.gitignore`, `.ignore` and global excludes
//! apply and `.git/` is skipped.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use globset::{Glob as GlobPattern, GlobMatcher};
use regex::Regex;
use serde_json::{Value, json};

use super::fs::is_binary;
use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

const GLOB_LIMIT: usize = 100;

pub(crate) struct Glob;
pub(crate) struct Grep;

fn walker(base: &Path) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(base)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
}

/// A pattern without `/` matches file names anywhere; otherwise the path relative to `base`.
fn matcher(pattern: &str) -> Result<(GlobMatcher, bool), ToolError> {
    let glob = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .or_else(|_| GlobPattern::new(pattern))
        .map_err(|e| failed(format!("Invalid glob {pattern:?}: {e}")))?;
    Ok((glob.compile_matcher(), pattern.contains('/')))
}

fn glob_matches(m: &(GlobMatcher, bool), base: &Path, path: &Path) -> bool {
    if m.1 {
        path.strip_prefix(base).is_ok_and(|rel| m.0.is_match(rel))
    } else {
        path.file_name().is_some_and(|name| m.0.is_match(name))
    }
}

async fn scope(ctx: &Ctx<'_>, action: &str, resource: &str) -> Result<PathBuf, ToolError> {
    let raw = text(&ctx.inv.input, "path");
    let base = ctx.resolve(if raw.is_empty() { "." } else { raw });
    ctx.check_external(std::slice::from_ref(&base)).await?;
    let req = Request {
        action: action.into(),
        resources: vec![resource.into()],
        read_only: true,
        ..Request::default()
    };
    ctx.authorize(req, vec![resource.into()], Value::Null)
        .await?;
    if !base.exists() {
        return Err(failed(format!("Path not found: {}", base.display())));
    }
    Ok(base)
}

impl Tool for Glob {
    fn def(&self) -> ToolDef {
        def(
            "glob",
            "Find files by glob pattern (for example **/*.rs). Returns up to 100 paths, newest first.",
            json!({"type": "object", "required": ["pattern"], "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let pattern = text(&ctx.inv.input, "pattern");
            let base = scope(ctx, "glob", pattern).await?;
            let m = matcher(pattern)?;
            let mut found: Vec<(SystemTime, PathBuf)> = walker(&base)
                .filter(|p| glob_matches(&m, &base, p))
                .map(|p| {
                    (
                        std::fs::metadata(&p)
                            .and_then(|m| m.modified())
                            .unwrap_or(SystemTime::UNIX_EPOCH),
                        p,
                    )
                })
                .collect();
            if found.is_empty() {
                return Ok("No files found".into());
            }
            found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let total = found.len();
            let mut out: Vec<String> = found
                .into_iter()
                .take(GLOB_LIMIT)
                .map(|(_, p)| p.display().to_string())
                .collect();
            if total > GLOB_LIMIT {
                out.push(format!(
                    "({} more files not shown; narrow the pattern)",
                    total - GLOB_LIMIT
                ));
            }
            Ok(out.join("\n"))
        })
    }
}

impl Tool for Grep {
    fn def(&self) -> ToolDef {
        def(
            "grep",
            "Search file contents with a regular expression. Optional include glob, context_lines (0-5) and limit (default 100).",
            json!({"type": "object", "required": ["pattern"], "properties": {
                "pattern": {"type": "string"}, "path": {"type": "string"}, "include": {"type": "string"},
                "context_lines": {"type": "integer"}, "limit": {"type": "integer"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let pattern = text(input, "pattern");
            let base = scope(ctx, "grep", pattern).await?;
            let regex = Regex::new(pattern).map_err(|e| failed(format!("Invalid regex: {e}")))?;
            let include = match text(input, "include") {
                "" => None,
                glob => Some(matcher(glob)?),
            };
            let context = number(input, "context_lines").unwrap_or(0).min(5) as usize;
            let limit = number(input, "limit").unwrap_or(100).clamp(1, 1000) as usize;
            let files = walker(&base)
                .filter(|p| include.as_ref().is_none_or(|m| glob_matches(m, &base, p)));
            Ok(search(files, &regex, context, limit, &ctx.cancel))
        })
    }
}

fn search(
    files: impl Iterator<Item = PathBuf>,
    regex: &Regex,
    context: usize,
    limit: usize,
    cancel: &tokio_util::sync::CancellationToken,
) -> String {
    let mut blocks = Vec::new();
    let mut total = 0;
    for path in files {
        if total >= limit || cancel.is_cancelled() {
            break;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if is_binary(&bytes) {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = content.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| regex.is_match(l))
            .map(|(i, _)| i)
            .take(limit - total)
            .collect();
        if hits.is_empty() {
            continue;
        }
        total += hits.len();
        blocks.push(render(&path, &lines, &hits, context));
    }
    match total {
        0 => "No matches found".into(),
        n if n >= limit => format!("Found {n} matches (limit reached)\n{}", blocks.join("\n")),
        n => format!("Found {n} matches\n{}", blocks.join("\n")),
    }
}

fn render(path: &Path, lines: &[&str], hits: &[usize], context: usize) -> String {
    let mut shown: Vec<usize> = hits
        .iter()
        .flat_map(|&h| h.saturating_sub(context)..=(h + context).min(lines.len() - 1))
        .collect();
    shown.sort_unstable();
    shown.dedup();
    let rows: Vec<String> = shown
        .into_iter()
        .map(|i| {
            let line: String = lines[i].chars().take(500).collect();
            if hits.contains(&i) {
                format!("  Line {}: {line}", i + 1)
            } else {
                format!("  {}- {line}", i + 1)
            }
        })
        .collect();
    format!("{}:\n{}", path.display(), rows.join("\n"))
}
