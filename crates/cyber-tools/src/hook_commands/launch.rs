//! Command launch from resolved provenance and freshly checked checkout trust.
//! Callers retain execution ownership and admit durable receipts around this future.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookCatalog, HookDefinition, HookEvent};
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig};
use tokio_util::sync::CancellationToken;

use super::{HookCommandError, HookCommandReport, capture_hook_command, interpret_hook_command};
use crate::HookCommandProcess;

/// Noninteractive launch settings. The runtime still supplies pinned event identity,
/// current configuration, durable admission and cancellation ownership.
pub struct HookCommandRunner<'a> {
    pub resolved: &'a Resolved,
    pub trust: &'a TrustStore,
    pub invocation_trust: Option<&'a HookInvocationTrust>,
    pub home: &'a Path,
    pub temp_dir: &'a Path,
    pub shell: &'a str,
    pub helper: Option<&'a Path>,
    pub credential_env_names: &'a [String],
}

impl HookCommandRunner<'_> {
    /// Select by indexed config pointer; never accept a caller-mutated definition.
    /// Unknown proxy domains are refused; interactive network asks require runtime wiring.
    pub async fn run(
        &self,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        self.run_inner(pointer, event, cancel, None).await
    }

    /// Runtime-owned command execution with durable admission, cancellation tracking
    /// and a terminal receipt. Lifecycle dispatch and decision application are separate.
    pub async fn run_recorded(
        &self,
        runtime: &cyber_server::runtime::Runtime,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        let (definition, _) = self.authorize(pointer, event)?;
        if cancel.is_cancelled() {
            return Err("hook command cancelled before admission".into());
        }
        let log_io = self.resolved.value["telemetry"]["log_hook_io"]
            .as_bool()
            .unwrap_or(false);
        let Some(mut owner) = runtime
            .try_start_hook_execution(event, &definition, log_io)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(HookCommandReport {
                outcome: super::HookOutcome::Skipped,
                decision: Default::default(),
                ignored_fields: Vec::new(),
                diagnostic: None,
                acknowledged: true,
                must_stop: false,
            });
        };
        if let Some(message) = &definition.handler.status_message {
            runtime.hook_notice(
                &event.identity().session_id,
                &owner.record().hook_id,
                message,
            );
        }
        let stop = owner.cancellation();
        let result = {
            let execution =
                self.run_inner(pointer, event, stop.clone(), Some((&mut owner, runtime)));
            tokio::pin!(execution);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {stop.cancel(); execution.await}
                result = &mut execution => result,
            }
        };
        let mut report = match &result {
            Ok(report) => cyber_server::runtime::HookExecutionResult {
                outcome: report.outcome,
                decision: report.decision.clone(),
                acknowledged: report.acknowledged,
                must_stop: report.must_stop,
                io: None,
            },
            Err(_) => cyber_server::runtime::HookExecutionResult {
                outcome: if stop.is_cancelled() {
                    super::HookOutcome::Skipped
                } else {
                    super::HookOutcome::Error
                },
                decision: Default::default(),
                acknowledged: true,
                must_stop: stop.is_cancelled(),
                io: None,
            },
        };
        let fenced = owner.verify(runtime).is_err();
        report.must_stop |= fenced;
        let record = owner.finish(report).map_err(|error| error.to_string())?;
        if record.acknowledged == Some(true)
            && let Some(message) = &definition.handler.system_message
        {
            runtime.hook_notice(&record.session_id, &record.hook_id, message);
        }
        result.map(|mut report| {
            report.must_stop |= fenced;
            report
        })
    }

    async fn run_inner(
        &self,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
        mut owner: Option<(
            &mut cyber_server::runtime::HookExecution,
            &cyber_server::runtime::Runtime,
        )>,
    ) -> Result<HookCommandReport, String> {
        let (definition, sandbox_all) = self.authorize(pointer, event)?;
        if definition.handler.once && owner.is_none() {
            return Err("Once hooks require durable runtime admission".into());
        }
        if cancel.is_cancelled() {
            return Err("hook command cancelled before launch".into());
        }
        if cfg!(windows) {
            return Err("Windows hook command launch requires an owned event-stdin route".into());
        }
        let timeout = Duration::from_secs(definition.handler.timeout.into());
        let deadline = tokio::time::Instant::now() + timeout;
        let mut prepared = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("hook command cancelled before launch".into()),
            _ = tokio::time::sleep_until(deadline) => return Err("hook command preparation timed out".into()),
            prepared = self.prepare(&definition, sandbox_all, event) => prepared?,
        };
        // Preparation may await a proxy; revocation during that await must still refuse.
        self.authorize(pointer, event)?;
        if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline {
            return Err("hook command cancelled or expired before launch".into());
        }
        if let Some((owner, runtime)) = owner.as_mut() {
            owner
                .mark_launch(runtime)
                .map_err(|error| error.to_string())?;
        }
        let spawned = HookCommandProcess::spawn(
            &prepared.wrapped.program,
            &prepared.wrapped.args,
            self.helper,
            |command| {
                command
                    .env_clear()
                    .envs(prepared.env.iter().map(|(key, value)| (key, value)))
                    .current_dir(&event.identity().location.directory)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            },
        )
        .await;
        let capture = match spawned {
            Ok(process) => {
                prepared.scratch.cleanup = false;
                let result = capture_hook_command(
                    process,
                    event,
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    cancel,
                )
                .await;
                prepared.scratch.cleanup = match &result {
                    Ok(capture) => match capture.end {
                        super::HookCommandEnd::Exited(_) => true,
                        super::HookCommandEnd::TimedOut { acknowledged }
                        | super::HookCommandEnd::Cancelled { acknowledged } => acknowledged,
                    },
                    Err(error) => error.acknowledged,
                };
                result
            }
            Err(error) => Err(HookCommandError {
                message: format!("could not launch hook command: {error}"),
                acknowledged: true,
            }),
        };
        Ok(interpret_hook_command(
            event,
            definition
                .handler
                .id
                .as_deref()
                .unwrap_or(&definition.digest),
            definition.handler.fail_closed,
            capture,
        ))
    }

    fn authorize(
        &self,
        pointer: &str,
        event: &HookEvent,
    ) -> Result<(HookDefinition, bool), String> {
        let location = &event.identity().location.directory;
        let checkout = std::fs::canonicalize(cyber_core::config::project_root(location))
            .map_err(|error| error.to_string())?;
        if checkout != self.resolved.trust.checkout_root {
            return Err("hook event Location does not match resolved checkout".into());
        }
        let catalog = HookCatalog::from_config(self.resolved)?;
        let definition = catalog
            .definitions
            .into_iter()
            .find(|definition| definition.pointer == pointer)
            .ok_or("hook definition is absent from loaded configuration")?;
        if definition.event != event.event() || definition.kind() != HookKind::Command {
            return Err("hook command type or event does not match".into());
        }
        if definition.scope.requires_handler_trust() {
            let workspace_approved = match &self.resolved.trust.digest {
                Some(digest) => self.trust.is_approved(&checkout, digest),
                None => Ok(false),
            }
            .map_err(|error| error.to_string())?;
            if !self.resolved.trust.trusted || !workspace_approved {
                return Err("hook checkout configuration is untrusted".into());
            }
        }
        if !definition
            .is_trusted(&checkout, self.trust, self.invocation_trust)
            .map_err(|error| error.to_string())?
        {
            return Err("hook handler digest is untrusted".into());
        }
        Ok((definition, catalog.settings.sandbox_all))
    }

    async fn prepare(
        &self,
        definition: &HookDefinition,
        sandbox_all: bool,
        event: &HookEvent,
    ) -> Result<Prepared, String> {
        let mut sandbox = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        sandbox.policy = if definition.scope.requires_sandbox(sandbox_all) {
            Policy::WorkspaceWrite
        } else {
            Policy::FullAccess
        };
        let scratch = Scratch::new(self.temp_dir).map_err(|error| error.to_string())?;
        let proxy = start_proxy(&sandbox, &scratch.path)
            .await
            .map_err(|error| error.to_string())?;
        let writable = vec![
            self.resolved.trust.checkout_root.clone(),
            scratch.path.clone(),
        ]
        .into_iter()
        .chain(sandbox.extra_writable.iter().cloned())
        .collect::<Vec<_>>();
        let launch = Launch {
            read_only: writable
                .iter()
                .flat_map(|root| cyber_sandbox::protected_in(root))
                .collect(),
            unreadable: sandbox.unreadable(self.home),
            writable,
            proxy: proxy.as_ref().map(|proxy| proxy.endpoint.clone()),
            helper: self.helper.map(Path::to_path_buf),
            config: sandbox.clone(),
        };
        let command = definition
            .handler
            .command
            .as_ref()
            .ok_or("missing hook command")?;
        let wrapped = cyber_sandbox::wrap(
            &launch,
            &crate::tools::bash::shell(self.shell),
            &["-c".into(), command.clone()],
        )
        .map_err(|error| error.to_string())?;
        let mut credentials = crate::sandboxing::credential_env_names(&self.resolved.value);
        credentials.extend_from_slice(self.credential_env_names);
        let mut env = if sandbox.policy == Policy::FullAccess {
            std::env::vars().collect::<Vec<_>>()
        } else {
            cyber_sandbox::mask_env(std::env::vars(), &sandbox.env_allow, &credentials)
        };
        env.retain(|(key, _)| !wrapped.env.contains_key(key) && key != "TMPDIR");
        env.extend(wrapped.env.clone());
        env.push(("TMPDIR".into(), scratch.path.display().to_string()));
        let identity = event.identity();
        for (key, value) in [
            (
                "CYBER_PROJECT_DIR",
                self.resolved.trust.checkout_root.display().to_string(),
            ),
            ("CYBER_SESSION_ID", identity.session_id.clone()),
            ("CYBER_HOOK_EVENT", event.event().into()),
            ("CYBER_AGENT", identity.agent.clone()),
        ] {
            env.retain(|(existing, _)| existing != key);
            env.push((key.into(), value));
        }
        Ok(Prepared {
            wrapped,
            env,
            scratch,
            _proxy: proxy,
        })
    }
}

struct Prepared {
    wrapped: cyber_sandbox::Wrapped,
    env: Vec<(String, String)>,
    scratch: Scratch,
    _proxy: Option<Proxy>,
}

async fn start_proxy(config: &SandboxConfig, scratch: &Path) -> std::io::Result<Option<Proxy>> {
    if config.policy == Policy::FullAccess || config.network != NetworkMode::Proxy {
        return Ok(None);
    }
    let domains = config.allowed_domains.clone();
    let decide: Decide = Arc::new(move |host| {
        let allowed = cyber_sandbox::matches_domain(&host, &domains);
        Box::pin(async move { allowed })
    });
    #[cfg(unix)]
    if cyber_sandbox::proxy_uses_unix_socket() {
        return Proxy::start_unix(decide, &scratch.join("proxy.sock"))
            .await
            .map(Some);
    }
    let _ = scratch;
    Proxy::start(decide).await.map(Some)
}

struct Scratch {
    path: PathBuf,
    cleanup: bool,
}

impl Scratch {
    fn new(parent: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!("hook-{}", ulid::Ulid::new()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self {
            path,
            cleanup: true,
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scratch;

    #[test]
    fn scratch_creation_and_cleanup_preserve_unknown_owner_evidence() {
        let parent = tempfile::tempdir().unwrap();
        let ordinary = Scratch::new(parent.path()).unwrap();
        let mut unknown = Scratch::new(parent.path()).unwrap();
        assert_ne!(ordinary.path, unknown.path);
        assert_eq!(ordinary.path.parent(), Some(parent.path()));
        assert_eq!(unknown.path.parent(), Some(parent.path()));
        assert!(ordinary.path.is_dir() && unknown.path.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&ordinary.path, &unknown.path] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        let ordinary_path = ordinary.path.clone();
        let unknown_path = unknown.path.clone();
        unknown.cleanup = false;
        drop(ordinary);
        drop(unknown);
        assert!(!ordinary_path.exists());
        assert!(unknown_path.is_dir());
    }
}
