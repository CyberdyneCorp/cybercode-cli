use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use cyber_core::{
    config::{self, LoadRequest, LspSettings, Resolved},
    intelligence::{DetectedServer, ExecutableSearch, detect_servers},
    paths::Paths,
};
use cyber_sandbox::{
    Launch, NetworkMode, Policy, SandboxConfig,
    proxy::{Decide, Proxy},
};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{AuthorizedProcess, LaunchError, LaunchFn, LaunchRequest, LspError, ResourceLease};
use crate::HookCommandProcess;

pub type CheckoutClaim = Arc<
    dyn Fn(
            Vec<cyber_core::worktrees::Managed>,
            String,
            CancellationToken,
        )
            -> BoxFuture<'static, Result<Vec<cyber_core::worktrees::CheckoutLease>, LspError>>
        + Send
        + Sync,
>;

/// Immutable user-selected loader inputs for one Location service generation.
#[derive(Clone)]
pub struct LaunchOptions {
    pub checkout_claim: Option<CheckoutClaim>,
    pub paths: Paths,
    pub home: PathBuf,
    pub environment: HashMap<String, String>,
    pub profile: Option<String>,
    pub overrides: Vec<String>,
    pub flags: Value,
    pub sandbox_policy: Option<String>,
    pub helper: Option<PathBuf>,
    pub credential_env_names: Vec<String>,
}

pub struct LocalLauncher {
    location: PathBuf,
    checkouts: Vec<cyber_core::worktrees::Managed>,
    options: LaunchOptions,
}

impl LocalLauncher {
    pub fn pool(self: Arc<Self>) -> Result<super::Pool, LaunchError> {
        let selected = Arc::new(self.resolved()?);
        let servers = self.detected(&selected)?;
        let pool = super::Pool::new(&self.location, servers, self.clone().callback()).map_err(
            |error| LaunchError {
                error,
                acknowledged: true,
            },
        )?;
        let admission: super::AdmissionFn = Arc::new(move |request| {
            let launcher = self.clone();
            let selected = selected.clone();
            Box::pin(async move {
                let fresh = launcher
                    .observe(&request)
                    .await
                    .map_err(|error| error.error)?;
                if fresh.value != selected.value
                    || fresh.sources != selected.sources
                    || fresh.trust.digest != selected.trust.digest
                    || fresh.trust.trusted != selected.trust.trusted
                {
                    return Err(LspError::Protocol(
                        "language service generation authority changed",
                    ));
                }
                Ok(())
            })
        });
        pool.with_admission(admission).map_err(|error| LaunchError {
            error,
            acknowledged: true,
        })
    }
    pub fn new(location: &Path, options: LaunchOptions) -> Result<Self, LaunchError> {
        let location = location
            .canonicalize()
            .map_err(|_| refused("Location unavailable"))?;
        if !location.is_dir() {
            return Err(refused("Location is not a directory"));
        }
        let checkouts = checkout_records(&location)?;
        Ok(Self {
            location,
            options,
            checkouts,
        })
    }

    pub fn servers(&self) -> Result<Vec<DetectedServer>, LaunchError> {
        let resolved = self.resolved()?;
        self.detected(&resolved)
    }

    pub fn callback(self: Arc<Self>) -> LaunchFn {
        Arc::new(move |request, cancel| {
            let launcher = self.clone();
            Box::pin(async move { launcher.launch(request, cancel).await })
        })
    }

    fn resolved(&self) -> Result<Resolved, LaunchError> {
        config::load(&LoadRequest {
            location: &self.location,
            paths: &self.options.paths,
            env: &self.options.environment,
            home: &self.options.home,
            profile: self.options.profile.as_deref(),
            overrides: &self.options.overrides,
            flags: self.options.flags.clone(),
        })
        .map_err(|_| refused("trusted configuration unavailable"))
    }

    fn detected(&self, resolved: &Resolved) -> Result<Vec<DetectedServer>, LaunchError> {
        let settings = LspSettings::from_config(&resolved.value)
            .map_err(|_| refused("LSP settings unavailable"))?;
        let search = ExecutableSearch::new(
            &self.location,
            &self.options.paths.cache,
            &self.options.environment,
        );
        Ok(detect_servers(&settings, &search))
    }

    fn admit(&self, request: &LaunchRequest) -> Result<Resolved, LaunchError> {
        let location = request
            .location
            .canonicalize()
            .map_err(|_| refused("Location unavailable"))?;
        let root = request
            .root
            .canonicalize()
            .map_err(|_| refused("root unavailable"))?;
        if location != self.location
            || root != request.root
            || !root.starts_with(&location)
            || !root.is_dir()
        {
            return Err(refused("root is outside its Location"));
        }
        if checkout_records(&location)? != self.checkouts
            || checkout_records(&root)? != request.checkouts
        {
            return Err(refused("managed checkout creation changed"));
        }
        let resolved = self.resolved()?;
        let selected = self
            .detected(&resolved)?
            .into_iter()
            .find(|s| s.definition.id == request.server.definition.id)
            .ok_or_else(|| refused("server is absent from trusted configuration"))?;
        if !selected.enabled
            || !selected.installed
            || selected.definition != request.server.definition
            || selected.executable != request.server.executable
        {
            return Err(refused("server configuration changed or is unavailable"));
        }
        Ok(resolved)
    }

    async fn observe(self: &Arc<Self>, request: &LaunchRequest) -> Result<Resolved, LaunchError> {
        let launcher = self.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || launcher.admit(&request))
            .await
            .map_err(|_| refused("configuration observation failed"))?
    }

    async fn launch(
        self: &Arc<Self>,
        request: LaunchRequest,
        cancel: CancellationToken,
    ) -> Result<AuthorizedProcess, LaunchError> {
        check_cancel(&cancel)?;
        let resolved = self.observe(&request).await?;
        check_cancel(&cancel)?;
        let mut pins = self.claim_checkouts(&request, cancel.clone()).await?;
        check_cancel(&cancel).map_err(|mut error| {
            error.acknowledged = pins.settle();
            error
        })?;
        let mut resources = match self.prepare(&resolved).await {
            Ok(resources) => resources,
            Err(mut error) => {
                error.acknowledged = pins.settle();
                return Err(error);
            }
        };
        resources.pins = pins;
        resources.document_root = request.root.clone();
        let prepared = self
            .prepared(&request, &resolved, &resources, &cancel)
            .await;
        let (wrapped, environment) = match prepared {
            Ok(prepared) => prepared,
            Err(mut error) => {
                error.acknowledged = resources.close().await;
                return Err(error);
            }
        };
        if cancel.is_cancelled() {
            let acknowledged = resources.close().await;
            return Err(LaunchError {
                error: LspError::Protocol("launch cancelled"),
                acknowledged,
            });
        }
        // Never drop a started native-spawn future: it owns platform startup settlement.
        resources.pins_safe = false;
        let process = HookCommandProcess::spawn_with_stdin(
            &wrapped.program,
            &wrapped.args,
            self.options.helper.as_deref(),
            |command| {
                command
                    .env_clear()
                    .envs(&environment)
                    .current_dir(&request.root)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            },
        )
        .await;
        match process {
            Ok(process) => {
                resources.pins_safe = true;
                Ok(AuthorizedProcess {
                    process,
                    keepalive: Box::new(resources),
                })
            }
            Err(_) => {
                let _ = resources.close().await;
                Err(LaunchError {
                    error: LspError::Protocol("native launch failed; completion is unverified"),
                    acknowledged: false,
                })
            }
        }
    }

    async fn prepared(
        self: &Arc<Self>,
        request: &LaunchRequest,
        resolved: &Resolved,
        resources: &Resources,
        cancel: &CancellationToken,
    ) -> Result<(cyber_sandbox::Wrapped, BTreeMap<String, String>), LaunchError> {
        check_cancel(cancel)?;
        let fresh = self.observe(request).await?;
        check_cancel(cancel)?;
        if fresh.value != resolved.value
            || fresh.sources != resolved.sources
            || fresh.trust.digest != resolved.trust.digest
            || fresh.trust.trusted != resolved.trust.trusted
        {
            return Err(refused("configuration changed during launch"));
        }
        let writable = std::iter::once(self.location.clone())
            .chain(std::iter::once(resources.scratch.clone()))
            .chain(resources.config.extra_writable.iter().cloned())
            .collect::<Vec<_>>();
        let launch = Launch {
            config: resources.config.clone(),
            read_only: writable
                .iter()
                .flat_map(|p| cyber_sandbox::protected_in(p))
                .collect(),
            unreadable: resources.config.unreadable(&self.options.home),
            writable,
            proxy: resources.proxy.as_ref().map(|p| p.endpoint.clone()),
            helper: self.options.helper.clone(),
        };
        let program = request
            .server
            .executable
            .as_ref()
            .and_then(|p| p.to_str())
            .ok_or_else(|| refused("executable unavailable"))?;
        let wrapped =
            cyber_sandbox::wrap(&launch, program, &request.server.definition.command[1..])
                .map_err(|_| refused("sandbox enforcement unavailable"))?;
        let environment = self.environment(
            resolved,
            resources,
            &wrapped.env,
            &request.server.definition.env,
        );
        Ok((wrapped, environment))
    }

    async fn claim_checkouts(
        &self,
        request: &LaunchRequest,
        cancel: CancellationToken,
    ) -> Result<Pins, LaunchError> {
        let mut expected = self.checkouts.clone();
        for checkout in &request.checkouts {
            if !expected.contains(checkout) {
                expected.push(checkout.clone());
            }
        }
        claim_pins(&self.options.checkout_claim, expected, cancel).await
    }

    async fn prepare(&self, resolved: &Resolved) -> Result<Resources, LaunchError> {
        let config = SandboxConfig::resolve(
            &resolved.value,
            &resolved.sources,
            self.options.sandbox_policy.as_deref(),
            &self.options.home,
        );
        let scratch = scratch(&self.options.paths.tmp)?;
        let proxy = if config.policy != Policy::FullAccess && config.network == NetworkMode::Proxy {
            let domains = config.allowed_domains.clone();
            let decide: Decide = Arc::new(move |host| {
                let allowed = cyber_sandbox::matches_domain(&host, &domains);
                Box::pin(async move { allowed })
            });
            #[cfg(unix)]
            let proxy = if cyber_sandbox::proxy_uses_unix_socket() {
                Proxy::start_unix(decide, &scratch.join("proxy.sock")).await
            } else {
                Proxy::start(decide).await
            };
            #[cfg(not(unix))]
            let proxy = Proxy::start(decide).await;
            Some(proxy.map_err(|_| refused("allowlist proxy unavailable"))?)
        } else {
            None
        };
        Ok(Resources {
            checkout_claim: self.options.checkout_claim.clone(),
            document_root: self.location.clone(),
            pins: Pins::default(),
            pins_safe: true,
            config,
            scratch,
            proxy,
            shutdown: None,
            acknowledged: None,
        })
    }

    fn environment(
        &self,
        resolved: &Resolved,
        resources: &Resources,
        wrapped: &BTreeMap<String, String>,
        configured: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut credentials = crate::sandboxing::credential_env_names(&resolved.value);
        credentials.extend(self.options.credential_env_names.clone());
        let mut environment = if resources.config.policy == Policy::FullAccess {
            self.options.environment.clone().into_iter().collect()
        } else {
            cyber_sandbox::mask_env(
                self.options.environment.clone().into_iter(),
                &resources.config.env_allow,
                &credentials,
            )
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        };
        environment.extend(configured.clone());
        environment.retain(|key, _| !reserved(key));
        environment.extend(wrapped.clone());
        for key in ["TMPDIR", "TEMP", "TMP"] {
            environment.insert(key.into(), resources.scratch.to_string_lossy().into());
        }
        environment
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), LaunchError> {
    if cancel.is_cancelled() {
        return Err(refused("launch cancelled"));
    }
    Ok(())
}

fn refused(message: &'static str) -> LaunchError {
    LaunchError {
        error: LspError::Protocol(message),
        acknowledged: true,
    }
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

fn scratch(parent: &Path) -> Result<PathBuf, LaunchError> {
    std::fs::create_dir_all(parent).map_err(|_| refused("scratch parent unavailable"))?;
    let path = parent.join(format!("lsp-{}", ulid::Ulid::new()));
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&path)
        .map_err(|_| refused("scratch creation failed"))?;
    path.canonicalize()
        .map_err(|_| refused("scratch unavailable"))
}

struct Resources {
    checkout_claim: Option<CheckoutClaim>,
    document_root: PathBuf,
    pins: Pins,
    pins_safe: bool,
    config: SandboxConfig,
    scratch: PathBuf,
    proxy: Option<Proxy>,
    shutdown: Option<JoinHandle<std::io::Result<()>>>,
    acknowledged: Option<bool>,
}

impl Resources {
    async fn admit_scope(
        &mut self,
        path: PathBuf,
        checkouts: Vec<cyber_core::worktrees::Managed>,
        cancel: CancellationToken,
        removed: bool,
    ) -> Result<(), LspError> {
        if self.acknowledged.is_some() || !path.starts_with(&self.document_root) {
            return Err(LspError::Protocol("document root unavailable"));
        }
        super::documents::verify_scope(path.clone(), checkouts.clone(), removed).await?;
        let missing = self.pins.missing(&path, &checkouts)?;
        let pins = claim_pins(&self.checkout_claim, missing, cancel.clone())
            .await
            .map_err(|error| error.error)?;
        self.pins.checkouts.extend(pins.checkouts);
        self.pins.leases.extend(pins.leases);
        super::documents::verify_scope(path, checkouts, removed).await?;
        check_cancel(&cancel).map_err(|error| error.error)
    }
}

impl ResourceLease for Resources {
    fn admit_document(
        &mut self,
        path: PathBuf,
        checkouts: Vec<cyber_core::worktrees::Managed>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), LspError>> {
        Box::pin(self.admit_scope(path, checkouts, cancel, false))
    }
    fn admit_removed(
        &mut self,
        path: PathBuf,
        checkouts: Vec<cyber_core::worktrees::Managed>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), LspError>> {
        Box::pin(self.admit_scope(path, checkouts, cancel, true))
    }

    fn close(&mut self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            if let Some(acknowledged) = self.acknowledged {
                return acknowledged;
            }
            if let Some(proxy) = self.proxy.take() {
                self.shutdown = Some(tokio::spawn(proxy.shutdown()));
            }
            let acknowledged = match self.shutdown.as_mut() {
                Some(task) => matches!(task.await, Ok(Ok(()))),
                None => true,
            };
            self.shutdown.take();
            let acknowledged = acknowledged && self.pins_safe && self.pins.settle();
            self.acknowledged = Some(acknowledged);
            acknowledged
        })
    }
}

#[derive(Default)]
struct Pins {
    checkouts: Vec<cyber_core::worktrees::Managed>,
    leases: Vec<cyber_core::worktrees::CheckoutLease>,
    acknowledged: Option<bool>,
}
impl Pins {
    fn missing(
        &self,
        path: &Path,
        expected: &[cyber_core::worktrees::Managed],
    ) -> Result<Vec<cyber_core::worktrees::Managed>, LspError> {
        for previous in &self.checkouts {
            if path.starts_with(&previous.path) && !expected.contains(previous) {
                return Err(LspError::Protocol("document checkout creation changed"));
            }
        }
        let missing: Vec<_> = expected
            .iter()
            .filter(|checkout| !self.checkouts.contains(checkout))
            .cloned()
            .collect();
        if self.checkouts.len() + missing.len() > 128 {
            return Err(LspError::Protocol("document checkout capacity exhausted"));
        }
        Ok(missing)
    }
    fn settle(&mut self) -> bool {
        if let Some(acknowledged) = self.acknowledged {
            return acknowledged;
        }
        let result = std::mem::take(&mut self.leases)
            .into_iter()
            .map(|lease| lease.settle_retained())
            .collect::<std::io::Result<Vec<_>>>();
        let acknowledged = result.is_ok();
        if let Ok(retained) = result {
            self.leases = retained;
        }
        self.acknowledged = Some(acknowledged);
        acknowledged
    }
}

async fn claim_pins(
    claim: &Option<CheckoutClaim>,
    expected: Vec<cyber_core::worktrees::Managed>,
    cancel: CancellationToken,
) -> Result<Pins, LaunchError> {
    if expected.len() > 128 {
        return Err(refused("document checkout capacity exhausted"));
    }
    if expected.is_empty() {
        return Ok(Pins::default());
    }
    let claim = claim
        .as_ref()
        .ok_or_else(|| refused("managed checkout claim unavailable"))?;
    let owner = cyber_core::ids::new_id("lsp");
    let leases = claim(expected.clone(), owner.clone(), cancel)
        .await
        .map_err(|error| LaunchError {
            error,
            acknowledged: false,
        })?;
    let observed: std::collections::BTreeSet<_> =
        leases.iter().map(|lease| lease.worktree_id()).collect();
    let wanted: std::collections::BTreeSet<_> = expected
        .iter()
        .map(|checkout| checkout.id.as_str())
        .collect();
    if observed != wanted
        || leases.len() != expected.len()
        || leases.iter().any(|lease| lease.owner_id() != owner)
    {
        return Err(LaunchError {
            error: LspError::Protocol("managed checkout claims do not match launch"),
            acknowledged: false,
        });
    }
    Ok(Pins {
        checkouts: expected,
        leases,
        acknowledged: None,
    })
}

fn checkout_records(directory: &Path) -> Result<Vec<cyber_core::worktrees::Managed>, LaunchError> {
    cyber_core::worktrees::Repository::managed_locations_at(directory)
        .map(|locations| locations.into_iter().map(|(_, managed)| managed).collect())
        .map_err(|_| refused("managed checkout identity unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::oneshot,
    };

    fn resources(root: &Path) -> Resources {
        Resources {
            checkout_claim: None,
            document_root: root.into(),
            pins: Pins::default(),
            pins_safe: true,
            config: SandboxConfig::resolve(&serde_json::json!({}), &BTreeMap::new(), None, root),
            scratch: root.into(),
            proxy: None,
            shutdown: None,
            acknowledged: None,
        }
    }

    #[tokio::test]
    async fn interrupted_resource_wait_retains_the_same_join_handle() {
        let root = tempfile::tempdir().unwrap();
        let mut resources = resources(root.path());
        let (complete, receive) = oneshot::channel();
        resources.shutdown = Some(tokio::spawn(async move {
            receive.await.map_err(std::io::Error::other)
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), resources.close())
                .await
                .is_err()
        );
        assert!(resources.shutdown.is_some());
        assert!(resources.acknowledged.is_none());
        complete.send(()).unwrap();
        assert!(resources.close().await);
        assert!(resources.shutdown.is_none());
        assert!(resources.close().await);
    }

    #[tokio::test]
    async fn failed_resource_join_cannot_become_acknowledged_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut resources = resources(root.path());
        resources.shutdown = Some(tokio::spawn(async {
            Err(std::io::Error::other("fixture"))
        }));
        assert!(!resources.close().await);
        assert!(!resources.close().await);
    }

    #[tokio::test]
    async fn proxy_lease_closes_real_accepted_transports_before_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        let mut resources = resources(root.path());
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        let proxy = Proxy::start(Arc::new(|_| Box::pin(async { true })))
            .await
            .unwrap();
        let cyber_sandbox::proxy::Endpoint::Tcp(port) = proxy.endpoint else {
            panic!("unexpected endpoint")
        };
        let mut peer = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        peer.write_all(format!("CONNECT 127.0.0.1:{upstream_port} HTTP/1.1\r\nHost: 127.0.0.1:{upstream_port}\r\n\r\n").as_bytes()).await.unwrap();
        let (mut accepted, _) = tokio::time::timeout(Duration::from_secs(1), upstream.accept())
            .await
            .unwrap()
            .unwrap();
        let expected = b"HTTP/1.1 200 Connection Established\r\n\r\n";
        let mut response = vec![0; expected.len()];
        tokio::time::timeout(Duration::from_secs(1), peer.read_exact(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response, expected);
        resources.proxy = Some(proxy);
        assert!(resources.close().await);
        let mut bytes = [0; 1];
        let end = tokio::time::timeout(Duration::from_secs(1), peer.read(&mut bytes))
            .await
            .unwrap();
        assert!(end.is_err() || end.unwrap() == 0);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), accepted.read(&mut bytes))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
