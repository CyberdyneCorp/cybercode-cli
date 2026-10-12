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
    /// Display one loaded server and its resolved output options without connecting.
    Get { name: String },
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
        McpCmd::Get { name } => {
            let mut value = server_report(&resolved, &store, ctx, &name)?;
            let definition = &mut value["definition"];
            definition["required"] = json!(definition["required"].as_bool().unwrap_or(false));
            let output = resolved.value.get("tool_output");
            value["output_budget"] = json!({
                "output_token_limit":value["definition"]["output_token_limit"],
                "max_lines":output.and_then(|o|o["max_lines"].as_u64()).unwrap_or(2000),
                "max_bytes":output.and_then(|o|o["max_bytes"].as_u64()).unwrap_or(51200)
            });
            report(global, &value)
        }
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
        servers.push(server_report(resolved, store, ctx, name)?);
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

fn server_report(
    resolved: &Resolved,
    store: &TrustStore,
    ctx: &Context,
    name: &str,
) -> Result<Value, CliError> {
    let server = inspect_server(resolved, &ctx.location, name).map_err(CliError::usage)?;
    let call_timeout = McpSettings::from_config(&resolved.value)
        .map_err(CliError::usage)?
        .call_timeout(name);
    let pointer = format!("/mcp/{name}");
    let prefix = format!("{pointer}/");
    let origins: std::collections::BTreeMap<_, _> = resolved
        .sources
        .iter()
        .filter(|(key, _)| *key == &pointer || key.starts_with(&prefix))
        .collect();
    let mut definition = redacted(
        &serde_json::to_value(&server.definition)
            .map_err(|error| CliError::runtime(error.to_string()))?,
        server.requires_approval,
    );
    if server.requires_approval {
        show_templates(&mut definition, resolved, &pointer);
    }
    if server.definition.headers_command().is_some() {
        definition["headers_refresh_seconds"] = json!(server.definition.headers_refresh_seconds());
    }
    Ok(
        json!({"name":name,"digest":server.digest,"call_timeout_seconds":call_timeout,"definition":definition,
            "origins":origins,"requires_individual_approval":server.requires_approval,"sandbox_required":server.requires_sandbox,
            "individually_approved":store.is_mcp_approved(&resolved.trust.checkout_root,&server.digest)?,
            "authorized":authorize_server(resolved,store,&ctx.location,name).is_ok()}),
    )
}

/// `review` keeps `headers_command` verbatim: a project-defined command is code that runs
/// outside the sandbox once its digest is approved, so it is shown like a local `command`.
fn redacted(definition: &Value, review: bool) -> Value {
    let mut value = cyber_core::config::redact_secrets(definition);
    // This numeric budget is not an authentication token.
    if let Some(limit) = definition
        .get("output_token_limit")
        .filter(|limit| limit.is_u64())
    {
        value["output_token_limit"] = limit.clone();
    }

    for key in ["env", "headers"] {
        if let Some(fields) = value.get_mut(key).and_then(Value::as_object_mut) {
            for field in fields.values_mut() {
                *field = json!("***");
            }
        }
    }
    // A user's own command line may embed credentials and needs no review.
    if value["headers_command"].is_string() {
        value["headers_command"] = if review {
            definition["headers_command"].clone()
        } else {
            json!("***")
        };
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

/// A project definition can pull local values into what it sends through `{env:}` and
/// `{file:}` placeholders. Review shows those leaves as their unsubstituted templates so
/// the approver sees which values would leave the machine; resolved values never print.
fn show_templates(definition: &mut Value, resolved: &Resolved, pointer: &str) {
    let prefix = format!("{pointer}/");
    for (key, template) in &resolved.templates {
        let Some(leaf) = key.strip_prefix(&prefix) else {
            continue;
        };
        // Leaves redacted to a non-object (e.g. `oauth`) have no path and stay redacted.
        if let Some(field) = definition
            .pointer_mut(&format!("/{leaf}"))
            .filter(|field| field.is_string())
        {
            *field = json!(template);
        }
    }
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
