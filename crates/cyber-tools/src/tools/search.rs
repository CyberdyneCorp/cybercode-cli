//! glob and grep (`builtin-tools` → glob, grep and list tools).
//!
//! Both walk with ripgrep's `ignore` crate, so `.gitignore`, `.ignore` and global excludes
//! apply and `.git/` is skipped.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cyber_server::runtime::{ContentSource, Provenance, RetrySafety, ToolDef};
use futures::future::BoxFuture;
use globset::{Glob as GlobPattern, GlobMatcher};
use regex::Regex;
use serde_json::{Value, json};

use super::fs::is_binary;
use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

const GLOB_LIMIT: usize = 100;
/// Paths a Workflow script's `tools.glob` returns by default and at most: scripts fan out
/// over files, so their limit is wider than the model's.
const SCRIPT_GLOB_DEFAULT: usize = 1_000;
const SCRIPT_GLOB_MAX: usize = 10_000;
/// Matches a script's `tools.grep` returns by default and at most, as the grep tool.
const SCRIPT_GREP_DEFAULT: usize = 100;
const SCRIPT_GREP_MAX: usize = 1_000;
/// Characters of a matching line a result keeps.
const MATCH_CHARS: usize = 500;

pub(crate) struct Glob;
pub(crate) struct Grep;

/// Files under `base`, without `.git/` and without the profile's `deny_read` matches.
fn walker(base: &Path, hidden: cyber_sandbox::DenyRead) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(base)
        .hidden(false)
        .filter_entry(move |e| e.file_name() != ".git" && !hidden.denies(e.path()))
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
    // The searched base and file filter, for the secret-path check of a tainted run.
    let filter = match action {
        "glob" => resource,
        _ => text(&ctx.inv.input, "include"),
    };
    let mut reads = vec![base.clone()];
    if !filter.is_empty() {
        reads.push(base.join(filter));
    }
    let req = Request {
        action: action.into(),
        resources: vec![resource.into()],
        read_only: true,
        reads,
        ..Request::default()
    };
    ctx.authorize(req, vec![resource.into()], Value::Null)
        .await?;
    // The walk never filters its own root, so a hidden base is refused as `read` is.
    if crate::sandboxing::hidden(ctx).denies(&base) {
        return Err(failed(crate::sandboxing::hidden_read(ctx, &base).await));
    }
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
            let mut found: Vec<(SystemTime, PathBuf)> =
                walker(&base, crate::sandboxing::hidden(ctx))
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
            let files = walker(&base, crate::sandboxing::hidden(ctx))
                .filter(|p| include.as_ref().is_none_or(|m| glob_matches(m, &base, p)));
            let (output, matched) = search(files, &regex, context, limit, &ctx.cancel);
            frame_overflow_matches(ctx, &base, &matched);
            Ok(output)
        })
    }
}

/// Matches from managed overflow files keep their frame (`untrusted-content` → Framed
/// tool results): one file's recorded provenance, or `overflow` for several.
fn frame_overflow_matches(ctx: &Ctx<'_>, base: &Path, matched: &[PathBuf]) {
    let dir = &ctx.host.opts.tool_output_dir;
    let managed: Vec<&PathBuf> = matched
        .iter()
        .filter(|path| crate::untrusted::overflow::managed(dir, path))
        .collect();
    match managed.as_slice() {
        [] => {}
        [file] => ctx.frame_overflow_reads(file),
        _ => ctx.declare_provenance(Provenance::new(
            ContentSource::Overflow,
            base.display().to_string(),
        )),
    }
}

/// The rendered matches and the files they came from.
fn search(
    files: impl Iterator<Item = PathBuf>,
    regex: &Regex,
    context: usize,
    limit: usize,
    cancel: &tokio_util::sync::CancellationToken,
) -> (String, Vec<PathBuf>) {
    let mut matched = Vec::new();
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
        matched.push(path);
    }
    let output = match total {
        0 => "No matches found".into(),
        n if n >= limit => format!("Found {n} matches (limit reached)\n{}", blocks.join("\n")),
        n => format!("Found {n} matches\n{}", blocks.join("\n")),
    };
    (output, matched)
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

/// A script's `max`, else `default`, capped at `cap`.
fn script_limit(input: &Value, default: usize, cap: usize) -> usize {
    number(input, "max").map_or(default, |max| (max as usize).clamp(1, cap))
}

/// `tools.glob` of a Workflow script (`workflows` → Direct read-only tools): the glob
/// tool's permission checks, ignore rules and hidden paths, returning `{paths, truncated}`
/// in path order, Location-relative inside the Location, and `sizes` (bytes) with
/// `sizes: true`.
pub(crate) async fn glob_value(ctx: &Ctx<'_>) -> Result<Value, ToolError> {
    let input = &ctx.inv.input;
    let pattern = text(input, "pattern");
    let base = scope(ctx, "glob", pattern).await?;
    let m = matcher(pattern)?;
    let max = script_limit(input, SCRIPT_GLOB_DEFAULT, SCRIPT_GLOB_MAX);
    let mut paths: Vec<PathBuf> = walker(&base, crate::sandboxing::hidden(ctx))
        .filter(|p| glob_matches(&m, &base, p))
        .collect();
    paths.sort();
    let truncated = paths.len() > max;
    paths.truncate(max);
    let shown: Vec<String> = paths.iter().map(|path| ctx.resource(path)).collect();
    let mut value = json!({"paths": shown, "truncated": truncated});
    // `sizes: true` adds each file's size in bytes, in the same order.
    if input.get("sizes").and_then(Value::as_bool) == Some(true) {
        let sizes: Vec<u64> = paths
            .iter()
            .map(|path| std::fs::metadata(path).map_or(0, |meta| meta.len()))
            .collect();
        value["sizes"] = json!(sizes);
    }
    Ok(value)
}

/// `tools.grep` of a Workflow script: the grep tool's checks, returning
/// `{matches: [{path, line, text}], truncated}` in path and line order.
pub(crate) async fn grep_value(ctx: &Ctx<'_>) -> Result<Value, ToolError> {
    let input = &ctx.inv.input;
    let pattern = text(input, "pattern");
    let base = scope(ctx, "grep", pattern).await?;
    let regex = Regex::new(pattern).map_err(|e| failed(format!("Invalid regex: {e}")))?;
    let include = match text(input, "include") {
        "" => None,
        glob => Some(matcher(glob)?),
    };
    let max = script_limit(input, SCRIPT_GREP_DEFAULT, SCRIPT_GREP_MAX);
    let mut files: Vec<PathBuf> = walker(&base, crate::sandboxing::hidden(ctx))
        .filter(|p| include.as_ref().is_none_or(|m| glob_matches(m, &base, p)))
        .collect();
    files.sort();
    let mut matches = Vec::new();
    let mut truncated = false;
    for path in files {
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Aborted);
        }
        let found = file_matches(&path, &regex, max + 1 - matches.len());
        let room = max - matches.len();
        truncated = found.len() > room;
        let shown = ctx.resource(&path);
        matches.extend(
            found
                .into_iter()
                .take(room)
                .map(|(line, text)| json!({"path": shown, "line": line, "text": text})),
        );
        if truncated {
            break;
        }
    }
    Ok(json!({"matches": matches, "truncated": truncated}))
}

/// Up to `limit` matching lines of a text file: 1-based line number and the line, cut at
/// [`MATCH_CHARS`]. Unreadable and binary files have none.
fn file_matches(path: &Path, regex: &Regex, limit: usize) -> Vec<(usize, String)> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if is_binary(&bytes) {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .take(limit)
        .map(|(index, line)| (index + 1, line.chars().take(MATCH_CHARS).collect()))
        .collect()
}
