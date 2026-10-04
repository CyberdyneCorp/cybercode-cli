//! Linux enforcement with bubblewrap (`sandbox` → Linux enforcement).
//!
//! Bubblewrap mounts the host read-only, binds the writable roots back writable, re-binds
//! protected paths read-only on top, and hides credential files. Without network access the
//! command gets a private network namespace; in proxy mode `cyber-sandbox-exec` forwards a
//! loopback port inside it to the proxy's Unix socket.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::proxy::Endpoint;
use crate::{INNER_PROXY_PORT, Launch, NetworkMode, Policy, SandboxError, Wrapped};

/// The `bwrap` executable on `PATH`.
pub(crate) fn bwrap() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("bwrap"))
        .find(|p| p.is_file())
}

pub(crate) fn wrap(
    launch: &Launch,
    program: &str,
    args: &[String],
    env: BTreeMap<String, String>,
) -> Result<Wrapped, SandboxError> {
    let bwrap = bwrap().ok_or_else(|| {
        SandboxError::Unavailable(
            "bubblewrap (bwrap) is not installed; install it or run with --sandbox full-access"
                .into(),
        )
    })?;
    let mut out: Vec<String> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-ipc",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
    ]
    .map(str::to_string)
    .to_vec();
    if fresh_proc(&bwrap) {
        out.extend(["--unshare-pid", "--proc", "/proc"].map(str::to_string));
    } else {
        // Containers often mask /proc so a new one cannot be mounted; share the host's.
        out.extend(["--ro-bind", "/proc", "/proc"].map(str::to_string));
    }
    if launch.config.network != NetworkMode::On {
        out.push("--unshare-net".into());
    }
    if launch.config.policy == Policy::WorkspaceWrite {
        for root in &launch.writable {
            if root.exists() {
                bind(&mut out, "--bind", root);
            }
        }
        for path in launch.read_only.iter().filter(|p| p.exists()) {
            bind(&mut out, "--ro-bind", path);
        }
    }
    for path in launch.unreadable.iter().filter(|p| p.exists()) {
        if path.is_dir() {
            out.extend(["--tmpfs".into(), text(path)]);
        } else {
            out.extend(["--ro-bind".into(), "/dev/null".into(), text(path)]);
        }
    }
    if let Some(Endpoint::Unix(socket)) = &launch.proxy {
        let helper = launch.helper.as_ref().ok_or_else(|| {
            SandboxError::Unavailable("the sandbox helper cyber-sandbox-exec was not found".into())
        })?;
        // The socket's directory must be reachable inside; it already is through `/`.
        out.extend([
            "--".into(),
            text(helper),
            "--forward".into(),
            INNER_PROXY_PORT.to_string(),
            text(socket),
        ]);
    }
    out.extend(["--".into(), program.into()]);
    out.extend_from_slice(args);
    Ok(Wrapped {
        program: text(&bwrap),
        args: out,
        env,
    })
}

/// Whether bubblewrap may mount a fresh /proc in a new PID namespace (probed once).
fn fresh_proc(bwrap: &Path) -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        Command::new(bwrap)
            .args([
                "--unshare-pid",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--",
                "true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn bind(out: &mut Vec<String>, flag: &str, path: &Path) {
    out.extend([flag.to_string(), text(path), text(path)]);
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
