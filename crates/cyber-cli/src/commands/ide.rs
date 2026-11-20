//! `cyber ide token` (`cli-commands` → Resource command groups, `server-api` → Local IDE
//! token issuance): issue, list and revoke the scoped local credential an editor client
//! uses instead of the owner password.
//!
//! `create` writes the token to `<state>/ide/<client>.token` (0600, in a 0700 directory)
//! and prints only its ID and expiry, so the value never reaches scrollback or logs.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use cyber_client::Client;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum IdeCmd {
    /// Local IDE tokens: scoped, revocable credentials for editor clients.
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum TokenCmd {
    /// Issue a token for an editor client and write it to `<state>/ide/<client>.token`;
    /// the client's previous token is revoked. Prints only the token's ID and expiry.
    #[command(after_help = CREATE_EXAMPLES)]
    Create(CreateArgs),
    /// Token records: ID, client, name, creation, expiry, last use and revocation.
    #[command(alias = "ls")]
    List,
    /// Revoke a token: its next request is refused and its event streams close.
    Revoke {
        /// The token's `itk_…` ID, from `cyber ide token list`.
        id: String,
    },
}

const CREATE_EXAMPLES: &str = "Examples:
  cyber ide token create --client vscode
  cyber ide token create --client vscode --name laptop --expires 90";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ClientArg {
    Vscode,
}

impl ClientArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Vscode => "vscode",
        }
    }
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// The editor client the token is for.
    #[arg(long, value_enum)]
    pub client: ClientArg,
    /// A label shown by `cyber ide token list`.
    #[arg(long)]
    pub name: Option<String>,
    /// Days until the token expires: 1 to 90 (default 30).
    #[arg(long, value_name = "DAYS", value_parser = clap::value_parser!(u32).range(1..=90))]
    pub expires: Option<u32>,
}

pub fn run(cmd: IdeCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let IdeCmd::Token { cmd } = cmd;
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let client = local(ctx).await?;
        match cmd {
            TokenCmd::Create(args) => create(&client, &args, ctx, global).await,
            TokenCmd::List => list(&client, global).await,
            TokenCmd::Revoke { id } => revoke(&client, &id, ctx, global).await,
        }
    })
}

/// The token file of `client` under the state directory.
pub fn token_path(state: &Path, client: &str) -> PathBuf {
    state.join("ide").join(format!("{client}.token"))
}

/// A client of the local background server with owner credentials, started when needed.
async fn local(ctx: &Context) -> Result<Client, CliError> {
    let info = super::serve::start(ctx).await?;
    Ok(Client::http(&info.registration.url, Some(info.password)))
}

async fn call(
    client: &Client,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, CliError> {
    let (status, value) = client
        .raw(method, path, body, &[])
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    if (200..300).contains(&status) {
        return Ok(value["data"].clone());
    }
    let message = value["message"].as_str().unwrap_or("the server refused");
    let error = format!("{}: {message}", value["_tag"].as_str().unwrap_or("Error"));
    if status == 400 || status == 404 {
        return Err(CliError::usage(error));
    }
    Err(CliError::runtime(error))
}

async fn create(
    client: &Client,
    args: &CreateArgs,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let name = args.client.as_str();
    let mut body = json!({ "client": name });
    if let Some(label) = &args.name {
        body["name"] = json!(label);
    }
    if let Some(days) = args.expires {
        body["expires_in_days"] = json!(days);
    }
    let issued = call(client, "POST", "/ide/tokens", Some(body)).await?;
    let token = issued["token"]
        .as_str()
        .ok_or_else(|| CliError::runtime("the server returned no token"))?;
    let path = token_path(&ctx.paths.state, name);
    let expires = issued["expires_at"].as_str().unwrap_or_default();
    write_token(&path, token, expires)?;
    let summary = json!({
        "id": issued["id"],
        "client": issued["client"],
        "expires_at": issued["expires_at"],
        "path": path.display().to_string(),
    });
    if output::is_json(global.format) {
        return output::json(&summary);
    }
    println!(
        "Created IDE token {} for {name}, expires {}",
        issued["id"].as_str().unwrap_or_default(),
        issued["expires_at"].as_str().unwrap_or_default()
    );
    println!("Written to {}", path.display());
    Ok(())
}

/// Write `token` and its expiry atomically with mode 0600 inside a 0700 directory: the
/// token on the first line, then `expires_at=<RFC 3339>` so a client can warn before it
/// expires.
fn write_token(path: &Path, token: &str, expires_at: &str) -> Result<(), CliError> {
    let dir = path.parent().expect("the token file has a directory");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    cyber_core::trust::write_private_atomic(
        path,
        format!("{token}\nexpires_at={expires_at}\n").as_bytes(),
    )
    .map_err(|e| CliError::runtime(format!("cannot write {}: {e}", path.display())))
}

async fn list(client: &Client, global: &GlobalArgs) -> Result<(), CliError> {
    let records = call(client, "GET", "/ide/tokens", None).await?;
    if output::is_json(global.format) {
        return output::json(&records);
    }
    let records = records.as_array().cloned().unwrap_or_default();
    if records.is_empty() {
        println!("No IDE tokens. Create one with `cyber ide token create --client vscode`.");
        return Ok(());
    }
    for record in records {
        let field = |key: &str| record[key].as_str().unwrap_or("-").to_string();
        println!(
            "{}  {}  {}  {}  expires {}  last used {}",
            field("id"),
            field("client"),
            field("status"),
            field("name"),
            field("expires_at"),
            field("last_used_at"),
        );
    }
    Ok(())
}

async fn revoke(
    client: &Client,
    id: &str,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let records = call(client, "GET", "/ide/tokens", None).await?;
    let records = records.as_array().cloned().unwrap_or_default();
    let revoked = call(client, "DELETE", &format!("/ide/tokens/{id}"), None).await?;
    // The token file always holds the client's newest token (issuing rotates), so it
    // holds the revoked one exactly when that is the client's newest record.
    let client_name = revoked["client"].as_str().unwrap_or_default();
    let newest = records
        .iter()
        .filter(|record| record["client"] == client_name)
        .max_by_key(|record| {
            record["created_at"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
    let removed = newest.is_some_and(|record| record["id"] == id)
        && remove_token_file(&token_path(&ctx.paths.state, client_name))?;
    if output::is_json(global.format) {
        return output::json(&json!({ "token": revoked, "file_removed": removed }));
    }
    println!("Revoked IDE token {id}");
    if removed {
        println!(
            "Removed {}",
            token_path(&ctx.paths.state, client_name).display()
        );
    }
    Ok(())
}

fn remove_token_file(path: &Path) -> Result<bool, CliError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Remove every token file (the owner password was replaced, which revokes them all).
pub fn remove_token_files(state: &Path) -> Result<(), CliError> {
    let Ok(entries) = std::fs::read_dir(state.join("ide")) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "token") {
            remove_token_file(&path)?;
        }
    }
    Ok(())
}
