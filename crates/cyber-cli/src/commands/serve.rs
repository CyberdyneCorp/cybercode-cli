//! `cyber serve` and `cyber service` (`server-api` → One server per user, Background
//! service management, Local password authentication).

use clap::{Args, Subcommand};
use cyber_app::{App, AppOptions, ServeOptions};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Address to bind (default 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    pub hostname: String,
    /// TCP port (default 4747, falling back to a free port).
    #[arg(long)]
    pub port: Option<u16>,
    /// Unix socket path (default <state>/cyber.sock).
    #[arg(long)]
    pub socket: Option<std::path::PathBuf>,
    /// Do not listen on TCP.
    #[arg(long)]
    pub no_tcp: bool,
    /// Register in server.json so clients find this server.
    #[arg(long)]
    pub register: bool,
    /// Sandbox policy for model-run commands: read-only, workspace-write or full-access.
    #[arg(long)]
    pub sandbox: Option<String>,
    /// Publish the peer listener (`peers.listen`) over mDNS, as `peers.mdns` does. Only a
    /// listener off loopback is published; the main API never is.
    #[arg(long)]
    pub mdns: bool,
    /// Speak JSON-RPC over stdin and stdout.
    #[arg(long)]
    pub stdio: bool,
    /// Allow `bypass` mode without a sandbox or a container. Everything the model runs
    /// executes without approval or OS confinement.
    #[arg(long)]
    pub dangerously_bypass_permissions: bool,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCmd {
    /// Start the background server, or reuse a healthy one.
    Start,
    /// Stop the background server.
    Stop,
    /// Stop, then start.
    Restart,
    /// Show the registered server and whether it is healthy.
    Status,
    /// Print the local server password, or replace it (restarting the server).
    Password { value: Option<String> },
}

pub fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

pub fn serve(args: ServeArgs, ctx: &Context) -> Result<(), CliError> {
    if args.stdio {
        return stdio(args, ctx);
    }
    let password = cyber_app::password(&ctx.paths).map_err(CliError::runtime)?;
    runtime()?.block_on(async {
        let app = App::build(AppOptions {
            paths: ctx.paths.clone(),
            home: ctx.home.clone(),
            database: ctx.database(),
            default_directory: ctx.location.clone(),
            sandbox_policy: args.sandbox.clone(),
            snapshots: true,
            interactive: true,
            password: Some(password),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: crate::commands::exec::setup::isolation(args.dangerously_bypass_permissions),
        })
        .await
        .map_err(CliError::runtime)?;
        let opts = ServeOptions {
            hostname: args.hostname,
            port: args.port,
            socket: args.socket,
            no_tcp: args.no_tcp,
            register: args.register,
            mdns: args.mdns,
            ..ServeOptions::default()
        };
        cyber_app::run_server(app, opts, |url| println!("cyber server listening on {url}"))
            .await
            .map_err(CliError::runtime)
    })
}

/// An embedded private server on `CYBER_DB` (or in memory), speaking JSON-RPC on stdio.
fn stdio(args: ServeArgs, ctx: &Context) -> Result<(), CliError> {
    let database = match std::env::var("CYBER_DB") {
        Ok(_) => ctx.database(),
        Err(_) => cyber_core::paths::DatabaseLocation::Memory,
    };
    runtime()?.block_on(async {
        let app = App::build(AppOptions {
            paths: ctx.paths.clone(),
            home: ctx.home.clone(),
            database,
            default_directory: ctx.location.clone(),
            sandbox_policy: args.sandbox.clone(),
            snapshots: true,
            interactive: true,
            password: None,
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: crate::commands::exec::setup::isolation(args.dangerously_bypass_permissions),
        })
        .await
        .map_err(CliError::runtime)?;
        cyber_app::serve_stdio(app).await.map_err(CliError::runtime)
    })
}

pub fn service(cmd: ServiceCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let rt = runtime()?;
    let version = cyber_app::version();
    match cmd {
        ServiceCmd::Start => {
            let info = rt.block_on(start(ctx))?;
            report(
                global,
                &json!({ "url": info.registration.url, "pid": info.registration.pid, "version": version }),
            )
        }
        ServiceCmd::Stop => {
            let stopped = rt
                .block_on(cyber_app::stop_service(&ctx.paths))
                .map_err(CliError::runtime)?;
            report(global, &json!({ "stopped": stopped.map(|r| r.pid) }))
        }
        ServiceCmd::Restart => {
            rt.block_on(cyber_app::stop_service(&ctx.paths))
                .map_err(CliError::runtime)?;
            let info = rt.block_on(start(ctx))?;
            report(
                global,
                &json!({ "url": info.registration.url, "pid": info.registration.pid, "version": version }),
            )
        }
        ServiceCmd::Status => {
            let reg = cyber_app::read_registration(&ctx.paths);
            let probe = reg.as_ref().and_then(|r| {
                rt.block_on(cyber_app::probe(&r.url))
                    .filter(|p| p.serves(r))
            });
            let healthy = probe.as_ref().is_some_and(|p| p.version == version);
            let stale = match (&reg, &probe) {
                (Some(r), Some(p)) => cyber_app::ClientBuild::current(version).is_stale(
                    r,
                    &p.version,
                    p.build.as_deref(),
                ),
                _ => false,
            };
            report(
                global,
                &json!({ "registered": reg, "healthy": healthy, "stale": stale }),
            )
        }
        ServiceCmd::Password { value } => password(ctx, value, &rt),
    }
}

pub async fn start(ctx: &Context) -> Result<cyber_app::ServerClientInfo, CliError> {
    let exe = std::env::current_exe()?;
    let info = cyber_app::start_service(&ctx.paths, &exe, cyber_app::version())
        .await
        .map_err(CliError::runtime)?;
    if let Some(notice) = info.notice {
        eprintln!("{}", notice.message());
    }
    Ok(info)
}

fn password(
    ctx: &Context,
    value: Option<String>,
    rt: &tokio::runtime::Runtime,
) -> Result<(), CliError> {
    let Some(value) = value else {
        println!(
            "{}",
            cyber_app::password(&ctx.paths).map_err(CliError::runtime)?
        );
        return Ok(());
    };
    if value.len() < 16 {
        return Err(CliError::usage(
            "the password must be at least 16 characters",
        ));
    }
    let restart = rt
        .block_on(cyber_app::replace_password(&ctx.paths, &value))
        .map_err(CliError::runtime)?;
    if restart {
        rt.block_on(start(ctx))?;
    }
    Ok(())
}

fn report(global: &GlobalArgs, value: &serde_json::Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(value);
    }
    match value.as_object() {
        Some(map) => {
            for (k, v) in map {
                println!(
                    "{k}: {}",
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else {
                        v.to_string()
                    }
                );
            }
        }
        None => println!("{value}"),
    }
    Ok(())
}
