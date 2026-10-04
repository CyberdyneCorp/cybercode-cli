//! OS sandbox for model-run commands (`sandbox`): policy resolution, credential masking,
//! the network allowlist proxy and per-platform enforcement.

mod env;
#[cfg(target_os = "linux")]
mod linux;
mod policy;
pub mod proxy;
#[cfg(target_os = "macos")]
mod seatbelt;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use env::mask_env;
pub use policy::{
    DEFAULT_DOMAINS, NetworkMode, Policy, SandboxConfig, matches_domain, protected_in,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    #[error("SandboxUnavailableError: {0}")]
    Unavailable(String),
}

/// Everything needed to launch one sandboxed command.
#[derive(Debug, Clone)]
pub struct Launch {
    pub config: SandboxConfig,
    /// Canonical writable roots.
    pub writable: Vec<PathBuf>,
    /// Paths inside writable roots that stay read-only.
    pub read_only: Vec<PathBuf>,
    /// Paths that must not be read.
    pub unreadable: Vec<PathBuf>,
    /// The allowlist proxy when the network mode is `proxy`.
    pub proxy: Option<proxy::Endpoint>,
    /// The `cyber-sandbox-exec` helper (Linux), which bridges the proxy into the sandbox.
    pub helper: Option<PathBuf>,
}

/// The proxy port sandboxed processes use inside a private network namespace.
pub const INNER_PROXY_PORT: u16 = 3128;

/// A command line to spawn instead of the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub program: String,
    pub args: Vec<String>,
    /// Variables to add on top of the masked environment.
    pub env: BTreeMap<String, String>,
}

/// Wrap `program args` for the current platform. Full access runs the command as is.
pub fn wrap(launch: &Launch, program: &str, args: &[String]) -> Result<Wrapped, SandboxError> {
    let mut env = BTreeMap::new();
    let port = match &launch.proxy {
        Some(proxy::Endpoint::Tcp(port)) => Some(*port),
        Some(proxy::Endpoint::Unix(_)) => Some(INNER_PROXY_PORT),
        None => None,
    };
    if let Some(port) = port {
        let url = format!("http://127.0.0.1:{port}");
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            env.insert(name.to_string(), url.clone());
        }
        env.insert("NO_PROXY".into(), String::new());
        env.insert("no_proxy".into(), String::new());
    }
    if launch.config.policy == Policy::FullAccess {
        return Ok(Wrapped {
            program: program.into(),
            args: args.to_vec(),
            env,
        });
    }
    platform_wrap(launch, program, args, env)
}

#[cfg(target_os = "macos")]
fn platform_wrap(
    launch: &Launch,
    program: &str,
    args: &[String],
    env: BTreeMap<String, String>,
) -> Result<Wrapped, SandboxError> {
    let profile = seatbelt::profile(launch);
    let mut full = vec![
        "-p".to_string(),
        profile,
        "--".to_string(),
        program.to_string(),
    ];
    full.extend_from_slice(args);
    Ok(Wrapped {
        program: seatbelt::SANDBOX_EXEC.into(),
        args: full,
        env,
    })
}

#[cfg(target_os = "linux")]
fn platform_wrap(
    launch: &Launch,
    program: &str,
    args: &[String],
    env: BTreeMap<String, String>,
) -> Result<Wrapped, SandboxError> {
    linux::wrap(launch, program, args, env)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_wrap(
    _launch: &Launch,
    _program: &str,
    _args: &[String],
    _env: BTreeMap<String, String>,
) -> Result<Wrapped, SandboxError> {
    Err(SandboxError::Unavailable(format!(
        "no sandbox enforcement is available on {}; run with --sandbox full-access to opt out explicitly",
        std::env::consts::OS
    )))
}

/// Whether OS enforcement exists on this machine, for `cyber doctor` and session headers.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    return std::path::Path::new(seatbelt::SANDBOX_EXEC).exists();
    #[cfg(target_os = "linux")]
    return linux::bwrap().is_some();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return false;
}

/// Whether the proxy must listen on a Unix socket bridged into the sandbox.
pub fn proxy_uses_unix_socket() -> bool {
    cfg!(target_os = "linux")
}

/// Locate `cyber-sandbox-exec`: `CYBER_SANDBOX_HELPER`, next to the running executable (or
/// its parent directory, for test binaries under `target/*/deps`), then `PATH`.
pub fn find_helper() -> Option<PathBuf> {
    const NAME: &str = "cyber-sandbox-exec";
    if let Some(path) = std::env::var_os("CYBER_SANDBOX_HELPER") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let near = exe.ancestors().skip(1).take(2).map(|d| d.join(NAME));
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(NAME))
            .collect::<Vec<_>>()
    });
    near.chain(on_path).find(|p| p.is_file())
}
