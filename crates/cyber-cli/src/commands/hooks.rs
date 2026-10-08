//! Review resolved hook definitions and approve individual checkout-scoped digests.

use clap::Subcommand;
use cyber_core::hooks::{HookCatalog, HookDefinition};
use cyber_core::trust::TrustStore;
use serde::Serialize;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum HooksCmd {
    /// List resolved definitions, file origins, digests and individual trust state.
    List,
    /// Inspect committed Session execution receipts without running hooks or recovery.
    History {
        #[arg(long)]
        session: String,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Approve a currently resolved project/local handler after inspecting its digest.
    Trust {
        #[arg(long)]
        digest: String,
    },
    /// Revoke a handler digest, including an obsolete definition.
    Untrust {
        #[arg(long)]
        digest: String,
    },
}

#[derive(Serialize)]
struct ListedHook<'a> {
    #[serde(flatten)]
    definition: &'a HookDefinition,
    trusted: bool,
    sandbox_required: bool,
    last_run: Option<cyber_core::hooks::HookLastRun>,
}

pub fn run(cmd: HooksCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if let HooksCmd::History {
        session,
        limit,
        cursor,
    } = &cmd
    {
        return history(ctx, global, session, *limit, cursor.as_deref());
    }
    let store = TrustStore::new(ctx.paths.trust_file());
    if let HooksCmd::Untrust { digest } = cmd {
        let root = std::fs::canonicalize(cyber_core::config::project_root(&ctx.location))?;
        let removed = store.revoke_hook(&root, &digest)?;
        return report(
            global,
            &serde_json::json!({"digest":digest,"revoked":removed}),
        );
    }
    let resolved = ctx.config()?;
    let catalog = HookCatalog::from_config(&resolved).map_err(CliError::usage)?;
    match cmd {
        HooksCmd::List => list(
            &catalog,
            &resolved,
            &store,
            global,
            &ctx.withheld_hooks()?,
            ctx,
        ),
        HooksCmd::Trust { digest } => approve(&catalog, &resolved, &store, &digest, global),
        HooksCmd::Untrust { .. } | HooksCmd::History { .. } => {
            unreachable!("handled before configuration loading")
        }
    }
}

fn list(
    catalog: &HookCatalog,
    resolved: &cyber_core::config::Resolved,
    store: &TrustStore,
    global: &GlobalArgs,
    raw: &[cyber_core::config::RawHookSection],
    ctx: &Context,
) -> Result<(), CliError> {
    let database = review_database(ctx)?;
    let hooks: Vec<_> = catalog
        .definitions
        .iter()
        .map(|definition| {
            Ok(ListedHook {
                definition,
                trusted: definition.is_trusted(&resolved.trust.checkout_root, store, None)?,
                sandbox_required: definition
                    .scope
                    .requires_sandbox(catalog.settings.sandbox_all),
                last_run: database
                    .as_ref()
                    .map(|store| {
                        cyber_server::runtime::hook_last_run(store, &ctx.location, definition)
                    })
                    .transpose()
                    .map_err(|error| CliError::runtime(error.to_string()))?
                    .flatten(),
            })
        })
        .collect::<Result<_, CliError>>()?;
    let withheld = if resolved.trust.trusted {
        &[][..]
    } else {
        resolved.trust.definitions.as_slice()
    };
    if output::is_json(global.format) {
        let value =
            serde_json::json!({"hooks":hooks,"withheld_definitions":withheld,"withheld_hooks":raw});
        return output::json(&cyber_core::config::redact_secrets(&value));
    }
    for hook in hooks {
        println!(
            "{} {:?} {:?} {} {}",
            hook.definition.event,
            hook.definition.scope,
            hook.definition.kind(),
            if hook.trusted { "trusted" } else { "untrusted" },
            hook.definition.digest
        );
        println!("  {} {}", hook.definition.source, hook.definition.pointer);
        match hook.last_run {
            Some(run) => println!(
                "  last run: {} started_ms={} outcome={} duration_ms={} acknowledged={} must_stop={} session={}",
                run_status(&run.status),
                run.started_ms,
                run.outcome
                    .as_ref()
                    .map(|value| format!("{value:?}").to_lowercase())
                    .unwrap_or_else(|| "pending".into()),
                run.duration_ms
                    .map_or("pending".into(), |value| value.to_string()),
                run.acknowledged
                    .map_or("unverified", |value| if value { "yes" } else { "no" }),
                run.must_stop,
                run.session_id.escape_debug()
            ),
            None => println!("  last run: no recorded execution for this definition/checkout"),
        }
    }
    for definition in withheld {
        println!("withheld until checkout configuration approval: {definition}");
    }
    for section in raw {
        println!(
            "Withheld {:?} {}#{} (literal, inactive; no handler approval digest)",
            section.scope,
            section.source.escape_debug(),
            section.pointer.escape_debug()
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&section.value)
                .map_err(|error| CliError::usage(error.to_string()))?
                .escape_debug()
        );
    }
    Ok(())
}

fn run_status(status: &cyber_core::hooks::HookRunStatus) -> &'static str {
    match status {
        cyber_core::hooks::HookRunStatus::Running => "running (live state unverified)",
        cyber_core::hooks::HookRunStatus::Unknown => "unknown (recovery required)",
        cyber_core::hooks::HookRunStatus::Completed => "completed",
    }
}

fn review_database(ctx: &Context) -> Result<Option<cyber_store::Store>, CliError> {
    let location = ctx.database();
    let cyber_core::paths::DatabaseLocation::File(path) = &location else {
        return Ok(None);
    };
    if !path.try_exists()? {
        return Ok(None);
    }
    Ok(Some(cyber_store::Store::open(
        cyber_store::StoreOptions::new(location, cyber_server::runtime::Runtime::registry()),
    )?))
}

fn approve(
    catalog: &HookCatalog,
    resolved: &cyber_core::config::Resolved,
    store: &TrustStore,
    digest: &str,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    if !catalog
        .definitions
        .iter()
        .any(|definition| definition.digest == digest && definition.scope.requires_handler_trust())
    {
        return Err(CliError::usage("digest does not match a currently resolved project/local hook")
            .with_hint("inspect with cyber hooks list; withheld configuration requires cyber trust approval first"));
    }
    store.approve_hook(&resolved.trust.checkout_root, digest)?;
    report(
        global,
        &serde_json::json!({"digest":digest,"approved":true}),
    )
}

fn report(global: &GlobalArgs, value: &serde_json::Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(value);
    }
    println!("{value}");
    Ok(())
}

fn history(
    ctx: &Context,
    global: &GlobalArgs,
    session: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<(), CliError> {
    use cyber_server::runtime::{Runtime, RuntimeError, hook_execution_page};
    use cyber_store::{Store, StoreOptions};
    let store = Store::open(StoreOptions::new(ctx.database(), Runtime::registry()))?;
    let (data, next) =
        hook_execution_page(&store, session, limit, cursor).map_err(|error| match error {
            RuntimeError::Invalid(message) => CliError::usage(message),
            other => CliError::runtime(other.to_string()),
        })?;
    if output::is_json(global.format) {
        return output::json(&serde_json::json!({"data":data,"cursor":{"next":next}}));
    }
    for record in data {
        let observation = match record.status {
            cyber_server::runtime::HookExecutionStatus::Running => {
                "running (live state unverified)"
            }
            cyber_server::runtime::HookExecutionStatus::Unknown => "unknown (recovery required)",
            cyber_server::runtime::HookExecutionStatus::Completed => "completed",
        };
        println!(
            "{} {} {} event={:?} hook={:?} outcome={:?} duration_ms={:?} acknowledged={:?} must_stop={}",
            record.id,
            record.started_ms,
            observation,
            record.event,
            record.hook_id,
            record.outcome,
            record.duration_ms,
            record.acknowledged,
            record.must_stop
        );
    }
    if let Some(next) = next {
        println!("next cursor: {next}");
    }
    Ok(())
}
