//! Automatic formatting after an authorized edit, before language-server feedback.
use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};

use cyber_core::{
    config::FormatterSettings,
    intelligence::{DetectedFormatter, ExecutableSearch, detect_formatters},
    paths::Paths,
};
use serde_json::json;
use tokio::io::AsyncReadExt;

use super::{ToolError, failed, process::Process};
use crate::host::Ctx;

const TIMEOUT: Duration = Duration::from_secs(30);

fn selected(ctx: &Ctx<'_>, path: &Path) -> Result<Vec<DetectedFormatter>, ToolError> {
    let (config, _) = (ctx.host.opts.config)(&ctx.location).map_err(failed)?;
    let settings = FormatterSettings::from_config(&config).map_err(failed)?;
    let paths = Paths::resolve(ctx.host.opts.env.as_ref(), &ctx.host.opts.home);
    let environment: HashMap<String, String> = ["PATH", "PATHEXT"]
        .into_iter()
        .filter_map(|key| ctx.host.opts.env.get(key).map(|value| (key.into(), value)))
        .collect();
    let search = ExecutableSearch::new(&ctx.location, &paths.cache, &environment);
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{s}"));
    Ok(detect_formatters(&settings, &search)
        .into_iter()
        .filter(|formatter| {
            formatter.enabled
                && extension
                    .as_ref()
                    .is_some_and(|extension| formatter.definition.extensions.contains(extension))
        })
        .collect())
}

fn log(ctx: &Ctx<'_>, path: &Path, id: &str, reason: &str) {
    cyber_core::log::warn(
        "formatter",
        "Formatter failed after file edit",
        json!({
            "session_id":ctx.inv.session_id,"call_id":ctx.inv.call_id,
            "path":ctx.resource(path),"formatter":id,"reason":reason
        }),
    );
}

/// The tool owner retains this future, sandbox/proxy and native process through settlement.
pub(super) async fn apply(ctx: &Ctx<'_>, path: &Path, edited: &[u8]) -> (Vec<u8>, String) {
    let formatters = match selected(ctx, path) {
        Ok(formatters) => formatters,
        Err(_) => {
            log(
                ctx,
                path,
                "configuration",
                "formatter configuration unavailable",
            );
            return (edited.to_vec(), String::new());
        }
    };
    if formatters.is_empty() || ctx.cancel.is_cancelled() {
        return (edited.to_vec(), String::new());
    }
    let Ok(canonical) = path.canonicalize() else {
        log(ctx, path, "admission", "edited file unavailable");
        return (edited.to_vec(), String::new());
    };
    // Never give an automatic command an external or redirected edit target.
    if canonical != path || !canonical.starts_with(&ctx.location) || !canonical.is_file() {
        log(
            ctx,
            path,
            "admission",
            "formatter requires a canonical file in its Location",
        );
        return (edited.to_vec(), String::new());
    }
    let lock = ctx.host.path_lock(path);
    let _guard = lock.lock().await;
    let mut current = edited.to_vec();
    for formatter in formatters {
        if ctx.cancel.is_cancelled()
            || read_regular(path).await.as_deref() != Some(current.as_slice())
        {
            break;
        }
        if let Err(reason) = execute(ctx, path, &current, &formatter, TIMEOUT).await {
            log(ctx, path, &formatter.definition.id, &reason);
        }
        match read_regular(path).await {
            Some(bytes) => current = bytes,
            None => {
                log(
                    ctx,
                    path,
                    &formatter.definition.id,
                    "formatted file unavailable",
                );
                break;
            }
        }
    }
    if current == edited {
        return (current, String::new());
    }
    let before = String::from_utf8_lossy(edited);
    let after = String::from_utf8_lossy(&current);
    let output = json!({"formatted_file":ctx.resource(path),"content":after,
        "diff":super::fs::unified_diff(&ctx.resource(path), &before, &after)});
    (
        current,
        format!(
            "\nFormatting result:\n{}",
            serde_json::to_string_pretty(&output).unwrap()
        ),
    )
}

async fn execute(
    ctx: &Ctx<'_>,
    path: &Path,
    expected: &[u8],
    formatter: &DetectedFormatter,
    timeout: Duration,
) -> Result<(), String> {
    let authority =
        (ctx.host.opts.config)(&ctx.location).map_err(|_| "formatter configuration unavailable")?;
    let executable = formatter
        .executable
        .as_ref()
        .and_then(|path| path.to_str())
        .ok_or("formatter executable unavailable")?;
    let file = path.to_str().ok_or("formatter path is not UTF-8")?;
    let args: Vec<String> = formatter
        .definition
        .command
        .iter()
        .skip(1)
        .map(|arg| arg.replace("$FILE", file))
        .collect();
    let mut prepared = crate::sandboxing::prepare_formatter_command(ctx, executable, &args)
        .await
        .map_err(|_| "sandbox enforcement unavailable")?;
    // Automatic formatting uses the sandbox allowlist; unknown domains receive no approval.
    drop(prepared.asks.take());
    for (key, value) in &formatter.definition.env {
        if !reserved(key) {
            prepared.env.retain(|(existing, _)| existing != key);
            prepared.env.push((key.clone(), value.clone()));
        }
    }
    if ctx.cancel.is_cancelled() {
        return Err("formatter cancelled before launch".into());
    }
    // Re-resolve immediately before native effects; a changed command/env is not replayed.
    if !selected(ctx, path)
        .map_err(|_| "formatter configuration unavailable")?
        .iter()
        .any(|fresh| {
            fresh.definition.id == formatter.definition.id
                && fresh.definition.command == formatter.definition.command
                && fresh.definition.env == formatter.definition.env
                && fresh.executable == formatter.executable
        })
    {
        return Err("formatter configuration changed before launch".into());
    }
    if (ctx.host.opts.config)(&ctx.location).map_err(|_| "formatter configuration unavailable")?
        != authority
        || read_regular(path).await.as_deref() != Some(expected)
    {
        return Err("formatter authority or edited source changed before launch".into());
    }
    let mut process = Process::spawn(
        &prepared.program,
        &prepared.args,
        ctx.host.opts.sandbox_helper.as_deref(),
        |command| {
            command
                .env_clear()
                .envs(prepared.env.iter().map(|(key, value)| (key, value)))
                .current_dir(&ctx.location)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        },
    )
    .await
    .map_err(|_| "formatter native launch failed")?;
    capture(ctx, &mut process, timeout).await
}

async fn capture(ctx: &Ctx<'_>, process: &mut Process, timeout: Duration) -> Result<(), String> {
    let stdout = process.stdout().ok_or("formatter stdout unavailable")?;
    let stderr = process.stderr().ok_or("formatter stderr unavailable")?;
    let result = {
        let operation = async {
            let (status, out, err) =
                tokio::join!(process.wait_tree(), drain(stdout), drain(stderr));
            out.map_err(|_| "formatter stdout failed")?;
            err.map_err(|_| "formatter stderr failed")?;
            status.map_err(|_| "formatter wait failed")
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => None,
            _ = tokio::time::sleep(timeout) => None,
            result = &mut operation => Some(result),
        }
    };
    match result {
        Some(Ok(status)) if status.success() => Ok(()),
        Some(Ok(status)) => Err(format!("formatter exited with code {:?}", status.code())),
        stopped => {
            process.terminate();
            // Retain native ownership and sandbox resources until termination is observed.
            while process.wait_tree().await.is_err() {
                process.terminate();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(stopped.map_or_else(
                || "formatter timed out or cancelled".into(),
                |error| error.unwrap_err().into(),
            ))
        }
    }
}

async fn read_regular(path: &Path) -> Option<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        if path.canonicalize().ok()? != path {
            return None;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
        let mut file = options.open(&path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return None;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let fresh = std::fs::symlink_metadata(&path).ok()?;
            if fresh.dev() != metadata.dev() || fresh.ino() != metadata.ino() {
                return None;
            }
        }
        (path.canonicalize().ok()? == path).then_some(bytes)
    })
    .await
    .ok()
    .flatten()
}

async fn drain(mut stream: super::process::ReadStream) -> std::io::Result<()> {
    let mut buffer = [0; 8192];
    while stream.read(&mut buffer).await? != 0 {}
    Ok(())
}

fn reserved(key: &str) -> bool {
    matches!(
        key.to_ascii_uppercase().as_str(),
        "TMPDIR"
            | "TEMP"
            | "TMP"
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "ALL_PROXY"
            | "NO_PROXY"
            | "CYBER_PROXY_SOCKET"
            | "CYBER_PROXY_PORT"
    )
}
