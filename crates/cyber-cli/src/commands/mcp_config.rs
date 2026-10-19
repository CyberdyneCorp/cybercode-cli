//! `cyber mcp list|add|remove` (`mcp` → MCP CLI and UI): live status from the shared
//! server, and comment-preserving edits of the user, project or local config document.
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use cyber_client::Client;
use serde_json::{Map, Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

/// Where `cyber mcp add` writes and `remove` looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// The global config document (`~/.config/cyber/cyber.jsonc`).
    User,
    /// The project's `.cyber/cyber.jsonc`, shared through the repository.
    Project,
    /// The project's `.cyber/cyber.local.jsonc`, for this checkout only.
    Local,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Server name (letters, digits, `-` and `_`).
    pub name: String,
    #[arg(long, value_enum, default_value_t = Scope::User)]
    pub scope: Scope,
    /// A remote (Streamable HTTP) server URL.
    #[arg(long, conflicts_with = "command")]
    pub url: Option<String>,
    /// A request header for a remote server, as `Name=value` (repeatable).
    #[arg(long = "header", value_name = "K=V", requires = "url")]
    pub headers: Vec<String>,
    /// An environment variable for a local server, as `NAME=value` (repeatable).
    #[arg(long = "env", value_name = "K=V")]
    pub env: Vec<String>,
    /// A local (stdio) server: the command and its arguments, after `--`.
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

pub fn add(args: AddArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    valid_name(&args.name)?;
    let definition = definition(&args)?;
    let path = document(ctx, args.scope)?;
    if defined_in(&path, &args.name)? {
        return Err(CliError::usage(format!(
            "MCP server {} is already defined in {}",
            args.name,
            path.display()
        ))
        .with_hint(format!(
            "remove it first with cyber mcp remove {}",
            args.name
        )));
    }
    cyber_core::config::set_document_member(&path, "mcp", &args.name, Some(&definition))
        .map_err(CliError::runtime)?;
    let digest = serde_json::from_value::<cyber_core::config::McpServer>(definition.clone())
        .ok()
        .and_then(|server| server.digest(&args.name).ok());
    let approval = (args.scope != Scope::User).then(|| {
        format!(
            "project servers start after approval: cyber mcp trust {} --digest {}",
            args.name,
            digest.as_deref().unwrap_or("<digest>")
        )
    });
    if output::is_json(global.format) {
        return output::json(&json!({
            "name": args.name, "file": path, "definition": definition,
            "digest": digest, "approval": approval,
        }));
    }
    println!("added MCP server {} to {}", args.name, path.display());
    if let Some(approval) = approval {
        println!("{approval}");
    }
    println!("it connects when a Session opens this Location (cyber mcp list shows its status)");
    Ok(())
}

pub fn remove(
    name: &str,
    scope: Option<Scope>,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let scopes = scope.map_or_else(
        || vec![Scope::User, Scope::Project, Scope::Local],
        |s| vec![s],
    );
    let mut found = Vec::new();
    for scope in scopes {
        let path = document(ctx, scope)?;
        if defined_in(&path, name)? {
            found.push(path);
        }
    }
    let path = match found.as_slice() {
        [] => {
            return Err(CliError::usage(format!(
                "MCP server {name} is not defined in an editable config document"
            ))
            .with_hint("cyber mcp definitions shows where each server comes from"));
        }
        [path] => path.clone(),
        _ => {
            return Err(CliError::usage(format!(
                "MCP server {name} is defined in several documents: {}",
                found
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .with_hint("pick one with --scope user|project|local"));
        }
    };
    cyber_core::config::set_document_member(&path, "mcp", name, None).map_err(CliError::runtime)?;
    if output::is_json(global.format) {
        return output::json(&json!({ "name": name, "file": path, "removed": true }));
    }
    println!("removed MCP server {name} from {}", path.display());
    Ok(())
}

pub fn list(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let rt = super::serve::runtime()?;
    let data = rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        client
            .get("/mcp")
            .await
            .map_err(|e| CliError::runtime(e.to_string()))
    })?;
    let servers: Vec<Value> = data["data"].as_array().cloned().unwrap_or_default();
    if output::is_json(global.format) {
        return output::json(&servers);
    }
    if servers.is_empty() {
        println!("no MCP servers configured");
        println!(
            "add one: cyber mcp add <name> --url <url>   or   cyber mcp add <name> -- <command> [args...]"
        );
        return Ok(());
    }
    for server in &servers {
        let tools = server["tools"].as_array().map_or(0, Vec::len);
        let error = server["error"].as_str().map(clean).unwrap_or_default();
        println!(
            "{:<20} {:<7} {:<26} {:>3} tools  {}",
            clean(server["name"].as_str().unwrap_or_default()),
            server["transport"].as_str().unwrap_or("-"),
            server["status"].as_str().unwrap_or_default(),
            tools,
            error
        );
    }
    Ok(())
}

pub(crate) fn valid_name(name: &str) -> Result<(), CliError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "invalid MCP server name \"{name}\": use letters, digits, `-` and `_`"
    )))
}

fn definition(args: &AddArgs) -> Result<Value, CliError> {
    match (&args.url, args.command.as_slice()) {
        (Some(url), []) => {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(CliError::usage(format!("--url must be http(s): {url}")));
            }
            if !args.env.is_empty() {
                return Err(CliError::usage("--env applies to local servers only"));
            }
            let mut server = json!({ "type": "remote", "url": url });
            if !args.headers.is_empty() {
                server["headers"] = Value::Object(pairs(&args.headers, "--header")?);
            }
            Ok(server)
        }
        (None, [command, rest @ ..]) => {
            let mut server = json!({ "type": "local", "command": command });
            if !rest.is_empty() {
                server["args"] = json!(rest);
            }
            if !args.env.is_empty() {
                server["env"] = Value::Object(pairs(&args.env, "--env")?);
            }
            Ok(server)
        }
        (Some(_), [_, ..]) => Err(CliError::usage(
            "give either --url or a command after --, not both",
        )),
        (None, []) => Err(CliError::usage(
            "give --url <url> for a remote server or `-- <command> [args...]` for a local one",
        )),
    }
}

fn pairs(items: &[String], flag: &str) -> Result<Map<String, Value>, CliError> {
    items
        .iter()
        .map(|item| {
            item.split_once('=')
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_owned(), Value::from(v)))
                .ok_or_else(|| CliError::usage(format!("{flag} expects K=V, got \"{item}\"")))
        })
        .collect()
}

/// The document a scope edits; an existing `.json` is kept when there is no `.jsonc`.
pub(crate) fn document(ctx: &Context, scope: Scope) -> Result<PathBuf, CliError> {
    let (directory, stem) = match scope {
        Scope::User => (ctx.paths.config.clone(), "cyber"),
        Scope::Project | Scope::Local => {
            let root = cyber_core::config::project_root(&ctx.location);
            let stem = if scope == Scope::Local {
                "cyber.local"
            } else {
                "cyber"
            };
            (root.join(".cyber"), stem)
        }
    };
    let jsonc = directory.join(format!("{stem}.jsonc"));
    let json = directory.join(format!("{stem}.json"));
    Ok(
        if !jsonc.exists() && json.exists() && scope != Scope::Local {
            json
        } else {
            jsonc
        },
    )
}

fn defined_in(path: &Path, name: &str) -> Result<bool, CliError> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let value = cyber_core::config::parse_jsonc(&path.display().to_string(), &text)
        .map_err(|e| CliError::runtime(e.to_string()))?;
    Ok(value["mcp"].get(name).is_some())
}

/// Server-provided names and errors on one line without control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}
