//! Running bash under the OS sandbox (`sandbox`): roots, masked environment, the network
//! proxy and `network` permission requests for domains outside the allowlist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig, matches_domain, protected_in};
use tokio::sync::{mpsc, oneshot};

use crate::host::Ctx;
use crate::permissions::Request;
use crate::tools::{ToolError, failed};

/// A domain the proxy needs a decision for.
pub(crate) type NetworkAsk = (String, oneshot::Sender<bool>);

/// A command ready to spawn, with the proxy kept alive for its lifetime.
pub(crate) struct Prepared {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub asks: Option<mpsc::Receiver<NetworkAsk>>,
    _proxy: Option<Proxy>,
}

pub(crate) async fn prepare(
    ctx: &Ctx<'_>,
    shell: &str,
    command: &str,
) -> Result<Prepared, ToolError> {
    let (config, sources) = (ctx.host.opts.config)(&ctx.location).unwrap_or_default();
    let home = &ctx.host.opts.home;
    let sandbox = SandboxConfig::resolve(
        &config,
        &sources,
        ctx.host.opts.sandbox_policy.as_deref(),
        home,
    );
    let tmp = session_tmp(ctx)?;
    let (asks, proxy) =
        if sandbox.policy != Policy::FullAccess && sandbox.network == NetworkMode::Proxy {
            let (tx, rx) = mpsc::channel(16);
            let decide = decider(
                &sandbox.allowed_domains,
                ctx.host.network_approvals(&ctx.inv.session_id),
                tx,
            );
            let proxy = start_proxy(decide, &tmp)
                .await
                .map_err(|e| failed(format!("Could not start the sandbox network proxy: {e}")))?;
            (Some(rx), Some(proxy))
        } else {
            (None, None)
        };
    let writable = roots(ctx, &sandbox, &tmp);
    let launch = Launch {
        read_only: writable.iter().flat_map(|r| protected_in(r)).collect(),
        unreadable: sandbox.unreadable(home),
        writable,
        proxy: proxy.as_ref().map(|p| p.endpoint.clone()),
        helper: ctx.host.opts.sandbox_helper.clone(),
        config: sandbox.clone(),
    };
    let wrapped = cyber_sandbox::wrap(&launch, shell, &["-c".to_string(), command.to_string()])
        .map_err(|e| failed(e.to_string()))?;
    let base: Vec<(String, String)> = if sandbox.policy == Policy::FullAccess {
        std::env::vars().collect()
    } else {
        cyber_sandbox::mask_env(std::env::vars(), &sandbox.env_allow, &[])
    };
    let mut env: Vec<(String, String)> = base
        .into_iter()
        .filter(|(k, _)| !wrapped.env.contains_key(k) && k != "TMPDIR")
        .collect();
    env.extend(wrapped.env);
    env.push(("TMPDIR".into(), tmp.display().to_string()));
    Ok(Prepared {
        program: wrapped.program,
        args: wrapped.args,
        env,
        asks,
        _proxy: proxy,
    })
}

async fn start_proxy(decide: Decide, _tmp: &Path) -> std::io::Result<Proxy> {
    #[cfg(unix)]
    if cyber_sandbox::proxy_uses_unix_socket() {
        let socket = _tmp.join(format!(
            "proxy-{}.sock",
            ulid::Ulid::new().to_string().to_lowercase()
        ));
        return Proxy::start_unix(decide, &socket).await;
    }
    Proxy::start(decide).await
}

/// Allowlisted and session-approved domains pass at once; others wait for the tool loop.
fn decider(
    allowed: &[String],
    approved: Arc<Mutex<HashSet<String>>>,
    tx: mpsc::Sender<NetworkAsk>,
) -> Decide {
    let allowed = allowed.to_vec();
    Arc::new(move |host: String| {
        let known = matches_domain(&host, &allowed)
            || approved
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&host);
        let tx = tx.clone();
        Box::pin(async move {
            if known {
                return true;
            }
            let (reply, answer) = oneshot::channel();
            tx.send((host, reply)).await.is_ok() && answer.await.unwrap_or(false)
        })
    })
}

/// Ask for `network` on a domain; an approval holds for the rest of the Session.
pub(crate) async fn answer(ctx: &Ctx<'_>, host: String) -> bool {
    let req = Request {
        action: "network".into(),
        resources: vec![host.clone()],
        read_only: true,
        ..Request::default()
    };
    let allowed = ctx
        .authorize(
            req,
            vec![host.clone()],
            serde_json::json!({ "domain": host }),
        )
        .await
        .is_ok();
    if allowed {
        ctx.host
            .network_approvals(&ctx.inv.session_id)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host);
    }
    allowed
}

/// Location, worktree root, the Session's temp directory, managed tool output and
/// `sandbox.writable_roots`.
fn roots(ctx: &Ctx<'_>, sandbox: &SandboxConfig, tmp: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        ctx.location.clone(),
        cyber_core::config::project_root(&ctx.location),
        tmp.to_path_buf(),
        ctx.host.opts.tool_output_dir.clone(),
    ];
    // macOS tools such as mktemp use the per-user temp directory even when TMPDIR is set.
    if cfg!(target_os = "macos") {
        roots.push(std::env::temp_dir());
    }
    roots.extend(sandbox.extra_writable.iter().cloned());
    let mut unique: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

fn session_tmp(ctx: &Ctx<'_>) -> Result<PathBuf, ToolError> {
    let dir = ctx.host.opts.temp_dir.join(&ctx.inv.session_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| failed(format!("Could not create {}: {e}", dir.display())))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_sandbox::proxy::Endpoint;

    #[tokio::test]
    async fn tool_proxy_listens_on_the_supported_platform_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let decide: Decide = Arc::new(|_| Box::pin(async { false }));
        let proxy = start_proxy(decide, tmp.path()).await.unwrap();
        match &proxy.endpoint {
            Endpoint::Tcp(port) => {
                assert!(!cyber_sandbox::proxy_uses_unix_socket());
                tokio::net::TcpStream::connect(("127.0.0.1", *port))
                    .await
                    .unwrap();
            }
            Endpoint::Unix(path) => {
                assert!(cyber_sandbox::proxy_uses_unix_socket());
                #[cfg(unix)]
                tokio::net::UnixStream::connect(path).await.unwrap();
                #[cfg(not(unix))]
                panic!("unsupported Unix proxy endpoint: {}", path.display());
            }
        }
    }
}
