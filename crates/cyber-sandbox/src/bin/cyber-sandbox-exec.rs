//! Unix proxy bridge and Windows process-tree owner, selected explicitly.

use std::process::ExitCode;

#[cfg(unix)]
#[path = "../proxy_bridge.rs"]
mod proxy_bridge;

#[cfg(windows)]
#[path = "../windows_job.rs"]
mod windows_job;

fn main() -> ExitCode {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--git-inspect")
    {
        return git_inspect();
    }

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

fn git_inspect() -> ExitCode {
    use cyber_core::worktrees::inspection::{MAX_RESPONSE, Request};
    let result = (|| -> Result<Vec<u8>, String> {
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        let [request] = args.as_slice() else {
            return Err("expected one inspection request".into());
        };
        let request = request
            .to_str()
            .filter(|request| request.len() <= 8192)
            .ok_or("invalid inspection request")?;
        let request: Request = serde_json::from_str(request).map_err(|error| error.to_string())?;
        let parent = request
            .overrides
            .parent()
            .ok_or("inspection configuration has no parent")?;
        let isolated = tempfile::tempdir_in(parent).map_err(|error| error.to_string())?;
        isolate_config(isolated.path())?;
        let report =
            cyber_core::worktrees::inspection::read(&request).map_err(|error| error.to_string())?;
        let response = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
        if response.len() > MAX_RESPONSE {
            return Err("inspection response exceeds supported size".into());
        }
        Ok(response)
    })();
    match result {
        Ok(bytes) => {
            use std::io::Write;
            match std::io::stdout().write_all(&bytes) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::from(74),
            }
        }
        Err(error) => {
            eprintln!("cyber-sandbox-exec: {error}");
            ExitCode::from(65)
        }
    }
}

#[allow(unsafe_code)]
fn isolate_config(directory: &std::path::Path) -> Result<(), String> {
    for level in [
        git2::ConfigLevel::System,
        git2::ConfigLevel::Global,
        git2::ConfigLevel::XDG,
        git2::ConfigLevel::ProgramData,
    ] {
        // SAFETY: --git-inspect runs on the sole main thread, before any other
        // libgit2 access or runtime/thread creation. The directory stays alive throughout inspection.
        unsafe { git2::opts::set_search_path(level, directory) }
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
