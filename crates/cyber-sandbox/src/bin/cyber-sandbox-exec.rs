//! Unix proxy bridge, Windows process-tree owner and Windows AppContainer launcher,
//! selected explicitly.

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
        .is_some_and(|arg| arg == "--plugin-copy")
    {
        return plugin_copy();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--git-inspect")
    {
        return git_inspect();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--git-review")
    {
        return git_review();
    }

    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == cyber_sandbox::APPCONTAINER_FLAG)
    {
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        std::process::exit(cyber_sandbox::windows_confine::main(&args));
    }

    #[cfg(windows)]
    if let Some(mode) = std::env::args_os().nth(1)
        && (mode == "--job" || mode == "--parent-job" || mode == "--parent-job-stdin")
    {
        return windows_job::run(mode != "--job", mode == "--parent-job-stdin");
    }

    #[cfg(unix)]
    return proxy_bridge::run();

    #[cfg(not(unix))]
    {
        eprintln!("cyber-sandbox-exec: the Unix proxy bridge is unavailable on this platform");
        ExitCode::from(69)
    }
}

/// Prepare bytes in a separate process: opening/closing a raced source alias
/// must not release the owner's process-wide SQLite record locks.
fn plugin_copy() -> ExitCode {
    use cyber_core::plugins::image::{MAX_REQUEST, MAX_RESPONSE, Request, prepare};
    let result = (|| -> Result<Vec<u8>, String> {
        let input = request(MAX_REQUEST).map_err(|_| "invalid package copy request")?;
        let request: Request =
            serde_json::from_str(&input).map_err(|_| "invalid package copy request")?;
        let copy = prepare(&request).map_err(|error| error.to_string())?;
        let response =
            serde_json::to_vec(&copy).map_err(|_| "package copy response unavailable")?;
        if response.len() > MAX_RESPONSE {
            return Err("package copy response exceeds supported size".into());
        }
        Ok(response)
    })();
    respond(result.map_err(Failure::Internal))
}

fn git_inspect() -> ExitCode {
    use cyber_core::worktrees::inspection::{MAX_RESPONSE, Request};
    let result = (|| -> Result<Vec<u8>, String> {
        let request: Request =
            serde_json::from_str(&request(8192)?).map_err(|error| error.to_string())?;
        let _isolated = isolated(&request.overrides)?;
        let report =
            cyber_core::worktrees::inspection::read(&request).map_err(|error| error.to_string())?;
        let response = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
        if response.len() > MAX_RESPONSE {
            return Err("inspection response exceeds supported size".into());
        }
        Ok(response)
    })();
    respond(result.map_err(Failure::Internal))
}

/// Resolves a review target. A typed target failure is written to stdout as
/// JSON with exit status 2; protocol and internal failures go to stderr (65).
fn git_review() -> ExitCode {
    use cyber_core::review::resolve::{MAX_REQUEST, MAX_RESPONSE, Request, ResolveError, resolve};
    let result = (|| -> Result<Vec<u8>, Failure> {
        let request = request(MAX_REQUEST).map_err(Failure::Internal)?;
        let request: Request =
            serde_json::from_str(&request).map_err(|error| Failure::Internal(error.to_string()))?;
        let _isolated = isolated(&request.overrides).map_err(Failure::Internal)?;
        let diff = resolve(&request).map_err(|error| match error {
            ResolveError::Target(error) => Failure::Target(error),
            ResolveError::Io(error) => Failure::Internal(error.to_string()),
        })?;
        let response = serde_json::to_vec(&diff).map_err(|e| Failure::Internal(e.to_string()))?;
        if response.len() > MAX_RESPONSE {
            return Err(Failure::Internal(
                "review response exceeds supported size".into(),
            ));
        }
        Ok(response)
    })();
    respond(result)
}

enum Failure {
    Target(cyber_core::review::TargetError),
    Internal(String),
}

fn request(limit: usize) -> Result<String, String> {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    let [request] = args.as_slice() else {
        return Err("expected one inspection request".into());
    };
    let request = request
        .to_str()
        .filter(|request| request.len() <= limit)
        .ok_or("invalid inspection request")?;
    Ok(request.to_owned())
}

/// Confines libgit2's ambient configuration search to an empty directory that
/// lives until the returned guard drops.
fn isolated(overrides: &std::path::Path) -> Result<tempfile::TempDir, String> {
    let parent = overrides
        .parent()
        .ok_or("inspection configuration has no parent")?;
    let isolated = tempfile::tempdir_in(parent).map_err(|error| error.to_string())?;
    isolate_config(isolated.path())?;
    Ok(isolated)
}

fn respond(result: Result<Vec<u8>, Failure>) -> ExitCode {
    use std::io::Write;
    let (bytes, code) = match result {
        Ok(bytes) => (bytes, ExitCode::SUCCESS),
        Err(Failure::Target(error)) => {
            eprintln!("cyber-sandbox-exec: {error}");
            match serde_json::to_vec(&error) {
                Ok(bytes) => (bytes, ExitCode::from(2)),
                Err(_) => return ExitCode::from(65),
            }
        }
        Err(Failure::Internal(error)) => {
            eprintln!("cyber-sandbox-exec: {error}");
            return ExitCode::from(65);
        }
    };
    match std::io::stdout().write_all(&bytes) {
        Ok(()) => code,
        Err(_) => ExitCode::from(74),
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
        // SAFETY: --git-inspect and --git-review run on the sole main thread, before any other
        // libgit2 access or runtime/thread creation. The directory stays alive throughout inspection.
        unsafe { git2::opts::set_search_path(level, directory) }
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
