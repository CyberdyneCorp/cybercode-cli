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
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--job")) {
        return windows_job::run();
    }

    #[cfg(unix)]
    return proxy_bridge::run();

    #[cfg(not(unix))]
    {
        eprintln!("cyber-sandbox-exec: the Unix proxy bridge is unavailable on this platform");
        ExitCode::from(69)
    }
}
