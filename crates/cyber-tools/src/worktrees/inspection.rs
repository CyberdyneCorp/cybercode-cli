//! Repository inspection uses the same sandbox, process tree and cancellation as Git.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cyber_core::worktrees::inspection::{Inspection, MAX_RESPONSE, OVERRIDES, Request, Target};

use super::{Capture, GitPort, tool_error};

struct Override(PathBuf);

impl Override {
    fn create(directory: &Path) -> io::Result<Self> {
        let path = directory.join(format!("git-inspect-{}.config", ulid::Ulid::new()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        let owned = Self(path);
        file.write_all(OVERRIDES.as_bytes())?;
        Ok(owned)
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(super) async fn run(port: &GitPort<'_>, target: &Target) -> io::Result<Inspection> {
    let helper = port.ctx.host.opts.sandbox_helper.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Repository inspection helper is unavailable",
        )
    })?;
    let program = helper
        .to_str()
        .ok_or_else(|| io::Error::other("Helper path is not Unicode"))?;
    let directory = crate::sandboxing::session_tmp(port.ctx).map_err(tool_error)?;
    let owned = Override::create(&directory)?;
    let request = serde_json::to_string(&Request {
        target: target.clone(),
        overrides: owned.0.clone(),
    })?;
    if request.len() > 8192 {
        return Err(io::Error::other(
            "Repository inspection request exceeds supported size",
        ));
    }
    let mut prepared = crate::sandboxing::prepare_worktree_command(
        port.ctx,
        program,
        &["--git-inspect".into(), request],
        port.credentials,
    )
    .await
    .map_err(tool_error)?;
    prepared
        .env
        .retain(|(key, _)| !key.to_ascii_uppercase().starts_with("GIT_"));
    prepared.env.extend([
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        (
            "GIT_CONFIG_GLOBAL".into(),
            if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
        ),
    ]);
    let capture = Capture::default();
    let status = super::run(port.ctx, prepared, &target.metadata, &capture).await?;
    let (stdout, stderr) = capture
        .0
        .into_inner()
        .map_err(|_| io::Error::other("Inspection capture poisoned"))?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "Repository inspection failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    if stdout.len() > MAX_RESPONSE {
        return Err(io::Error::other(
            "Repository inspection response exceeds supported size",
        ));
    }
    let report: Inspection = serde_json::from_slice(&stdout)?;
    if report.target != *target {
        return Err(io::Error::other(
            "Repository inspection response identity changed",
        ));
    }
    Ok(report)
}
