//! `cyber models` (`provider-catalog` → Models command and API).

use clap::Args;
use cyber_llm::catalog::{
    Availability, BuildInputs, Catalog, CatalogError, Listing, ModelRole, SourceOptions,
    default_model, load_source, recent_models, role_ref,
};
use serde_json::{Map, Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Args)]
pub struct ModelsArgs {
    /// Only this provider (`list` and `refresh` are accepted as verbs).
    pub provider: Option<String>,
    /// Show limits, prices and variants.
    #[arg(long)]
    pub verbose: bool,
    /// Refetch the catalog even when the cache is fresh.
    #[arg(long)]
    pub refresh: bool,
}

pub fn run(args: ModelsArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let (provider, refresh) = match args.provider.as_deref() {
        Some("list") => (None, args.refresh),
        Some("refresh") => (None, true),
        other => (other.map(str::to_string), args.refresh),
    };
    let config = ctx.config()?.value;
    let mut source = SourceOptions::from_env(&ctx.env, &ctx.paths.cache);
    source.refresh = refresh;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let loaded = runtime
        .block_on(load_source(&source))
        .map_err(catalog_error)?;
    if let Some(warning) = &loaded.warning {
        eprintln!("warning: {warning}");
    }
    let catalog = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &config,
        env: &ctx.env,
    });
    let rows = catalog.list(provider.as_deref()).map_err(catalog_error)?;
    if output::is_json(global.format) {
        let recent = recent_models(&ctx.paths.state.join("model.json"));
        let default = default_model(&catalog, &config, &recent)
            .ok()
            .map(|(r, _)| r.to_string());
        return output::json(&json!({
            "origin": loaded.origin,
            "default": default,
            "roles": roles(&config),
            "models": rows.iter().map(row_json).collect::<Vec<_>>(),
        }));
    }
    for row in &rows {
        println!("{}", line(row, args.verbose));
    }
    Ok(())
}

fn catalog_error(e: CatalogError) -> CliError {
    CliError::runtime(e.to_string())
}

fn roles(config: &Value) -> Value {
    let map: Map<String, Value> = ModelRole::ALL
        .iter()
        .map(|r| {
            (
                r.key().to_string(),
                role_ref(config, *r).map_or(Value::Null, Value::String),
            )
        })
        .collect();
    Value::Object(map)
}

fn line(row: &Listing<'_>, verbose: bool) -> String {
    let mut text = row.model_ref.clone();
    if let Availability::Unavailable(reason) = &row.availability {
        text.push_str(&format!("  ({reason})"));
    }
    if verbose {
        let m = row.model;
        let price = m.cost.as_ref().map_or("unpriced".to_string(), |c| {
            format!("${}/${} per Mtok", c.input, c.output)
        });
        let variants = m.variants.keys().cloned().collect::<Vec<_>>().join(",");
        text.push_str(&format!(
            "  ctx={} out={} {price}",
            m.limits.context, m.limits.output
        ));
        if !variants.is_empty() {
            text.push_str(&format!(" variants={variants}"));
        }
    }
    text
}

fn row_json(row: &Listing<'_>) -> Value {
    let m = row.model;
    let (available, reason) = match &row.availability {
        Availability::Available => (true, Value::Null),
        Availability::Unavailable(r) => (false, json!(r)),
    };
    json!({
        "ref": row.model_ref,
        "name": m.name,
        "available": available,
        "reason": reason,
        "limits": m.limits,
        "cost": m.cost,
        "capabilities": m.capabilities,
        "variants": m.variants.keys().collect::<Vec<_>>(),
        "released_at": m.released_at,
    })
}
