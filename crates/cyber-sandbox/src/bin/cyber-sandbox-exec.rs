//! Linux proxy-bridge helper; unsupported platforms refuse to launch a command.

use std::process::ExitCode;

#[cfg(unix)]
#[path = "../proxy_bridge.rs"]
mod proxy_bridge;

fn main() -> ExitCode {
    #[cfg(unix)]
    return proxy_bridge::run();

    #[cfg(not(unix))]
    {
        eprintln!("cyber-sandbox-exec: the Unix proxy bridge is unavailable on this platform");
        ExitCode::from(69)
    }
}
