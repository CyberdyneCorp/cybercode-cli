//! `cyber doctor` (`cli-commands` → Doctor and debug commands): pass/warn/fail checks,
//! exit 1 when any check fails.

use cyber_core::paths::DatabaseLocation;
use cyber_llm::catalog::{
    BuildInputs, Catalog, CredentialSource, Origin, SourceOptions, load_source,
};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
}

struct Check {
    name: String,
    status: Status,
    detail: String,
}

fn check(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}

pub fn run(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let rt = super::serve::runtime()?;
    let config = ctx.config();
    let mut checks = vec![match &config {
        Ok(_) => check("config", Status::Pass, "parsed and valid"),
        Err(e) => check("config", Status::Fail, e.message.clone()),
    }];
    checks.extend(rt.block_on(extension_checks(ctx)));
    checks.push(instructions(ctx, config.as_ref().ok()));
    let config = config.map(|c| c.value).unwrap_or(Value::Null);
    checks.extend(lsp_checks(ctx, &config));
    checks.extend(rt.block_on(catalog_checks(ctx, &config)));
    checks.push(database(ctx));
    checks.push(sandbox());
    checks.push(tool("git", &["--version"], Status::Fail));
    checks.push(tool("rg", &["--version"], Status::Warn));
    checks.push(rt.block_on(server(ctx)));
    checks.extend(rt.block_on(peer_checks(ctx)));
    checks.push(rt.block_on(account_check(ctx)));
    report(global, &checks)
}

/// Skill and command diagnostics as the app itself discovers them for this Location.
async fn extension_checks(ctx: &Context) -> Vec<Check> {
    let catalog = match super::skills::Inspector::start(ctx).await {
        Ok(inspector) => {
            let catalog = inspector.skills("build").await;
            inspector.shutdown().await;
            catalog
        }
        Err(error) => Err(error),
    };
    let catalog = match catalog {
        Ok(catalog) => catalog,
        Err(error) => {
            let detail = format!("unavailable: {}", error.message);
            return vec![
                check("skills", Status::Warn, detail.clone()),
                check("commands", Status::Warn, detail),
            ];
        }
    };
    vec![skills_check(&catalog), commands_check(&catalog)]
}

fn skills_check(catalog: &Value) -> Check {
    let entries = catalog["skills"].as_array().map_or(&[][..], Vec::as_slice);
    let loaded = entries.iter().filter(|e| e["status"] == "loaded").count();
    let skipped = entries.len() - loaded;
    let preload = catalog["preload_issues"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let diagnostics: Vec<&str> = entries
        .iter()
        .flat_map(|e| e["diagnostics"].as_array().into_iter().flatten())
        .chain(preload)
        .filter_map(Value::as_str)
        .collect();
    let mut detail = format!("{loaded} loaded");
    if !diagnostics.is_empty() {
        detail.push_str(&format!("; {}", diagnostics.join("; ")));
    }
    let status = if skipped == 0 && preload.is_empty() {
        Status::Pass
    } else {
        Status::Warn
    };
    check("skills", status, detail)
}

fn commands_check(catalog: &Value) -> Check {
    let issues: Vec<String> = catalog["command_issues"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|issue| {
            format!(
                "{}: {}",
                issue["path"].as_str().unwrap_or(""),
                issue["reason"].as_str().unwrap_or("")
            )
        })
        .collect();
    if issues.is_empty() {
        check("commands", Status::Pass, "no discovery issues")
    } else {
        check("commands", Status::Warn, issues.join("; "))
    }
}

/// The instruction files the next Session at this Location loads, in load order, with a
/// warning for truncated files and unresolved or refused imports. `config` is `None` when
/// the Location's config failed to load: like the runtime, `CLAUDE.md` is then off and no
/// project file passes the read gate.
fn instructions(ctx: &Context, config: Option<&cyber_core::config::Resolved>) -> Check {
    use cyber_server::runtime::instructions::{InstructionSettings, load_baseline};
    // The same per-Location `compat.claude` and `read` rules the runtime reads through its
    // tool host.
    let compat = config.is_some_and(|c| cyber_core::config::claude_compat(&c.value));
    let mut settings = InstructionSettings::from_env(compat, &ctx.env);
    settings.read_gate = Some(cyber_tools::permissions::instruction_read_gate(
        config.map(|c| (&c.value, &c.sources)),
        &ctx.location,
    ));
    let loaded = match load_baseline(&ctx.paths.config, &ctx.location, &settings) {
        Ok(loaded) => loaded,
        Err(reason) => {
            return check(
                "instructions",
                Status::Warn,
                format!("unavailable: {reason}"),
            );
        }
    };
    let files: Vec<String> = loaded
        .files
        .iter()
        .map(|file| file.path.display().to_string())
        .collect();
    let mut detail = match files.len() {
        0 => "no instruction files".to_owned(),
        n => format!("{n} loaded: {}", files.join(", ")),
    };
    if loaded.warnings.is_empty() {
        return check("instructions", Status::Pass, detail);
    }
    detail.push_str(&format!("; {}", loaded.warnings.join("; ")));
    check("instructions", Status::Warn, detail)
}

fn lsp_checks(ctx: &Context, config: &Value) -> Vec<Check> {
    let Ok(settings) = cyber_core::config::LspSettings::from_config(config) else {
        return Vec::new();
    };
    let search =
        cyber_core::intelligence::ExecutableSearch::new(&ctx.location, &ctx.paths.cache, &ctx.env);
    cyber_core::intelligence::detect_servers(&settings, &search)
        .into_iter()
        .map(|server| {
            let disabled = !settings.enabled
                || settings
                    .servers
                    .get(&server.definition.id)
                    .is_some_and(|s| s.disabled);
            let detail = lsp_detail(server.installed, disabled, settings.auto_install);
            check(
                format!("lsp:{}", server.definition.id),
                if server.installed || disabled {
                    Status::Pass
                } else {
                    Status::Warn
                },
                detail,
            )
        })
        .collect()
}

/// `lsp.auto_install` is accepted but never downloads: missing servers stay missing.
fn lsp_detail(installed: bool, disabled: bool, auto_install: bool) -> &'static str {
    match (installed, disabled) {
        (false, true) => "disabled; not installed",
        (false, false) if auto_install => "not installed; auto_install does not download servers",
        (false, false) => "not installed",
        (true, true) => "installed; disabled",
        (true, false) => "installed",
    }
}

async fn catalog_checks(ctx: &Context, config: &Value) -> Vec<Check> {
    let mut source = SourceOptions::from_env(&ctx.env, &ctx.paths.cache);
    source.allow_fetch = false;
    let Ok(loaded) = load_source(&source).await else {
        return vec![check(
            "catalog",
            Status::Fail,
            "no model catalog could be loaded",
        )];
    };
    let freshness = match loaded.origin {
        Origin::FreshCache | Origin::Fetched | Origin::Path => {
            check("catalog", Status::Pass, "fresh")
        }
        Origin::StaleCache => check(
            "catalog",
            Status::Warn,
            "cached catalog is stale; run `cyber models refresh`",
        ),
        Origin::Snapshot => check(
            "catalog",
            Status::Warn,
            "using the bundled snapshot; run `cyber models refresh`",
        ),
    };
    let catalog = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config,
        env: &ctx.env,
    });
    vec![freshness, credentials(&catalog)]
}

/// Providers with a key. Local endpoints need none, so they prove nothing about keys.
fn credentials(catalog: &Catalog) -> Check {
    let (mut keyed, mut local): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    for id in catalog.providers.keys() {
        match catalog.credential(id).map(|c| &c.source) {
            Some(CredentialSource::NotRequired) => local.push(id),
            Some(_) => keyed.push(id),
            None => {}
        }
    }
    let note = if local.is_empty() {
        String::new()
    } else {
        format!("; local endpoints (not checked): {}", local.join(", "))
    };
    if keyed.is_empty() {
        check(
            "credentials",
            Status::Fail,
            format!(
                "no provider has an API key; set one such as OPENAI_API_KEY or ANTHROPIC_API_KEY{note}"
            ),
        )
    } else {
        check(
            "credentials",
            Status::Pass,
            format!("{}{note}", keyed.join(", ")),
        )
    }
}

fn database(ctx: &Context) -> Check {
    let DatabaseLocation::File(path) = ctx.database() else {
        return check("database", Status::Pass, "in memory");
    };
    if !path.exists() {
        return check(
            "database",
            Status::Pass,
            format!("not created yet ({})", path.display()),
        );
    }
    let quick =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|c| c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)));
    match quick {
        Ok(ok) if ok == "ok" => check(
            "database",
            Status::Pass,
            format!("quick_check ok ({})", path.display()),
        ),
        Ok(problem) => check("database", Status::Fail, format!("quick_check: {problem}")),
        Err(e) => check("database", Status::Fail, e.to_string()),
    }
}

fn sandbox() -> Check {
    if cyber_sandbox::available() {
        return check("sandbox", Status::Pass, "OS enforcement available");
    }
    let detail = if cfg!(target_os = "linux") {
        "bubblewrap not found; install bubblewrap or run with --sandbox full-access"
    } else {
        "no sandbox enforcement on this platform; commands need --sandbox full-access"
    };
    check("sandbox", Status::Fail, detail)
}

fn tool(name: &'static str, args: &[&str], missing: Status) -> Check {
    match std::process::Command::new(name).args(args).output() {
        Ok(out) if out.status.success() => check(
            name,
            Status::Pass,
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
        ),
        _ => check(name, missing, format!("{name} not found on PATH")),
    }
}

async fn server(ctx: &Context) -> Check {
    match cyber_app::read_registration(&ctx.paths) {
        None => check("server", Status::Pass, "not running (starts on demand)"),
        Some(reg) if cyber_app::registered_health(&reg, cyber_app::version()).await => check(
            "server",
            Status::Pass,
            format!("healthy at {} (pid {})", reg.url, reg.pid),
        ),
        Some(reg) => check(
            "server",
            Status::Warn,
            format!(
                "registered at {} but not healthy; run `cyber service restart`",
                reg.url
            ),
        ),
    }
}

/// The peer section (`cross-session-messaging` → Direct peer transport): the listener's
/// address, TLS and fingerprint with the active grant count, and the remotes whose token
/// is in the file fallback rather than the OS keyring or config.
async fn peer_checks(ctx: &Context) -> Vec<Check> {
    let settings = match super::peers::settings(ctx) {
        Ok(settings) => settings,
        Err(error) => return vec![check("peers", Status::Warn, error.message)],
    };
    vec![
        peer_listener(ctx, settings.listen.as_ref()).await,
        peer_remotes(ctx, &settings.remotes),
    ]
}

async fn peer_listener(ctx: &Context, listen: Option<&cyber_core::config::PeerListen>) -> Check {
    let name = "peers:listener";
    let running = match cyber_app::read_registration(&ctx.paths) {
        Some(reg) if cyber_app::registered_health(&reg, cyber_app::version()).await => Some(reg),
        _ => None,
    };
    let Some(reg) = running else {
        return match listen {
            None => check(name, Status::Pass, "off (peers.listen not set)"),
            Some(listen) => check(
                name,
                Status::Pass,
                format!(
                    "{} · TLS {} · opens when the server starts",
                    listen.address,
                    on_off(listen.tls)
                ),
            ),
        };
    };
    let overview = match peers_overview(ctx, &reg.url).await {
        Ok(overview) => overview,
        Err(error) => return check(name, Status::Warn, format!("unavailable: {error}")),
    };
    let grants = overview["grants"]
        .as_array()
        .map_or(0, |g| g.iter().filter(|g| g["active"] == true).count());
    let listener = &overview["listener"];
    match (listener["url"].as_str(), listen) {
        (Some(url), _) => check(
            name,
            Status::Pass,
            format!(
                "{url} · TLS {} · fingerprint {} · {grants} active grant{}",
                on_off(listener["tls"] == true),
                listener["fingerprint"].as_str().unwrap_or_default(),
                if grants == 1 { "" } else { "s" },
            ),
        ),
        (None, None) => check(name, Status::Pass, "off (peers.listen not set)"),
        (None, Some(listen)) => check(
            name,
            Status::Warn,
            format!(
                "peers.listen is {} but the server has no peer listener; see `peer listener not started` in its log",
                listen.address
            ),
        ),
    }
}

/// `GET /api/v1/peers` of the running server.
async fn peers_overview(ctx: &Context, url: &str) -> Result<Value, String> {
    let password = cyber_app::password(&ctx.paths)?;
    let client = cyber_client::Client::http(url, Some(password));
    let (status, body) = client
        .raw("GET", "/peers", None, &[])
        .await
        .map_err(|e| e.to_string())?;
    if status == 200 {
        Ok(body["data"].clone())
    } else {
        Err(format!("GET /peers answered HTTP {status}"))
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off (loopback only)" }
}

fn peer_remotes(ctx: &Context, remotes: &[cyber_core::config::PeerRemote]) -> Check {
    let name = "peers:remotes";
    if remotes.is_empty() {
        return check(name, Status::Pass, "none");
    }
    let names: Vec<&str> = remotes.iter().map(|r| r.name.as_str()).collect();
    let stored = match cyber_app::peer_tokens::file_token_names(&ctx.paths) {
        Ok(stored) => stored,
        Err(error) => return check(name, Status::Warn, format!("{}; {error}", names.join(", "))),
    };
    let in_file: Vec<&str> = remotes
        .iter()
        .filter(|r| r.token.is_none() && stored.contains(&r.name))
        .map(|r| r.name.as_str())
        .collect();
    if in_file.is_empty() {
        return check(name, Status::Pass, names.join(", "));
    }
    check(
        name,
        Status::Warn,
        format!(
            "{}; tokens in the file fallback (no OS keyring): {} in {}",
            names.join(", "),
            in_file.join(", "),
            ctx.paths.peer_tokens_file().display()
        ),
    )
}

fn report(global: &GlobalArgs, checks: &[Check]) -> Result<(), CliError> {
    let failed = checks.iter().any(|c| c.status == Status::Fail);
    if output::is_json(global.format) {
        let rows: Vec<Value> = checks.iter().map(|c| json!({ "check": c.name, "status": format!("{:?}", c.status).to_lowercase(), "detail": c.detail })).collect();
        output::json(&json!({ "ok": !failed, "checks": rows }))?;
    } else {
        for c in checks {
            println!(
                "{}: {} {}",
                c.name,
                format!("{:?}", c.status).to_lowercase(),
                c.detail
            );
        }
    }
    if failed {
        Err(CliError::silent(1))
    } else {
        Ok(())
    }
}

async fn account_check(ctx: &Context) -> Check {
    match super::account::store(ctx).storage_kind().await {
        Ok(Some(cyber_account::StorageKind::File)) => check(
            "account",
            Status::Warn,
            "account tokens stored in file (no OS keyring)",
        ),
        Ok(Some(cyber_account::StorageKind::Keyring)) => {
            check("account", Status::Pass, "credentials in OS keyring")
        }
        Ok(None) => check(
            "account",
            Status::Pass,
            "not logged in; local use needs no account",
        ),
        Err(error) => check("account", Status::Warn, error.to_string()),
    }
}
