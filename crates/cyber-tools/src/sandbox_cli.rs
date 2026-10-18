//! `cyber sandbox explain|test|run` (`sandbox` → Sandbox CLI): the effective policy of a
//! Location, a probe suite and one command under it, outside any Session. Nothing here
//! asks: a domain outside the allowlist is refused.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig, protected_in};
use serde::Serialize;

use crate::BuiltinHost;

/// What `cyber sandbox explain` prints.
#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub profile: String,
    pub policy: String,
    pub enforcement: String,
    pub isolation: String,
    pub writable_roots: Vec<String>,
    pub read_only: Vec<String>,
    pub unreadable: Vec<String>,
    pub deny_read: Vec<String>,
    pub network: String,
    pub allowed_domains: Vec<String>,
    pub denied_domains: Vec<String>,
    pub env_mode: String,
    pub login_shell: bool,
    pub env_set: BTreeMap<String, String>,
    /// Names in this process's environment a sandboxed command does not receive.
    pub masked: Vec<String>,
    pub excluded_commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A command ready to spawn under the effective sandbox; keep it alive while it runs.
pub struct Standalone {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Hosts the proxy refused.
    pub refused: Arc<Mutex<Vec<String>>>,
    _proxy: Option<Proxy>,
    _tmp: RunDir,
}

/// The private temporary directory of one standalone command, removed afterwards.
struct RunDir(PathBuf);

impl RunDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One probe of `cyber sandbox test`.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub operation: String,
    pub target: String,
    pub expected: String,
    pub actual: String,
    pub passed: bool,
}

impl BuiltinHost {
    /// The effective sandbox of `location`, as `cyber sandbox explain` prints it.
    pub fn sandbox_explain(&self, location: &Path) -> Explanation {
        let sandbox = self.sandbox_config(location, None);
        let tmp = self.opts.temp_dir.join("<session>");
        let writable = root_list(location, &tmp, &self.opts.tool_output_dir, &sandbox, true);
        let status = self.sandbox_status(location);
        let (config, _) = (self.opts.config)(location).unwrap_or_default();
        let kept: Vec<String> = environment(std::env::vars(), &sandbox, &config)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let mut masked: Vec<String> = std::env::vars()
            .map(|(name, _)| name)
            .filter(|name| !kept.contains(name))
            .collect();
        masked.sort();
        let text = |paths: &[PathBuf]| paths.iter().map(|p| p.display().to_string()).collect();
        Explanation {
            profile: sandbox.profile.clone(),
            policy: sandbox.policy.as_str().into(),
            enforcement: status["enforcement"].as_str().unwrap_or_default().into(),
            isolation: status["isolation"].as_str().unwrap_or_default().into(),
            read_only: text(
                &writable
                    .iter()
                    .flat_map(|r| protected_in(r))
                    .collect::<Vec<_>>(),
            ),
            writable_roots: text(&writable),
            unreadable: text(&sandbox.unreadable(&self.opts.home)),
            deny_read: sandbox.deny_read_for(location, &self.opts.home).anchored(),
            network: sandbox.network.as_str().into(),
            allowed_domains: sandbox.allowed_domains.clone(),
            denied_domains: sandbox.denied_domains.clone(),
            env_mode: if sandbox.env.clean { "none" } else { "inherit" }.into(),
            login_shell: sandbox.env.login_shell,
            env_set: sandbox.env.set.clone(),
            masked,
            excluded_commands: sandbox.excluded_commands.clone(),
            notice: sandbox.notice.clone(),
            error: sandbox.error.clone(),
        }
    }

    /// `program args` under the effective sandbox of `location`.
    pub async fn sandbox_command(
        &self,
        location: &Path,
        program: &str,
        args: &[String],
    ) -> Result<Standalone, String> {
        let sandbox = self.sandbox_config(location, None);
        let tmp = RunDir(self.opts.temp_dir.join(format!(
            "sandbox-run-{}",
            ulid::Ulid::new().to_string().to_lowercase()
        )));
        std::fs::create_dir_all(tmp.path()).map_err(|e| e.to_string())?;
        let refused = Arc::new(Mutex::new(Vec::new()));
        let proxy = if sandbox.policy != Policy::FullAccess && sandbox.network == NetworkMode::Proxy
        {
            Some(start_proxy(refusing(&sandbox, Arc::clone(&refused)), tmp.path()).await?)
        } else {
            None
        };
        let writable = root_list(
            location,
            tmp.path(),
            &self.opts.tool_output_dir,
            &sandbox,
            true,
        );
        let launch = Launch {
            read_only: writable.iter().flat_map(|r| protected_in(r)).collect(),
            unreadable: sandbox.unreadable(&self.opts.home),
            deny_read: sandbox.deny_read_for(location, &self.opts.home),
            writable,
            proxy: proxy.as_ref().map(|p| p.endpoint.clone()),
            helper: self.opts.sandbox_helper.clone(),
            config: sandbox.clone(),
        };
        let wrapped = cyber_sandbox::wrap(&launch, program, args).map_err(|e| e.to_string())?;
        let (config, _) = (self.opts.config)(location).unwrap_or_default();
        let mut env: Vec<(String, String)> = environment(std::env::vars(), &sandbox, &config)
            .into_iter()
            .filter(|(k, _)| !wrapped.env.contains_key(k) && k != "TMPDIR")
            .collect();
        env.extend(wrapped.env);
        env.push(("TMPDIR".into(), tmp.path().display().to_string()));
        env.push(("CYBER".into(), "1".into()));
        env.push(("CYBER_PROJECT_DIR".into(), location.display().to_string()));
        Ok(Standalone {
            program: wrapped.program,
            args: wrapped.args,
            env,
            refused,
            _proxy: proxy,
            _tmp: tmp,
        })
    }

    /// Run `script` with `/bin/sh -c` under the sandbox and return (success, output).
    async fn probe_run(&self, location: &Path, script: &str) -> Result<(bool, String), String> {
        let command = self
            .sandbox_command(location, "/bin/sh", &["-c".into(), script.into()])
            .await?;
        let output = tokio::process::Command::new(&command.program)
            .args(&command.args)
            .env_clear()
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .current_dir(location)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.success(), text))
    }

    /// The probe suite of `cyber sandbox test`: reads, writes and the network.
    pub async fn sandbox_probes(&self, location: &Path) -> Result<Vec<Probe>, String> {
        let sandbox = self.sandbox_config(location, None);
        let enforced = sandbox.policy != Policy::FullAccess;
        let id = ulid::Ulid::new().to_string().to_lowercase();
        let mut probes = Vec::new();
        let (ok, out) = self.probe_run(location, "ls . >/dev/null").await?;
        probes.push(probe("read", location, true, ok, &out));
        let inside = location.join(format!(".cyber-sandbox-probe-{id}"));
        let script = format!(
            "echo probe > '{}' && rm -f '{}'",
            quoted(&inside),
            quoted(&inside)
        );
        let (ok, out) = self.probe_run(location, &script).await?;
        let _ = std::fs::remove_file(&inside);
        probes.push(probe(
            "write",
            &inside,
            sandbox.policy != Policy::ReadOnly,
            ok,
            &out,
        ));
        let outside = self.opts.home.join(format!(".cyber-sandbox-probe-{id}"));
        let script = format!("echo probe > '{}'", quoted(&outside));
        let (ok, out) = self.probe_run(location, &script).await?;
        let _ = std::fs::remove_file(&outside);
        // Home may itself lie in a writable root (on macOS, under the per-user temp).
        let roots = root_list(
            location,
            &self.opts.temp_dir,
            &self.opts.tool_output_dir,
            &sandbox,
            true,
        );
        let writable = !enforced || roots.iter().any(|root| outside.starts_with(root));
        probes.push(probe("write", &outside, writable, ok, &out));
        if let Some(secret) = sandbox
            .unreadable(&self.opts.home)
            .into_iter()
            .find(|p| p.exists())
        {
            let reader = if secret.is_dir() { "ls" } else { "cat" };
            let script = format!("{reader} '{}' >/dev/null", quoted(&secret));
            let (ok, out) = self.probe_run(location, &script).await?;
            probes.push(probe("read", &secret, !enforced, ok, &out));
        }
        if enforced && sandbox.network != NetworkMode::On {
            probes.push(self.network_probe(location).await?);
        }
        Ok(probes)
    }

    async fn network_probe(&self, location: &Path) -> Result<Probe, String> {
        const HOST: &str = "cyber-sandbox-probe.invalid";
        let script = format!(
            "command -v curl >/dev/null || {{ echo skipped; exit 0; }}; curl -s -o /dev/null -w '%{{http_code}}' --max-time 10 http://{HOST}/"
        );
        let (_, out) = self.probe_run(location, &script).await?;
        let reached = out.trim() != "403" && !out.contains("skipped") && out.trim() != "000";
        Ok(Probe {
            operation: "network".into(),
            target: HOST.into(),
            expected: "denied".into(),
            actual: if out.contains("skipped") {
                "skipped (curl not installed)".into()
            } else if reached {
                "allowed".into()
            } else {
                "denied".into()
            },
            passed: !reached,
        })
    }
}

fn probe(operation: &str, target: &Path, allowed: bool, ok: bool, out: &str) -> Probe {
    let actual = if ok { "allowed" } else { "denied" };
    let mut actual = actual.to_string();
    if !ok && let Some(line) = out.lines().find(|l| !l.trim().is_empty()) {
        actual = format!("denied ({})", line.trim());
    }
    Probe {
        operation: operation.into(),
        target: target.display().to_string(),
        expected: if allowed { "allowed" } else { "denied" }.into(),
        passed: ok == allowed,
        actual,
    }
}

fn quoted(path: &Path) -> String {
    path.display().to_string().replace('\'', "'\\''")
}

fn environment(
    vars: impl Iterator<Item = (String, String)>,
    sandbox: &SandboxConfig,
    config: &serde_json::Value,
) -> Vec<(String, String)> {
    if sandbox.policy == Policy::FullAccess {
        return vars.collect();
    }
    cyber_sandbox::policy_env(
        vars,
        sandbox,
        &crate::sandboxing::credential_env_names(config),
    )
}

/// The writable roots of a sandboxed command in `location` (`sandbox` → Writable roots).
pub(crate) fn root_list(
    location: &Path,
    tmp: &Path,
    tool_output: &Path,
    sandbox: &SandboxConfig,
    ambient_temp: bool,
) -> Vec<PathBuf> {
    let mut roots = vec![
        location.to_path_buf(),
        cyber_core::config::project_root(location),
        tmp.to_path_buf(),
        tool_output.to_path_buf(),
    ];
    // macOS tools such as mktemp use the per-user temp directory even when TMPDIR is set.
    if cfg!(target_os = "macos") && ambient_temp {
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

/// Allowlisted domains pass; every other one is refused and remembered.
fn refusing(sandbox: &SandboxConfig, refused: Arc<Mutex<Vec<String>>>) -> Decide {
    let sandbox = sandbox.clone();
    Arc::new(move |host: String| {
        let allowed = sandbox.domain_allowed(&host);
        if !allowed {
            refused
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(host);
        }
        Box::pin(async move { allowed })
    })
}

async fn start_proxy(decide: Decide, _tmp: &Path) -> Result<Proxy, String> {
    #[cfg(unix)]
    if cyber_sandbox::proxy_uses_unix_socket() {
        return Proxy::start_unix(decide, &_tmp.join("proxy.sock"))
            .await
            .map_err(|e| e.to_string());
    }
    Proxy::start(decide).await.map_err(|e| e.to_string())
}
