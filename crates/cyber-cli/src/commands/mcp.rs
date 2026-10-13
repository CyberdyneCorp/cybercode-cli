//! MCP definition review, exact per-server approval and remote OAuth credentials, without
//! server startup.
use clap::Subcommand;
use cyber_core::config::{McpSettings, Resolved};
use cyber_core::trust::TrustStore;
use cyber_tools::mcp::{authorize_server, inspect_server, oauth};
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
    /// Authorize a remote server with OAuth: prints the authorization URL to open in a
    /// browser, waits for the loopback callback and stores the token privately.
    Auth {
        name: String,
        /// Loopback callback port (overrides `oauth.redirect_port`; default ephemeral).
        #[arg(long, value_parser = clap::value_parser!(u16).range(i64::from(cyber_core::config::OAUTH_REDIRECT_PORT_MIN)..))]
        port: Option<u16>,
    },
    /// Remove every stored OAuth credential of a server (at any URL, from any issuer) and
    /// ask a running server to reconnect without it. Tokens are not revoked remotely.
    Logout { name: String },
}

/// How long `cyber mcp auth` waits for the browser to reach the loopback callback.
const AUTH_CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub fn run(cmd: McpCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let store = TrustStore::new(ctx.paths.trust_file());
    if let McpCmd::Untrust { digest } = cmd {
        let root = std::fs::canonicalize(cyber_core::config::project_root(&ctx.location))?;
        let revoked = store.revoke_mcp(&root, &digest)?;
        return report(global, &json!({"digest":digest,"revoked":revoked}));
    }
    // Removing local credentials needs no configuration, so it also works for a server
    // that is no longer (or not yet) configured.
    if let McpCmd::Logout { name } = cmd {
        return logout(ctx, global, &name);
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
        McpCmd::Auth { name, port } => auth(&resolved, &store, ctx, global, &name, port),
        McpCmd::Untrust { .. } | McpCmd::Logout { .. } => {
            unreachable!("handled before configuration loading")
        }
    }
}

/// `cyber mcp auth`: never opens a browser; the URL goes to stderr for the user to open.
fn auth(
    resolved: &Resolved,
    store: &TrustStore,
    ctx: &Context,
    global: &GlobalArgs,
    name: &str,
    port: Option<u16>,
) -> Result<(), CliError> {
    if withheld(resolved, name) {
        return Err(CliError::usage("MCP checkout configuration is untrusted")
            .with_hint("review and approve it with cyber trust inspect / cyber trust approve"));
    }
    let request = oauth::AuthRequest {
        resolved,
        trust: store,
        location: &ctx.location,
        home: &ctx.home,
        name,
        port,
        timeout: AUTH_CALLBACK_TIMEOUT,
    };
    let credentials = oauth::FileCredentialStore::new(ctx.paths.mcp_auth_file());
    let present = |url: &str| {
        eprintln!("Open this URL in a browser to authorize MCP server {name}:\n{url}");
        eprintln!("Waiting for the authorization callback...");
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let outcome = runtime
        .block_on(oauth::authenticate(&request, &credentials, &present))
        .map_err(auth_error)?;
    let reconnect = runtime.block_on(reconnect_running(ctx, name));
    report(
        global,
        &json!({"name":name,"authenticated":true,"issuer":outcome.issuer,"scopes":outcome.scopes,
            "expires_at":outcome.expires_at,"registered":outcome.registered,"reconnect":reconnect}),
    )
}

/// `cyber mcp logout`: forget the server's credentials and step-up/registration markers,
/// then, when a credential was removed, ask a healthy registered server to reconnect it,
/// which settles `needs_auth`.
fn logout(ctx: &Context, global: &GlobalArgs, name: &str) -> Result<(), CliError> {
    use oauth::McpCredentialStore as _;
    // The name also becomes a request path segment.
    let valid = !name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c));
    if !valid {
        return Err(CliError::usage("invalid MCP server name"));
    }
    let credentials = oauth::FileCredentialStore::new(ctx.paths.mcp_auth_file());
    let removed = credentials
        .remove_server(name)
        .map_err(|error| CliError::runtime(error.to_string()))?;
    // With nothing removed no running connection changes, so nothing is reconnected.
    let reconnect = if removed == 0 {
        Value::Null
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(reconnect_running(ctx, name))
    };
    report(
        global,
        &json!({"name":name,"removed":removed,"reconnect":reconnect}),
    )
}

/// How long the best-effort reconnect request may take.
const RECONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Ask a healthy registered server to reconnect `name` with whatever is stored now: its
/// HTTP status and the server's status right after re-admission, or `null` when no
/// healthy server is registered. Never starts a server; no token is ever sent.
async fn reconnect_running(ctx: &Context, name: &str) -> Value {
    let Some(registered) =
        cyber_app::read_registration(&ctx.paths).filter(|registered| !registered.url.is_empty())
    else {
        return Value::Null;
    };
    let Ok(password) = std::fs::read_to_string(ctx.paths.state.join("password")) else {
        return Value::Null;
    };
    if password.trim().is_empty() || !cyber_app::health(&registered.url, cyber_app::version()).await
    {
        return Value::Null;
    }
    let client = cyber_client::Client::http(&registered.url, Some(password.trim().into()))
        .at(&ctx.location.display().to_string());
    let path = format!("/mcp/{name}/reconnect");
    let request = client.raw("POST", &path, None, &[]);
    match tokio::time::timeout(RECONNECT_TIMEOUT, request).await {
        Ok(Ok((status, value))) => {
            json!({"http_status":status,"status":value["data"]["status"],"error":value["message"]})
        }
        _ => json!({"http_status":null,"status":null,"error":"reconnect request failed"}),
    }
}

fn auth_error(error: oauth::OAuthError) -> CliError {
    match error {
        oauth::OAuthError::Unauthorized(_)
        | oauth::OAuthError::NotRemote
        | oauth::OAuthError::Disabled => CliError::usage(error.to_string()),
        oauth::OAuthError::NeedsClientRegistration => CliError::usage(error.to_string())
            .with_hint("register a client with the authorization server and set oauth.client_id"),
        error => CliError::runtime(error.to_string()),
    }
}

/// Whether a project definition of `name` (or the whole `mcp` section) is withheld from
/// an untrusted checkout. Entries are `<file>#<pointer>`.
fn withheld(resolved: &Resolved, name: &str) -> bool {
    let server = format!("/mcp/{name}");
    let covers = |pointer: &str| {
        pointer == "/mcp" || pointer == server || pointer.starts_with(&format!("{server}/"))
    };
    !resolved.trust.trusted
        && resolved
            .trust
            .definitions
            .iter()
            .filter_map(|entry| entry.rsplit_once('#'))
            .any(|(_, pointer)| covers(pointer))
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
    // Client id, scopes and port are reviewable; `client_secret` is already redacted by
    // the generic secret-key rule and only its `{env:}` template may be shown in review.
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
        // Leaves redacted to a non-object have no path and stay redacted.
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
