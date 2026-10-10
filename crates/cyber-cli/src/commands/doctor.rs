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
    let config = config.map(|c| c.value).unwrap_or(Value::Null);
    checks.extend(lsp_checks(ctx, &config));
    checks.extend(rt.block_on(catalog_checks(ctx, &config)));
    checks.push(database(ctx));
    checks.push(sandbox());
    checks.push(tool("git", &["--version"], Status::Fail));
    checks.push(tool("rg", &["--version"], Status::Warn));
    checks.push(rt.block_on(server(ctx)));
    report(global, &checks)
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
            let detail = match (server.installed, disabled) {
                (false, true) => "disabled; not installed",
                (false, false) => "not installed",
                (true, true) => "installed; disabled",
                (true, false) => "installed",
            };
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
        Some(reg) if cyber_app::health(&reg.url, cyber_app::version()).await => check(
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
