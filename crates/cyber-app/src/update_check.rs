//! The background update check (`installation-upgrade` → Background update policy,
//! Minimum supported version): 5 seconds after the server starts and then every 24
//! hours, unless `policy::should_check` turns it off. A newer release is announced with
//! `installation.update_available.1`; a patch with `autoupdate = true` on a `script`,
//! `brew` or `npm` install that the binary's path shows is installed silently and
//! announced with `installation.updated.1`, or announced as available when that fails. The last result
//! is what `GET /service/update` answers. The server never restarts itself: the existing
//! stale-build rule replaces it once it is idle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cyber_core::config::AutoUpdate;
use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_core::version::{Channel, ReleaseSource};
use cyber_install::manifest::{self, Latest, Lookup};
use cyber_install::method::{self, Detected, InstallMethod, Probe};
use cyber_install::policy::{self, Bump, CheckInputs, Decision};
use cyber_install::upgrade::{Plan, SelfUpdate};
use cyber_server::runtime::{InstallationNotice, Runtime, UpdateStatus};
use cyber_tools::ConfigFn;
use serde_json::json;

use crate::App;

pub const FIRST_CHECK: Duration = Duration::from_secs(5);
pub const INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// Connect and per-read timeout of a silent upgrade's downloads, as `cyber upgrade`.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// What the check compares and where it looks; tests pin the version, channel,
/// environment, binary and cadence.
#[derive(Clone)]
pub struct UpdateCheckOptions {
    /// The running version.
    pub current: String,
    /// The build's channel; `dev` never checks.
    pub channel: Channel,
    /// The target triple whose archive a silent upgrade installs.
    pub target: String,
    /// The binary a silent upgrade replaces; `None` is the running executable, resolved
    /// once when the checks start.
    pub exe: Option<PathBuf>,
    /// `CI`, `CYBER_OFFLINE`, `CYBER_DISABLE_AUTOUPDATE`, the install-method variables
    /// and, in debug builds, the release test hook.
    pub env: Arc<dyn EnvSource + Send + Sync>,
    pub first: Duration,
    pub interval: Duration,
}

impl Default for UpdateCheckOptions {
    fn default() -> Self {
        let build = cyber_core::version::build_info();
        Self {
            current: build.version.into(),
            channel: build.channel,
            target: build.target.into(),
            exe: None,
            env: Arc::new(ProcessEnv),
            first: FIRST_CHECK,
            interval: INTERVAL,
        }
    }
}

/// The status before any check: the running build and the cached manifest's minimum
/// supported version, read without the network.
pub fn initial_status(cache: &Path, current: &str, channel: Channel) -> UpdateStatus {
    let mut status = UpdateStatus::initial(current, manifest::lookup_channel(channel).as_str());
    if let Some((_, cached)) = manifest::read_cache(cache, channel) {
        status.unsupported = policy::unsupported(current, cached.min_supported.as_deref());
        status.min_supported = cached.min_supported;
    }
    status
}

/// Start the checks of `app`'s server; the caller aborts the task on shutdown.
pub(crate) fn spawn(app: &App, mut opts: UpdateCheckOptions) -> tokio::task::JoinHandle<()> {
    // Resolved now, before an upgrade renames the running binary: on Linux a later
    // `current_exe` follows the rename to `cyber.old` (or `cyber.old (deleted)`).
    if opts.exe.is_none() {
        opts.exe = std::env::current_exe().ok();
    }
    let checker = Checker {
        runtime: app.runtime.clone(),
        config: Arc::clone(&app.config),
        directory: app.state.options.default_directory.clone(),
        cache: app.paths.cache.clone(),
        opts,
        upgraded: AtomicBool::new(false),
    };
    checker.runtime.set_update_status(initial_status(
        &checker.cache,
        &checker.opts.current,
        checker.opts.channel,
    ));
    tokio::spawn(async move {
        tokio::time::sleep(checker.opts.first).await;
        loop {
            checker.check().await;
            tokio::time::sleep(checker.opts.interval).await;
        }
    })
}

struct Checker {
    runtime: Runtime,
    config: Arc<ConfigFn>,
    /// The server's default Location, whose resolved config holds `autoupdate`.
    directory: PathBuf,
    cache: PathBuf,
    opts: UpdateCheckOptions,
    /// A silent upgrade replaced the binary; the running build no longer matches the
    /// disk, so later releases are only announced until a restart.
    upgraded: AtomicBool,
}

impl Checker {
    async fn check(&self) {
        let Some(autoupdate) = self.enabled() else {
            return;
        };
        match self.latest().await {
            Ok((source, latest)) => self.apply(autoupdate, &source, latest).await,
            Err(reason) => {
                cyber_core::log::warn("update", "update check failed", json!({ "reason": reason }))
            }
        }
    }

    /// The `autoupdate` in effect when the check is on; config is read at every check,
    /// so an edit applies without a restart. A config that does not load skips the
    /// check: its defaults would drop an org policy that disables it.
    fn enabled(&self) -> Option<AutoUpdate> {
        let (value, sources) = match (self.config)(&self.directory) {
            Ok(resolved) => resolved,
            Err(reason) => {
                cyber_core::log::warn(
                    "update",
                    "update check skipped: the configuration does not load",
                    json!({ "reason": reason }),
                );
                return None;
            }
        };
        let inputs = CheckInputs::new(
            AutoUpdate::from_config(&value).unwrap_or_default(),
            AutoUpdate::managed(&value, &sources),
            self.opts.channel,
            &*self.opts.env,
        );
        policy::should_check(&inputs).then(|| inputs.effective())
    }

    async fn latest(&self) -> Result<(ReleaseSource, Latest), String> {
        let source = ReleaseSource::resolve(&*self.opts.env)?;
        let mut lookup = Lookup::new(&source, &self.cache, &*self.opts.env);
        lookup.client = "server";
        let latest = lookup
            .latest(self.opts.channel)
            .await
            .map_err(|e| e.to_string())?;
        Ok((source, latest))
    }

    async fn apply(&self, autoupdate: AutoUpdate, source: &ReleaseSource, latest: Latest) {
        let detected = self.method().await;
        let method = detected.method;
        let manifest = latest.manifest;
        let current = self.opts.current.as_str();
        let bump = policy::bump(current, &manifest.version);
        let previous = self.runtime.update_status();
        let mut status = UpdateStatus {
            current: current.into(),
            channel: latest.channel.as_str().into(),
            method: Some(method.as_str().into()),
            latest: Some(manifest.version.clone()),
            kind: bump.map(|bump| bump.as_str().into()),
            notes_url: manifest.notes_url,
            unsupported: policy::unsupported(current, manifest.min_supported.as_deref()),
            min_supported: manifest.min_supported,
            updated: false,
            checked_at_ms: Some(manifest::now_ms()),
        };
        // A version already installed silently is not downloaded or announced again; a
        // newer one after that is only announced.
        let installed = previous.is_some_and(|p| p.updated && p.latest == status.latest);
        let announce = match decide(autoupdate, detected, bump) {
            Decision::Current => false,
            Decision::Silent if installed => {
                status.updated = true;
                false
            }
            Decision::Silent if self.upgraded.load(Ordering::Relaxed) => true,
            Decision::Silent => {
                status.updated = self.upgrade(source, method, &manifest.version).await;
                self.upgraded.store(status.updated, Ordering::Relaxed);
                true
            }
            Decision::Notify(_) => true,
        };
        self.runtime.set_update_status(status.clone());
        if let Some(notice) = announce.then(|| notice(&status)).flatten() {
            self.runtime.publish_installation(notice, status.updated);
        }
    }

    /// How the server binary was installed; listings may take seconds, so off the
    /// async workers.
    async fn method(&self) -> Detected {
        let env = Arc::clone(&self.opts.env);
        let exe = self.opts.exe.clone();
        tokio::task::spawn_blocking(move || {
            let home = cyber_core::paths::home_dir(&*env)?;
            let mut probe = Probe::current(&*env, &home).ok()?;
            probe.exe = exe?;
            Some(method::detect_origin(&probe))
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(Detected {
            method: InstallMethod::Unknown,
            from_path: false,
        })
    }

    /// The unattended upgrade `cyber upgrade` would run; whether it installed `version`.
    async fn upgrade(&self, source: &ReleaseSource, method: InstallMethod, version: &str) -> bool {
        let result = match Plan::new(method, &self.opts.current, version) {
            Plan::Replace => self.replace(source, version).await,
            Plan::Manager { argv } => {
                let env = Arc::clone(&self.opts.env);
                tokio::task::spawn_blocking(move || {
                    cyber_install::upgrade::run_manager(&argv, &*env)
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
            }
            Plan::Current | Plan::Manual { .. } => Err(format!(
                "{} installs are not upgraded unattended",
                method.as_str()
            )),
        };
        let fields = json!({ "version": version, "method": method.as_str() });
        match result {
            Ok(()) => {
                cyber_core::log::info("update", "upgraded silently", fields);
                true
            }
            Err(reason) => {
                cyber_core::log::warn(
                    "update",
                    &format!("silent upgrade failed, announcing it instead: {reason}"),
                    fields,
                );
                false
            }
        }
    }

    /// Verify and replace the binary, as `cyber upgrade` does for `script`. The
    /// download's future is not `Send`, so it runs on a blocking thread of its own.
    async fn replace(&self, source: &ReleaseSource, version: &str) -> Result<(), String> {
        let exe = self
            .opts
            .exe
            .clone()
            .ok_or("the running executable is unknown")?;
        let (source, version) = (source.clone(), version.to_owned());
        let target = self.opts.target.clone();
        tokio::task::spawn_blocking(move || {
            let update = SelfUpdate {
                source: &source,
                version: &version,
                target: &target,
                exe: &exe,
                timeout: DOWNLOAD_TIMEOUT,
            };
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(update.run())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// [`policy::decide`] for the detected method, except that a method known from a
/// manager's listing alone is only announced: that package may be another install than
/// the running binary, which an unattended upgrade would then leave unchanged.
fn decide(autoupdate: AutoUpdate, detected: Detected, bump: Option<Bump>) -> Decision {
    match policy::decide(autoupdate, detected.method, bump) {
        Decision::Silent if !detected.from_path => bump.map_or(Decision::Current, Decision::Notify),
        decision => decision,
    }
}

/// The event data of a check that found a newer version.
fn notice(status: &UpdateStatus) -> Option<InstallationNotice> {
    Some(InstallationNotice {
        current: status.current.clone(),
        latest: status.latest.clone()?,
        kind: status.kind.clone()?,
        notes_url: status.notes_url.clone(),
    })
}
