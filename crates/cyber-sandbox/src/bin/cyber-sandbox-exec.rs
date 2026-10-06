//! Unix proxy bridge and Windows process-tree owner, selected explicitly.

use std::process::ExitCode;

#[cfg(unix)]
#[path = "../proxy_bridge.rs"]
mod proxy_bridge;

#[cfg(windows)]
#[path = "../windows_job.rs"]
mod windows_job;

fn main() -> ExitCode {
    #[cfg(windows)]
    if let Some(mode) = std::env::args_os().nth(1)
        && (mode == "--job" || mode == "--parent-job")
    {
        return windows_job::run(mode == "--parent-job");
    }

    #[cfg(unix)]
    return proxy_bridge::run();

    #[cfg(not(unix))]
    {
        eprintln!("cyber-sandbox-exec: the Unix proxy bridge is unavailable on this platform");
        ExitCode::from(69)
    }
}
