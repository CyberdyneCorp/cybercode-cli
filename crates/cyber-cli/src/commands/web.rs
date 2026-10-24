//! Launch the embedded browser dashboard against the user's existing owner service.
use crate::{context::Context, error::CliError};
use clap::Args;

#[derive(Debug, Args)]
pub struct WebArgs {
    /// Address to bind when starting a server (default 127.0.0.1). Off loopback requires HTTPS.
    #[arg(long)]
    pub hostname: Option<String>,
    /// TCP port when starting a server (default 4747, falling back to a free port).
    #[arg(long)]
    pub port: Option<u16>,
    /// Print the dashboard URL without opening a browser.
    #[arg(long)]
    pub no_open: bool,
}

pub fn run(args: WebArgs, ctx: &Context) -> Result<(), CliError> {
    let global = ctx.owner_server_config()?;
    let files = cyber_app::owner_tls_files(&global).map_err(CliError::runtime)?;
    // Fail before detaching a process, with a useful message instead of a readiness timeout.
    if let Some(hostname) = &args.hostname {
        let addresses = std::net::ToSocketAddrs::to_socket_addrs(&(
            hostname.as_str(),
            args.port.unwrap_or(4747),
        ))?;
        if files.is_none() && addresses.into_iter().any(|a| !a.ip().is_loopback()) {
            return Err(CliError::usage(
                "off-loopback web access requires global server.tls.cert and server.tls.key; use HTTPS or an SSH forward to loopback",
            ));
        }
    }
    if let Some((cert, key)) = &files {
        cyber_server::http::owner_tls(cert, key).map_err(CliError::runtime)?;
    }
    let exe = std::env::current_exe()?;
    let info = super::serve::runtime()?
        .block_on(cyber_app::start_web_service(
            &ctx.paths,
            &exe,
            cyber_app::version(),
            args.hostname.as_deref(),
            args.port,
            files.is_some(),
        ))
        .map_err(CliError::runtime)?;
    let url = format!("{}/", info.registration.url.trim_end_matches('/'));
    println!("{url}");
    if !args.no_open {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer.exe"
        } else {
            "xdg-open"
        };
        if std::process::Command::new(program)
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_err()
        {
            eprintln!(
                "Open the URL above in a browser. Retrieve the owner password with `cyber service password`."
            );
        }
    }
    Ok(())
}
