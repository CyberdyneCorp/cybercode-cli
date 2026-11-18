//! `cyber lsp map` (`code-intelligence` → Repository map inspection surfaces): through the
//! registered server when there is one, otherwise in process on the same cache, without
//! starting a server or creating the session database.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_core::config::RepoMapSettings;
use cyber_server::http::{RepoMapIndex, RepoMapRequest, RepoMapView};
use cyber_tools::repo_map::{self, MapQuery, RepoMaps};

use crate::{cli::GlobalArgs, context::Context, error::CliError, output};

#[derive(Debug, clap::Args)]
pub struct MapArgs {
    /// Location-relative directory or file to restrict the map to.
    pub path: Option<String>,
    /// Center the map on this file (repeatable, up to 20).
    #[arg(long = "focus", value_name = "PATH")]
    pub focus: Vec<String>,
    /// Center the map on this symbol (repeatable, up to 20).
    #[arg(long = "symbol", value_name = "NAME")]
    pub symbol: Vec<String>,
    /// Token budget, clamped to 256-8192 (default 2048).
    #[arg(long)]
    pub tokens: Option<usize>,
    /// Print the index status: files by language, exclusions, symbols, freshness, cache.
    #[arg(long, conflicts_with_all = ["rebuild", "clear"])]
    pub status: bool,
    /// Re-extract every file of this checkout.
    #[arg(long, conflicts_with = "clear")]
    pub rebuild: bool,
    /// Delete this checkout's rows from the index cache.
    #[arg(long)]
    pub clear: bool,
}

pub fn map(args: MapArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let config = ctx.config()?;
    let (settings, _) = RepoMapSettings::from_config(&config.value);
    if !settings.enabled {
        return Err(CliError::usage(
            "Repository map is disabled (repo_map.enabled)",
        ));
    }
    let maps = RepoMaps::new(ctx.paths.cache.clone());
    let path = repo_map::relative_arg(args.path.as_deref().unwrap_or_default())
        .map_err(CliError::usage)?;
    if args.clear {
        let (root, _) = scope(ctx, "")?;
        maps.clear(&root).map_err(CliError::runtime)?;
        println!("Cleared the repository index of {}", root.display());
        return Ok(());
    }
    let request = RepoMapRequest {
        path: (!path.is_empty()).then(|| path.clone()),
        focus: args.focus,
        symbol: args.symbol,
        tokens: args.tokens,
    };
    let runtime = super::serve::runtime()?;
    if args.status || args.rebuild {
        let index = match server_index(ctx, &runtime, &request, args.rebuild)? {
            Some(index) => index,
            None => local_index(ctx, &maps, &settings, args.rebuild)?,
        };
        return print_status(&index, &maps, global);
    }
    let view = match runtime
        .block_on(cyber_app::registered_repo_map(
            &ctx.paths,
            &ctx.location,
            &request,
        ))
        .map_err(CliError::runtime)?
    {
        Some(view) => view,
        None => local_map(ctx, &maps, &settings, &request, &config)?,
    };
    if output::is_json(global.format) {
        return output::json(&view);
    }
    print!("{}", header(&view));
    print!("{}", view.text);
    Ok(())
}

fn scope(ctx: &Context, path: &str) -> Result<(PathBuf, Option<String>), CliError> {
    repo_map::checkout_scope(&ctx.location, &ctx.home, path).map_err(CliError::usage)
}

/// The status (after a rebuild, when asked) from the registered server, if any.
fn server_index(
    ctx: &Context,
    runtime: &tokio::runtime::Runtime,
    request: &RepoMapRequest,
    rebuild: bool,
) -> Result<Option<RepoMapIndex>, CliError> {
    let result = if rebuild {
        runtime.block_on(cyber_app::registered_repo_map_rebuild(
            &ctx.paths,
            &ctx.location,
        ))
    } else {
        let request = RepoMapRequest {
            tokens: Some(repo_map::MIN_TOKENS),
            ..request.clone()
        };
        runtime
            .block_on(cyber_app::registered_repo_map(
                &ctx.paths,
                &ctx.location,
                &request,
            ))
            .map(|view| view.map(|view| view.index))
    };
    result.map_err(CliError::runtime)
}

fn local_index(
    ctx: &Context,
    maps: &RepoMaps,
    settings: &RepoMapSettings,
    rebuild: bool,
) -> Result<RepoMapIndex, CliError> {
    let (root, _) = scope(ctx, "")?;
    if rebuild {
        maps.build_blocking(&root, settings, true)
            .map_err(CliError::runtime)?;
    }
    maps.status(&root).map_err(CliError::runtime)
}

/// Refresh the index in process, then rank and render it.
fn local_map(
    ctx: &Context,
    maps: &RepoMaps,
    settings: &RepoMapSettings,
    request: &RepoMapRequest,
    config: &cyber_core::config::Resolved,
) -> Result<RepoMapView, CliError> {
    let path = request.path.clone().unwrap_or_default();
    let (root, scoped) = scope(ctx, &path)?;
    maps.build_blocking(&root, settings, false)
        .map_err(CliError::runtime)?;
    let rules = config
        .value
        .get("permissions")
        .map(|value| cyber_tools::permissions::parse_rules(value, &config.sources))
        .unwrap_or_default();
    let hidden = read_denied(rules);
    let query = MapQuery {
        path: scoped,
        focus_paths: request.focus.clone(),
        focus_symbols: request.symbol.clone(),
        session_files: Vec::new(),
        tokens: repo_map::clamp_tokens(
            request
                .tokens
                .map_or(repo_map::DEFAULT_TOOL_TOKENS as i64, |t| t as i64),
        ),
    };
    let output = maps
        .answer(&root, &query, &*hidden)
        .map_err(CliError::runtime)?;
    Ok(output.view())
}

fn read_denied(rules: Vec<cyber_tools::permissions::Rule>) -> repo_map::Hidden {
    use cyber_tools::permissions::{Effect, evaluate};
    Arc::new(move |rel: &str| evaluate(&rules, "read", rel).0 == Effect::Deny)
}

fn header(view: &RepoMapView) -> String {
    if view.index.files == 0 {
        return "No indexed files\n".into();
    }
    format!(
        "Repository map: {} of {} files, {} symbols, ~{} tokens (index {})\n",
        view.files.len(),
        view.index.files,
        view.index.symbols,
        repo_map::render::estimate_tokens(view.text.len()),
        view.index.status
    )
}

fn print_status(
    index: &RepoMapIndex,
    maps: &RepoMaps,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let (cache, size) = maps.cache_usage();
    if output::is_json(global.format) {
        return output::json(&serde_json::json!({
            "index": index,
            "cache": { "path": cache, "bytes": size },
        }));
    }
    println!("Index: {}", index.status);
    println!("Indexed files: {}", index.files);
    for (lang, count) in &index.languages {
        println!("  {lang}: {count}");
    }
    let excluded: u64 = index.excluded.values().sum();
    println!("Excluded files: {excluded}");
    for (reason, count) in &index.excluded {
        println!("  {reason}: {count}");
    }
    println!("Symbols: {}", index.symbols);
    println!("Last refresh: {}", refreshed(index.refreshed_at));
    if let Some(ms) = index.build_ms {
        println!("Build duration: {ms} ms");
    }
    println!("Cache: {} ({} bytes)", display(&cache), size);
    Ok(())
}

fn refreshed(at: Option<i64>) -> String {
    at.and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(|| "never".into(), |time| time.to_rfc3339())
}

fn display(path: &Path) -> String {
    path.display().to_string()
}
