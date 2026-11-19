//! The `repo_map` tool (`code-intelligence` → repo_map tool): a ranked map of the
//! Location's files and definitions, fitted to a token budget. Read-only and
//! process-free, so it is offered in plan mode and in untrusted checkouts.

use std::path::{Path, PathBuf};

use cyber_core::config::RepoMapSettings;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed};
use crate::host::Ctx;
use crate::permissions::{self, Effect, Request};
use crate::repo_map::{self, MapQuery};

pub(crate) const NAME: &str = "repo_map";
pub(crate) const DISABLED: &str = "Repository map is disabled (repo_map.enabled)";

pub(crate) struct RepoMap;

impl Tool for RepoMap {
    fn def(&self) -> ToolDef {
        def(
            NAME,
            "Show a ranked map of the repository: the most relevant files and their key definitions (signature lines), fitted to a token budget. Use it to find where things are before searching. Pass focus_paths and focus_symbols to center the map on what you are working on; path restricts it to a directory.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Location-relative directory or file to restrict the map to"},
                    "focus_paths": {"type": "array", "items": {"type": "string"}, "maxItems": repo_map::MAX_FOCUS},
                    "focus_symbols": {"type": "array", "items": {"type": "string"}, "maxItems": repo_map::MAX_FOCUS},
                    "tokens": {"type": "integer", "description": "Token budget, 256 to 8192"}
                }
            }),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(run(ctx))
    }
}

/// Whether the tool is offered at `location`: the index service is attached and
/// `repo_map.enabled` is true.
pub(crate) fn offered(host: &crate::BuiltinHost, location: &Path) -> bool {
    host.repo_maps.get().is_some() && settings(host, location).enabled
}

pub(crate) fn settings(host: &crate::BuiltinHost, location: &Path) -> RepoMapSettings {
    RepoMapSettings::from_config(&host.config_for(location)).0
}

/// The tool's token budget: the argument, else the profile's `tool_tokens`, clamped.
fn tokens(input: &Value, profile: u32) -> usize {
    repo_map::clamp_tokens(
        input
            .get("tokens")
            .and_then(Value::as_i64)
            .unwrap_or(i64::from(profile)),
    )
}

fn strings(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(repo_map::MAX_FOCUS)
        .map(str::to_owned)
        .collect()
}

fn relative(path: &str) -> Result<String, ToolError> {
    repo_map::relative_arg(path).map_err(failed)
}

async fn run(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let maps = ctx
        .host
        .repo_maps
        .get()
        .ok_or_else(|| failed("Repository map unavailable in this host"))?
        .clone();
    let settings = settings(ctx.host, &ctx.location);
    if !settings.enabled {
        return Err(failed(DISABLED));
    }
    let input = &ctx.inv.input;
    let path = relative(super::text(input, "path"))?;
    let resource = if path.is_empty() { "*" } else { path.as_str() };
    let req = Request {
        action: NAME.into(),
        resources: vec![resource.into()],
        read_only: true,
        reads: vec![ctx.location.join(&path)],
        ..Request::default()
    };
    ctx.authorize(req, vec![resource.into()], Value::Null)
        .await?;
    let (root, scoped) = scope(ctx, &path)?;
    let mut query = MapQuery {
        path: scoped,
        focus_paths: strings(input, "focus_paths"),
        focus_symbols: strings(input, "focus_symbols"),
        session_files: Vec::new(),
        tokens: tokens(input, ctx.inv.repo_map.tool_tokens),
    };
    query.session_files = ctx
        .host
        .recent_files(&ctx.inv.session_id)
        .iter()
        .filter_map(|p| crate::repo_map::walk::relative(&root, &canonical(p)))
        .collect();
    let hidden = hidden_filter(ctx, &root);
    let output = maps
        .query(&root, &settings, &query, hidden, repo_map::TOOL_WAIT)
        .await
        .map_err(failed)?;
    Ok(output.tool_text())
}

fn scope(ctx: &Ctx<'_>, path: &str) -> Result<(PathBuf, Option<String>), ToolError> {
    repo_map::checkout_scope(&ctx.location, &ctx.host.opts.home, path).map_err(failed)
}

pub(crate) fn canonical(path: &Path) -> PathBuf {
    repo_map::canonical(path)
}

/// Files the agent may not read: a `read` rule ending in deny, or a sandbox-hidden path.
fn hidden_filter(ctx: &Ctx<'_>, root: &Path) -> repo_map::Hidden {
    let rules = ctx.policy.rules.clone();
    let any_deny = rules.iter().any(|rule| rule.effect == Effect::Deny);
    let sandbox = crate::sandboxing::hidden(ctx);
    let location = canonical(&ctx.location);
    let root = root.to_path_buf();
    std::sync::Arc::new(move |rel: &str| {
        let absolute = crate::repo_map::walk::absolute(&root, rel);
        if sandbox.denies(&absolute) {
            return true;
        }
        if !any_deny {
            return false;
        }
        let resource = absolute
            .strip_prefix(&location)
            .map_or_else(|_| permissions::slash(&absolute), permissions::slash);
        permissions::evaluate(&rules, "read", &resource).0 == Effect::Deny
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_clamped_and_default_to_the_profile() {
        assert_eq!(tokens(&json!({}), 2048), 2048);
        assert_eq!(tokens(&json!({"tokens": 10}), 2048), 256);
        assert_eq!(tokens(&json!({"tokens": 100000}), 2048), 8192);
        assert_eq!(tokens(&json!({}), 100), 256);
    }

    #[test]
    fn paths_must_stay_in_the_location() {
        assert_eq!(relative("./src/").unwrap(), "src");
        assert_eq!(relative("").unwrap(), "");
        assert!(relative("../x").is_err());
        assert!(relative("/etc").is_err());
    }
}
