//! `cyber peers` (`cli-commands` → Peers command, `cross-session-messaging` → Peer pairing
//! and grants): pair this machine with other `cyber` servers the same person controls.
//!
//! The receiving side (`invite`, `revoke` and the grants of `list`) goes through the local
//! background server, which owns the grants and the running peer listener. The sending side
//! (`add`, `rm`, `test` and the remotes of `list`) edits user config and the token store and
//! calls peers with the same pinned-key client the server sends with. Tokens are printed
//! only by `invite`, once, inside the pairing code.

use clap::{Args, Subcommand, ValueEnum};
use cyber_app::peer_discovery::{self, Discovered};
use cyber_app::peer_tokens::{self, PeerTokenStore, PeerTokens};
use cyber_client::Client;
use cyber_core::config::{PeerRemote, PeersSettings, valid_peer_name};
use cyber_server::runtime::messaging::{PeerClient, PeerError, PeerTarget, RemoteHello};
use cyber_server::runtime::peers::PairingCode;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum PeersCmd {
    /// Let another machine send to this one: create a grant and print its pairing code
    /// once. Needs `peers.listen` in user config.
    #[command(after_help = INVITE_EXAMPLES)]
    Invite(InviteArgs),
    /// Pair with another machine from the code its `cyber peers invite` printed: test it,
    /// store the token in the OS keyring and add the remote to user config.
    #[command(after_help = ADD_EXAMPLES)]
    Add(AddArgs),
    /// Remove a remote and its stored token.
    #[command(alias = "remove")]
    Rm { name: String },
    /// Remotes (state and latency) and grants of this machine.
    #[command(alias = "ls")]
    List {
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Call a remote's `hello`: machine, version, scopes and latency; exit 1 with the
    /// error tag on failure.
    Test { name: String },
    /// End a grant: the peer's next request is refused.
    Revoke { name: String },
    /// Peer listeners published on this network over mDNS: machine, address, fingerprint
    /// and whether each is a remote already. It only lists: pairing still needs a code.
    Discover {
        /// How long to listen: `3s`, `500ms` or `1m`.
        #[arg(long, default_value = "3s")]
        timeout: String,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
}

const INVITE_EXAMPLES: &str = "Examples:
  cyber peers invite laptop
  cyber peers invite laptop --scope message --scope attach --expires 90d
  cyber peers invite laptop --host desk.tailnet.example --rotate-key";

const ADD_EXAMPLES: &str = "Examples:
  cyber peers add desk 'cyberpeer://desk.local:4748/?machine=desk&fp=sha256:…&t=cyp_…'
  DESK_PEER_TOKEN=cyp_… cyber peers add desk '<code>' --token-env DESK_PEER_TOKEN";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ScopeArg {
    Message,
    Attach,
}

#[derive(Debug, Args)]
pub struct InviteArgs {
    /// The other machine's name here: its senders appear as `<session>@<name>`.
    pub name: String,
    /// What the grant allows (repeatable); default `message`.
    #[arg(long = "scope", value_enum)]
    pub scopes: Vec<ScopeArg>,
    /// End the grant after this long: `90d`, `12h` or `30m`.
    #[arg(long)]
    pub expires: Option<String>,
    /// Make a new listener key first; every existing pairing must be redone.
    #[arg(long)]
    pub rotate_key: bool,
    /// The host the code names (default: the bound IP, or this host's LAN name).
    #[arg(long)]
    pub host: Option<String>,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The name to send to it by (`<session>@<name>`).
    pub name: String,
    /// The pairing code `cyber peers invite` printed on the other machine.
    pub code: String,
    /// Keep the token out of the keyring: config gets `{env:NAME}`, and `NAME` must hold
    /// the code's token wherever the server runs (headless machines).
    #[arg(long, value_name = "NAME")]
    pub token_env: Option<String>,
}

pub fn run(cmd: PeersCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let rt = super::serve::runtime()?;
    match cmd {
        PeersCmd::Invite(args) => rt.block_on(invite(&args, ctx, global)),
        PeersCmd::Revoke { name } => rt.block_on(revoke(&name, ctx, global)),
        PeersCmd::Add(args) => rt.block_on(add(&args, ctx, global)),
        PeersCmd::Rm { name } => remove(&name, ctx, global),
        PeersCmd::Test { name } => rt.block_on(test(&name, ctx, global)),
        PeersCmd::List { json } => rt.block_on(list(ctx, global, json)),
        PeersCmd::Discover { timeout, json } => discover(&timeout, json, ctx, global),
    }
}

/// A client of the local background server, started when needed.
async fn local(ctx: &Context) -> Result<Client, CliError> {
    let info = super::serve::start(ctx).await?;
    Ok(Client::http(&info.registration.url, Some(info.password)))
}

/// One local request; a failure keeps the server's tag and message.
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
    Err(if status == 400 {
        CliError::usage(error)
    } else {
        CliError::runtime(error)
    })
}

async fn invite(args: &InviteArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if !valid_peer_name(&args.name) {
        return Err(invalid_name(&args.name));
    }
    let expires_ms = args
        .expires
        .as_deref()
        .map(|text| expiry_ms(text, now_ms()))
        .transpose()?;
    let scopes: Vec<&str> = args
        .scopes
        .iter()
        .map(|scope| match scope {
            ScopeArg::Message => "message",
            ScopeArg::Attach => "attach",
        })
        .collect();
    let body = json!({ "name": args.name, "scopes": scopes, "expires_ms": expires_ms,
        "rotate_key": args.rotate_key, "host": args.host });
    let client = local(ctx).await?;
    let invitation = call(&client, "POST", "/peers/grants", Some(body)).await?;
    if output::is_json(global.format) {
        return output::json(&invitation);
    }
    let machine = text(&invitation["listener"]["machine"]);
    if args.rotate_key {
        println!(
            "new listener key {}: every existing pairing must be redone",
            text(&invitation["listener"]["fingerprint"])
        );
    }
    println!(
        "pairing code for {} (shown once; it holds the token):\n\n  {}\n",
        args.name,
        text(&invitation["code"])
    );
    println!("on {}: cyber peers add {machine} '<code>'", args.name);
    if args.host.is_none()
        && let Some(host) = lan_host_differing(&text(&invitation["code"]), &machine)
    {
        println!(
            "the code names {host}, this host's LAN name (peers.machine is {machine}); if {} \
             cannot reach it, run `cyber peers invite {} --host <IP or name>` instead",
            args.name, args.name
        );
    }
    Ok(())
}

/// The host a pairing code names when it is a LAN name (`….local`) other than
/// `<machine>.local`: `peers.machine` differs from the host name, so the other machine
/// cannot guess the name from the peer's.
fn lan_host_differing(code: &str, machine: &str) -> Option<String> {
    let authority = code.strip_prefix("cyberpeer://")?.split('/').next()?;
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    let lan = host.to_ascii_lowercase().ends_with(".local");
    let named = host.eq_ignore_ascii_case(&format!("{machine}.local"));
    (lan && !named).then(|| host.to_string())
}

async fn revoke(name: &str, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let client = local(ctx).await?;
    let path = format!("/peers/grants/{}", super::messages::query(name));
    let result = call(&client, "DELETE", &path, None).await?;
    if output::is_json(global.format) {
        output::json(&result)?;
    } else if result["revoked"] == true {
        println!("revoked {name}: its next request is refused");
    }
    if result["revoked"] != true {
        return Err(
            CliError::runtime(format!("no active grant is named {name}"))
                .with_hint("cyber peers list shows the grants"),
        );
    }
    Ok(())
}

async fn add(args: &AddArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if !valid_peer_name(&args.name) {
        return Err(invalid_name(&args.name));
    }
    let code = PairingCode::parse(&args.code).map_err(CliError::usage)?;
    check_discovered(&code, ctx)?;
    let mut entry =
        json!({ "name": args.name, "url": code.url(), "fingerprint": code.fingerprint });
    if let Some(name) = &args.token_env {
        check_token_env(name, &code.token)?;
        entry["token"] = json!(format!("{{env:{name}}}"));
    }
    let path = user_document(ctx);
    let mut remotes = raw_remotes(&path)?;
    if remotes
        .iter()
        .any(|remote| remote["name"] == args.name.as_str())
    {
        return Err(
            CliError::usage(format!("a remote named {} exists", args.name))
                .with_hint(format!("remove it first with cyber peers rm {}", args.name)),
        );
    }
    remotes.push(entry);
    // The same rules as loading: plain http only to loopback, a well-formed pin.
    PeersSettings::from_config(&json!({ "peers": { "remotes": remotes } }))
        .map_err(CliError::usage)?;
    let target = PeerTarget {
        name: args.name.clone(),
        url: code.url(),
        fingerprint: code.fingerprint.clone(),
        token: code.token.clone(),
    };
    // Test first: a code that does not work stores nothing.
    let (hello, latency) = hello(target)
        .await
        .map_err(|error| unpaired(error, &args.name))?;
    if args.token_env.is_none() {
        let stored = PeerTokenStore::open(&ctx.paths)
            .put(&args.name, &code.token)
            .map_err(CliError::runtime)?;
        if let Some(warning) = stored.warning() {
            eprintln!("warning: {warning}");
        }
    }
    if let Err(error) = write_remotes(&path, remotes) {
        let _ = PeerTokenStore::open(&ctx.paths).delete(&args.name);
        return Err(error);
    }
    if output::is_json(global.format) {
        return output::json(&json!({ "name": args.name, "url": code.url(),
            "fingerprint": code.fingerprint, "file": path, "hello": hello_json(&hello, latency) }));
    }
    println!(
        "added {} ({}, {}): machine {}, version {}, scopes {}, {} ms",
        args.name,
        code.url(),
        short(&code.fingerprint),
        clean(&hello.machine),
        clean(&hello.version),
        hello.scopes.join(","),
        latency.as_millis()
    );
    println!(
        "{0} names this machine `{1}`; for {0} to reply, run `cyber peers invite {0}` here and \
         `cyber peers add {1} '<code>'` on {0}",
        args.name,
        clean(&hello.grant)
    );
    Ok(())
}

/// A code whose pin differs from what the last discovery found at its address is refused
/// before anything is stored or called.
fn check_discovered(code: &PairingCode, ctx: &Context) -> Result<(), CliError> {
    let found = peer_discovery::recalled(&ctx.paths, now_ms());
    let Some(entry) = peer_discovery::conflict(&found, &code.authority, &code.fingerprint) else {
        return Ok(());
    };
    Err(CliError::runtime(format!(
        "the pairing code pins {} but {} published {} at {}",
        code.fingerprint,
        clean(&entry.machine),
        entry.fingerprint,
        code.authority
    ))
    .with_hint(
        "compare the fingerprint `cyber peers invite` printed on the other machine; after a \
         key rotation there, run cyber peers discover again",
    ))
}

/// `cyber peers discover`: browse, keep what answered for `add`, and list it.
fn discover(timeout: &str, json: bool, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let timeout = super::exec::parse_duration(timeout)?;
    let settings = settings(ctx)?;
    let found = peer_discovery::browse(timeout).map_err(CliError::runtime)?;
    if let Err(error) = peer_discovery::remember(&ctx.paths, &found, now_ms()) {
        eprintln!("warning: cannot keep the discovery for cyber peers add: {error}");
    }
    let entries: Vec<Value> = found
        .iter()
        .map(|peer| discovered_entry(peer, &settings.remotes))
        .collect();
    if json || output::is_json(global.format) {
        return output::json(&json!({ "peers": entries }));
    }
    println!("discovered:");
    if entries.is_empty() {
        println!(
            "  none within {} ms (the other machine needs peers.mdns: true)",
            timeout.as_millis()
        );
    }
    for entry in &entries {
        let addresses: Vec<String> = entry["addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        let state = match entry["remote"].as_str() {
            Some(name) => format!("added as {name}"),
            None => "not added".into(),
        };
        println!(
            "  {:<16} {:<32} {} {}",
            text(&entry["machine"]),
            addresses.join(","),
            text(&entry["fingerprint"]),
            state
        );
    }
    Ok(())
}

/// One discovered listener, with the remote that pins its key, if any.
fn discovered_entry(peer: &Discovered, remotes: &[PeerRemote]) -> Value {
    let remote = remotes
        .iter()
        .find(|remote| remote.fingerprint == peer.fingerprint)
        .map(|remote| remote.name.as_str());
    let addresses: Vec<String> = peer
        .addresses
        .iter()
        .map(|ip| std::net::SocketAddr::new(*ip, peer.port).to_string())
        .collect();
    json!({ "machine": clean(&peer.machine), "host": clean(&peer.host), "port": peer.port,
        "addresses": addresses, "fingerprint": peer.fingerprint, "remote": remote })
}

/// `--token-env NAME`: `NAME` must already hold the code's token, so the server finds it.
fn check_token_env(name: &str, token: &str) -> Result<(), CliError> {
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(CliError::usage(format!(
            "--token-env {name:?}: expected an environment variable name"
        )));
    }
    if std::env::var(name).ok().as_deref() != Some(token) {
        return Err(CliError::usage(format!(
            "{name} does not hold the token of this pairing code"
        ))
        .with_hint(format!(
            "set {name} to the value after `t=` in the code, where the server runs too"
        )));
    }
    Ok(())
}

fn remove(name: &str, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let path = user_document(ctx);
    let mut remotes = raw_remotes(&path)?;
    let Some(index) = remotes.iter().position(|remote| remote["name"] == name) else {
        return Err(CliError::usage(format!("no remote is named {name}"))
            .with_hint("cyber peers list shows the remotes"));
    };
    let removed = remotes.remove(index);
    // A token given in config (`{env:NAME}`) was never stored. The stored one goes
    // first, so a failed deletion keeps the remote listed rather than orphaning it.
    let stored = removed.get("token").is_none();
    if stored {
        PeerTokenStore::open(&ctx.paths)
            .delete(name)
            .map_err(CliError::runtime)?;
    }
    write_remotes(&path, remotes)?;
    if output::is_json(global.format) {
        return output::json(&json!({ "name": name, "removed": true, "file": path }));
    }
    if stored {
        println!("removed {name} and its stored token");
    } else {
        println!("removed {name}");
    }
    Ok(())
}

async fn test(name: &str, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let settings = settings(ctx)?;
    let remote = settings
        .remotes
        .iter()
        .find(|remote| remote.name == name)
        .ok_or_else(|| peer_error(&PeerError::NotConfigured { peer: name.into() }))?;
    let (hello, latency) = hello(target(remote, ctx)?).await?;
    if output::is_json(global.format) {
        return output::json(&json!({ "name": name, "hello": hello_json(&hello, latency) }));
    }
    output::rows(&[
        ("machine", clean(&hello.machine)),
        ("version", clean(&hello.version)),
        ("scopes", hello.scopes.join(",")),
        ("latency", format!("{} ms", latency.as_millis())),
        ("named here", clean(&hello.grant)),
    ]);
    Ok(())
}

async fn list(ctx: &Context, global: &GlobalArgs, json: bool) -> Result<(), CliError> {
    let settings = settings(ctx)?;
    let probes = settings.remotes.iter().map(|remote| probe(remote, ctx));
    let remotes: Vec<Value> = futures::future::join_all(probes).await;
    let client = local(ctx).await?;
    let overview = call(&client, "GET", "/peers", None).await?;
    let machine = overview["listener"]["machine"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| cyber_server::http::peer::machine_name(settings.machine.as_deref()));
    let grants: Vec<Value> = overview["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|grant| grant_entry(grant, &settings.remotes, &machine))
        .collect();
    if json || output::is_json(global.format) {
        return output::json(
            &json!({ "machine": machine, "listener": overview["listener"],
            "remotes": remotes, "grants": grants }),
        );
    }
    print_remotes(&remotes);
    print_grants(&grants);
    Ok(())
}

/// A remote with its live state: `online`, `offline`, `auth_failed`, `no_token` or
/// `disabled`.
async fn probe(remote: &PeerRemote, ctx: &Context) -> Value {
    let mut entry = json!({ "name": remote.name, "url": remote.url.as_str(),
        "fingerprint": remote.fingerprint, "enabled": remote.enabled });
    if !remote.enabled {
        entry["state"] = json!("disabled");
        return entry;
    }
    let target = match target(remote, ctx) {
        Ok(target) => target,
        Err(_) => {
            entry["state"] = json!("no_token");
            return entry;
        }
    };
    match hello(target).await {
        Ok((hello, latency)) => {
            entry["state"] = json!("online");
            entry["hello"] = hello_json(&hello, latency);
        }
        Err(error) => {
            let auth = error.message.starts_with("PeerAuthError");
            entry["state"] = json!(if auth { "auth_failed" } else { "offline" });
            entry["error"] = json!(error.message);
        }
    }
    entry
}

/// A grant, flagged when no remote of the same name lets this machine reply.
fn grant_entry(grant: &Value, remotes: &[PeerRemote], machine: &str) -> Value {
    let mut entry = grant.clone();
    let name = grant["name"].as_str().unwrap_or_default();
    let can_reply = remotes.iter().any(|remote| remote.name == name);
    entry["can_reply"] = json!(can_reply);
    if grant["active"] == true && !can_reply {
        entry["reply_hint"] = json!(format!(
            "on {name}: cyber peers invite {machine}; then here: cyber peers add {name} '<code>'"
        ));
    }
    entry
}

fn print_remotes(remotes: &[Value]) {
    println!("remotes:");
    if remotes.is_empty() {
        println!("  none (pair one with cyber peers add <name> '<code>')");
    }
    for remote in remotes {
        let latency = remote["hello"]["latency_ms"]
            .as_u64()
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_default();
        println!(
            "  {:<16} {:<32} {:<18} {:<11} {}",
            text(&remote["name"]),
            text(&remote["url"]),
            short(remote["fingerprint"].as_str().unwrap_or_default()),
            text(&remote["state"]),
            latency
        );
    }
}

fn print_grants(grants: &[Value]) {
    println!("grants:");
    if grants.is_empty() {
        println!("  none (let a machine send here with cyber peers invite <name>)");
    }
    for grant in grants {
        let scopes: Vec<String> = grant["scopes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        let state = match (grant["revoked_ms"].is_null(), grant["active"] == true) {
            (false, _) => "revoked",
            (true, false) => "expired",
            (true, true) => "active",
        };
        println!(
            "  {:<16} {:<15} {:<8} created {}  last used {}  expires {}",
            text(&grant["name"]),
            scopes.join(","),
            state,
            time(&grant["created_ms"]),
            time(&grant["last_used_ms"]),
            time(&grant["expires_ms"]),
        );
        if let Some(hint) = grant["reply_hint"].as_str() {
            println!("    can receive, cannot reply: {hint}");
        }
    }
}

/// `peers` of user config, as the server reads it.
pub(crate) fn settings(ctx: &Context) -> Result<PeersSettings, CliError> {
    PeersSettings::from_config(&ctx.global_config()?).map_err(CliError::usage)
}

pub(crate) fn target(remote: &PeerRemote, ctx: &Context) -> Result<PeerTarget, CliError> {
    let token = peer_tokens::resolve(remote, &PeerTokenStore::open(&ctx.paths))
        .map_err(CliError::runtime)?;
    Ok(PeerTarget {
        name: remote.name.clone(),
        url: remote.url.as_str().trim_end_matches('/').to_string(),
        fingerprint: remote.fingerprint.clone(),
        token,
    })
}

async fn hello(target: PeerTarget) -> Result<(RemoteHello, std::time::Duration), CliError> {
    let client = PeerClient::new(target).map_err(|error| peer_error(&error))?;
    client.hello().await.map_err(|error| peer_error(&error))
}

/// A refused pairing code: nothing was stored, so the hint is a new code, not `rm`.
fn unpaired(error: CliError, name: &str) -> CliError {
    if !error.message.starts_with("PeerAuthError") {
        return error;
    }
    error.with_hint(format!(
        "nothing was stored; ask for a new code: on {name}, cyber peers invite <this machine>"
    ))
}

/// A peer failure exits 1 with its tag first.
pub(crate) fn peer_error(error: &PeerError) -> CliError {
    let hint = match error {
        PeerError::Auth { peer, .. } => Some(format!(
            "pair it again: cyber peers rm {peer}, then cyber peers add {peer} '<new code>'"
        )),
        PeerError::NotConfigured { .. } => Some("cyber peers list shows the remotes".into()),
        _ => None,
    };
    let cli = CliError::runtime(error.to_string());
    match hint {
        Some(hint) => cli.with_hint(hint),
        None => cli,
    }
}

fn hello_json(hello: &RemoteHello, latency: std::time::Duration) -> Value {
    json!({ "machine": hello.machine, "version": hello.version, "scopes": hello.scopes,
        "grant": hello.grant, "fingerprint": hello.fingerprint,
        "latency_ms": latency.as_millis() as u64 })
}

/// The user config document `add` and `rm` edit.
fn user_document(ctx: &Context) -> std::path::PathBuf {
    super::mcp_config::document(ctx, super::mcp_config::Scope::User)
        .unwrap_or_else(|_| ctx.paths.config.join("cyber.jsonc"))
}

/// `peers.remotes` as written (placeholders unexpanded).
fn raw_remotes(path: &std::path::Path) -> Result<Vec<Value>, CliError> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let document = cyber_core::config::parse_jsonc(&path.display().to_string(), &text)
        .map_err(|e| CliError::runtime(e.to_string()))?;
    match &document["peers"]["remotes"] {
        Value::Null => Ok(Vec::new()),
        Value::Array(remotes) => Ok(remotes.clone()),
        _ => Err(CliError::usage(format!(
            "peers.remotes in {} is not a list",
            path.display()
        ))),
    }
}

fn write_remotes(path: &std::path::Path, remotes: Vec<Value>) -> Result<(), CliError> {
    cyber_core::config::set_document_member(path, "peers", "remotes", Some(&Value::Array(remotes)))
        .map(|_| ())
        .map_err(CliError::runtime)
}

fn invalid_name(name: &str) -> CliError {
    CliError::usage(format!(
        "invalid peer name {name:?}: use letters, digits, '.', '_' or '-' (at most 64)"
    ))
}

/// `90d`, `12h` or `30m` from `now_ms`.
fn expiry_ms(text: &str, now_ms: i64) -> Result<i64, CliError> {
    let invalid = || {
        CliError::usage(format!(
            "invalid --expires {text:?}; use e.g. 90d, 12h or 30m"
        ))
    };
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let count: i64 = number.parse().map_err(|_| invalid())?;
    let unit_ms = match unit {
        "d" => 86_400_000,
        "h" => 3_600_000,
        "m" => 60_000,
        _ => return Err(invalid()),
    };
    count
        .checked_mul(unit_ms)
        .filter(|ms| *ms > 0)
        .and_then(|ms| now_ms.checked_add(ms))
        .ok_or_else(invalid)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A Unix-milliseconds time as UTC, `-` when absent.
fn time(value: &Value) -> String {
    value
        .as_i64()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

/// `sha256:` and the first 12 hex digits.
fn short(fingerprint: &str) -> String {
    let end = fingerprint.len().min("sha256:".len() + 12);
    format!("{}…", &fingerprint[..end])
}

fn text(value: &Value) -> String {
    clean(value.as_str().unwrap_or_default())
}

/// Text from another machine; keep it on one line without control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lan_host_other_than_the_machine_name_is_pointed_out() {
        let code = |host: &str| format!("cyberpeer://{host}:4748/?machine=desk&fp=x&t=y");
        assert_eq!(
            lan_host_differing(&code("leos-mini.local"), "desk").as_deref(),
            Some("leos-mini.local")
        );
        assert_eq!(lan_host_differing(&code("Desk.local"), "desk"), None);
        assert_eq!(lan_host_differing(&code("192.168.1.20"), "desk"), None);
        assert_eq!(
            lan_host_differing(&code("desk.tailnet.example"), "desk"),
            None
        );
    }

    #[test]
    fn expiries_are_days_hours_or_minutes_ahead() {
        assert_eq!(expiry_ms("90d", 1_000).unwrap(), 1_000 + 90 * 86_400_000);
        assert_eq!(expiry_ms("12h", 0).unwrap(), 12 * 3_600_000);
        assert_eq!(expiry_ms("30m", 0).unwrap(), 30 * 60_000);
        for bad in ["", "d", "0d", "-1d", "5w", "1.5h", "99999999999999999d"] {
            assert_eq!(expiry_ms(bad, 0).unwrap_err().code, 2, "{bad}");
        }
    }

    #[test]
    fn a_grant_without_a_remote_cannot_reply() {
        let remote = PeerRemote {
            name: "desk".into(),
            url: url::Url::parse("https://desk.local:4748").unwrap(),
            fingerprint: String::new(),
            enabled: true,
            token: None,
        };
        let entry = grant_entry(
            &json!({"name": "laptop", "active": true}),
            std::slice::from_ref(&remote),
            "desk",
        );
        assert_eq!(entry["can_reply"], false);
        assert_eq!(
            entry["reply_hint"],
            "on laptop: cyber peers invite desk; then here: cyber peers add laptop '<code>'"
        );
        let entry = grant_entry(&json!({"name": "desk", "active": true}), &[remote], "x");
        assert_eq!(entry["can_reply"], true);
        assert!(entry["reply_hint"].is_null());
        assert_eq!(
            short(&format!("sha256:{}", "a".repeat(64))),
            "sha256:aaaaaaaaaaaa…"
        );
    }
}
