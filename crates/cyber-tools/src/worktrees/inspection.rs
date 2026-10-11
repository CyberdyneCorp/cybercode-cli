//! Repository inspection uses the same sandbox, process tree and cancellation as Git.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cyber_core::worktrees::inspection::{Inspection, MAX_RESPONSE, OVERRIDES, Request, Target};

use super::{GitPort, tool_error};
use crate::host::Ctx;

/// A private helper input file, removed on drop.
pub(crate) struct OwnedFile(pub(crate) PathBuf);

impl OwnedFile {
    /// The fixed read-only Git overrides.
    pub(crate) fn overrides(directory: &Path) -> io::Result<Self> {
        Self::create(directory, "git-inspect", "config", OVERRIDES.as_bytes())
    }

    pub(crate) fn create(
        directory: &Path,
        stem: &str,
        extension: &str,
        contents: &[u8],
    ) -> io::Result<Self> {
        let path = directory.join(format!("{stem}-{}.{extension}", ulid::Ulid::new()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        let owned = Self(path);
        file.write_all(contents)?;
        Ok(owned)
    }
}

impl Drop for OwnedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs one owned helper mode (`--git-inspect`, `--git-review`) in the command
/// sandbox with ambient Git configuration excluded.
pub(crate) async fn run_helper(
    ctx: &Ctx<'_>,
    credentials: &[String],
    mode: &str,
    request: String,
    directory: &Path,
    limit: usize,
) -> io::Result<std::process::Output> {
    let helper = ctx.host.opts.sandbox_helper.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Repository inspection helper is unavailable",
        )
    })?;
    let program = helper
        .to_str()
        .ok_or_else(|| io::Error::other("Helper path is not Unicode"))?;
    let mut prepared = crate::sandboxing::prepare_worktree_command(
        ctx,
        program,
        &[mode.into(), request],
        credentials,
    )
    .await
    .map_err(tool_error)?;
    super::isolate_git_config(&mut prepared);
    super::run_captured(ctx, prepared, directory, limit).await
}

pub(super) async fn run(port: &GitPort<'_>, target: &Target) -> io::Result<Inspection> {
    let directory = crate::sandboxing::session_tmp(port.ctx).map_err(tool_error)?;
    let owned = OwnedFile::overrides(&directory)?;
    let request = serde_json::to_string(&Request {
        target: target.clone(),
        overrides: owned.0.clone(),
    })?;
    if request.len() > 8192 {
        return Err(io::Error::other(
            "Repository inspection request exceeds supported size",
        ));
    }
    let output = run_helper(
        port.ctx,
        port.credentials,
        "--git-inspect",
        request,
        &target.metadata,
        MAX_RESPONSE,
    )
    .await?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Repository inspection failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    if output.stdout.len() > MAX_RESPONSE {
        return Err(io::Error::other(
            "Repository inspection response exceeds supported size",
        ));
    }
    let report: Inspection = serde_json::from_slice(&output.stdout)?;
    if report.target != *target {
        return Err(io::Error::other(
            "Repository inspection response identity changed",
        ));
    }
    Ok(report)
}
