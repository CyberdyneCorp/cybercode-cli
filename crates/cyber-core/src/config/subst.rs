//! `{env:NAME}`, `{env:NAME:-default}` and `{file:path}` substitution in string values.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::env::EnvSource;

pub struct SubstContext<'a> {
    pub env: &'a dyn EnvSource,
    /// Directory of the declaring document; `{file:}` paths resolve against it.
    pub base_dir: &'a Path,
    pub home: &'a Path,
}

/// Substitute every string value (never keys). Failures are pushed onto `issues`.
pub fn substitute(
    value: &mut Value,
    ctx: &SubstContext<'_>,
    pointer: &str,
    issues: &mut Vec<String>,
) {
    match value {
        Value::String(s) if has_placeholder(s) => match expand(s, ctx) {
            Ok(expanded) => *s = expanded,
            Err(e) => issues.push(format!("{pointer}: {e}")),
        },
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                substitute(item, ctx, &format!("{pointer}/{i}"), issues);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                substitute(
                    item,
                    ctx,
                    &super::merge::child_pointer(pointer, key),
                    issues,
                );
            }
        }
        _ => {}
    }
}

pub fn has_placeholder(s: &str) -> bool {
    find_placeholder(s).is_some()
}

fn find_placeholder(s: &str) -> Option<usize> {
    [s.find("{env:"), s.find("{file:")]
        .into_iter()
        .flatten()
        .min()
}

fn expand(s: &str, ctx: &SubstContext<'_>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = find_placeholder(rest) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find('}')
            .ok_or_else(|| format!("unterminated placeholder in {s:?}"))?;
        out.push_str(&resolve(&tail[1..end], ctx)?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve(token: &str, ctx: &SubstContext<'_>) -> Result<String, String> {
    if let Some(spec) = token.strip_prefix("env:") {
        let (name, fallback) = match spec.split_once(":-") {
            Some((name, fallback)) => (name, Some(fallback)),
            None => (spec, None),
        };
        return Ok(ctx
            .env
            .get(name)
            .or_else(|| fallback.map(str::to_string))
            .unwrap_or_default());
    }
    let rel = token.strip_prefix("file:").unwrap_or(token);
    let path = resolve_path(rel, ctx);
    std::fs::read_to_string(&path)
        .map(|content| content.trim().to_string())
        .map_err(|e| format!("{{file:{rel}}} could not be read ({}): {e}", path.display()))
}

fn resolve_path(rel: &str, ctx: &SubstContext<'_>) -> PathBuf {
    if let Some(rest) = rel.strip_prefix("~/") {
        return ctx.home.join(rest);
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.base_dir.join(path)
    }
}
