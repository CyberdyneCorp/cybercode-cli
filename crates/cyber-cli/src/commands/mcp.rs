//! MCP definition review and exact per-server approval without server startup.
use clap::Subcommand;
use cyber_core::config::{McpSettings, Resolved};
use cyber_core::trust::TrustStore;
use cyber_tools::mcp::{authorize_server, inspect_server};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum McpCmd {
    /// Review loaded server definitions, origins and digests without connecting.
    Definitions,
    /// Approve an inspected, currently loaded project server definition.
    Trust {
        name: String,
        #[arg(long)]
        digest: String,
    },
    /// Revoke an individual server digest, including an obsolete definition.
    Untrust {
        #[arg(long)]
        digest: String,
    },
}

pub fn run(cmd: McpCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let store = TrustStore::new(ctx.paths.trust_file());
    if let McpCmd::Untrust { digest } = cmd {
        let root = std::fs::canonicalize(cyber_core::config::project_root(&ctx.location))?;
        let revoked = store.revoke_mcp(&root, &digest)?;
        return report(global, &json!({"digest":digest,"revoked":revoked}));
    }
    let resolved = ctx.config()?;
    match cmd {
        McpCmd::Definitions => definitions(&resolved, &store, ctx, global),
        McpCmd::Trust { name, digest } => {
            let server =
                inspect_server(&resolved, &ctx.location, &name).map_err(CliError::usage)?;
            if server.digest != digest || !server.requires_approval {
                return Err(CliError::usage("digest does not match a currently loaded project MCP server")
                    .with_hint("inspect cyber mcp definitions; approve withheld checkout configuration with cyber trust first"));
            }
            let approved = resolved
                .trust
                .digest
                .as_ref()
                .map(|digest| store.is_approved(&resolved.trust.checkout_root, digest))
                .transpose()?
                .unwrap_or(false);
            if !resolved.trust.trusted || !approved {
                return Err(CliError::usage("MCP checkout configuration is untrusted"));
            }
            store.approve_mcp(&resolved.trust.checkout_root, &digest)?;
            report(
                global,
                &json!({"name":name,"digest":digest,"approved":true}),
            )
        }
        McpCmd::Untrust { .. } => unreachable!("handled before configuration loading"),
    }
}

fn definitions(
    resolved: &Resolved,
    store: &TrustStore,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let settings = McpSettings::from_config(&resolved.value).map_err(CliError::usage)?;
    let mut servers = Vec::new();
    for name in settings.servers.keys() {
        let server = inspect_server(resolved, &ctx.location, name).map_err(CliError::usage)?;
        let pointer = format!("/mcp/{name}");
        let prefix = format!("{pointer}/");
        let origins: std::collections::BTreeMap<_, _> = resolved
            .sources
            .iter()
            .filter(|(key, _)| *key == &pointer || key.starts_with(&prefix))
            .collect();
        servers.push(json!({"name":name,"digest":server.digest,"definition":redacted(&serde_json::to_value(&server.definition).map_err(|error| CliError::runtime(error.to_string()))?),
            "origins":origins,"requires_individual_approval":server.requires_approval,"sandbox_required":server.requires_sandbox,
            "individually_approved":store.is_mcp_approved(&resolved.trust.checkout_root,&server.digest)?,
            "authorized":authorize_server(resolved,store,&ctx.location,name).is_ok()}));
    }
    let withheld: Vec<_> = resolved
        .trust
        .definitions
        .iter()
        .filter(|entry| entry.contains("/mcp") && !resolved.trust.trusted)
        .collect();
    report(
        global,
        &json!({"servers":servers,"withheld_definitions":withheld}),
    )
}

fn redacted(definition: &Value) -> Value {
    let mut value = cyber_core::config::redact_secrets(definition);
    for key in ["env", "headers"] {
        if let Some(fields) = value.get_mut(key).and_then(Value::as_object_mut) {
            for field in fields.values_mut() {
                *field = json!("***");
            }
        }
    }
    if value["oauth"].is_object() {
        value["oauth"] = json!("***");
    }
    if let Some(url) = value["url"]
        .as_str()
        .and_then(|url| url.split_once('?').map(|(url, _)| format!("{url}?***")))
    {
        value["url"] = json!(url);
    }
    value
}

fn report(global: &GlobalArgs, value: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(value);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|error| CliError::runtime(error.to_string()))?
    );
    Ok(())
}
