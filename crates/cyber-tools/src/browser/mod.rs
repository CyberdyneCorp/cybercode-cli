//! Per-Session browser integration (`browser-verification` → Supported optional browser
//! integration, Verification artifacts and lifecycle).
//!
//! With `browser.enabled`, the MCP server `browser.server` names is not a shared Location
//! connection: every Session that opens the Location starts its own process through the
//! durable MCP owner, with `TMPDIR` in a fresh per-Session profile directory
//! (`<data>/browser/profiles/<session_id>/`, locked while it runs). Its tools are offered
//! to that Session only, as `mcp__<server>__<tool>`, through the registered-tool path
//! (schema, hooks, permissions, output budget) plus the network policy for every
//! navigation. The Session's end, or the host's shutdown, stops it and removes the
//! profile. Trust, the per-server digest approval and the managed MCP policy apply as for
//! any MCP server: an unapproved definition never starts.

mod calls;
mod evidence;
pub(crate) mod health;
pub(crate) mod profiles;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_core::browser::BrowserSettings;
use cyber_core::config::{McpServer, McpSettings, Resolved};
use cyber_server::runtime::{RetrySafety, SessionInfo, ToolDef, ToolScope, TurnContext};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::host::BuiltinHost;
use crate::mcp::{DiscoveredTool, LocalLauncher, OwnedLocalServer, authorize_server};

pub(crate) use evidence::{Taken, Visit, record, store_artifact};

/// The Sessions' browsers and the dev server Jobs they own.
#[derive(Default)]
pub(crate) struct Browsers {
    sessions: Mutex<HashMap<String, Arc<SessionBrowser>>>,
    /// Dev server Jobs (`bash` with `ready_url`) by Session: the Session's end stops them.
    dev_servers: Mutex<HashMap<String, Vec<String>>>,
    closed: AtomicBool,
}

/// One Session's browser server.
pub(crate) struct SessionBrowser {
    pub(crate) session_id: String,
    pub(crate) name: String,
    directory: PathBuf,
    digest: String,
    cancel: CancellationToken,
    settled: tokio::sync::watch::Sender<bool>,
    pub(crate) server: tokio::sync::Mutex<Option<OwnedLocalServer>>,
    published: Mutex<Option<(String, Vec<DiscoveredTool>)>>,
    profile: profiles::Profile,
    pub(crate) visit: Mutex<Visit>,
}

impl SessionBrowser {
    fn settle(&self) {
        self.settled.send_replace(true);
    }

    /// The tool `exposed` names on the published connection, with its registration.
    pub(crate) fn tool(&self, exposed: &str) -> Option<(String, DiscoveredTool)> {
        let published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (id, tools) = published.as_ref()?;
        let tool = tools.iter().find(|tool| tool.exposed_name == exposed)?;
        Some((crate::mcp::pool::tool_binding(id, tool), tool.clone()))
    }

    /// Stop the server, then remove its profile. A server whose shutdown is unverified
    /// keeps its profile (and lock) as evidence for `cyber browser cleanup`.
    async fn stop(&self) {
        self.cancel.cancel();
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut server) = self.server.lock().await.take() else {
            self.profile.remove();
            return;
        };
        match server.shutdown_gracefully().await {
            Ok(_) => self.profile.remove(),
            Err(error) => cyber_core::log::error(
                "browser",
                &error.diagnostic,
                json!({"session_id": self.session_id, "acknowledged": error.acknowledged}),
            ),
        }
    }
}

/// The browser server `resolved` selects for `directory`, when browser support is on and
/// the definition is an authorized local server: `(name, digest)`.
fn selected(
    resolved: &Resolved,
    trust: &cyber_core::trust::TrustStore,
    directory: &Path,
) -> Option<(String, String)> {
    let settings = BrowserSettings::from_config(&resolved.value).ok()??;
    let selection = authorize_server(resolved, trust, directory, &settings.server).ok()?;
    // A remote server would be shared between Sessions: no per-Session profile.
    matches!(selection.definition, McpServer::Local { .. })
        .then_some((settings.server, selection.digest))
}

/// The durable connection name of a Session's browser: `<server>-<session ULID>`, within
/// the 48 characters a connection name may have.
fn owner_name(server: &str, session_id: &str) -> String {
    let session = session_id.strip_prefix("ses_").unwrap_or(session_id);
    let session: String = session
        .chars()
        .rev()
        .take(26)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let server: String = server.chars().take(48 - 1 - session.len()).collect();
    format!("{server}-{session}")
}

/// The browser server name of `config`, when browser support is on: the shared MCP pool
/// leaves it to the Sessions.
pub(crate) fn reserved_server(config: &serde_json::Value) -> Option<String> {
    BrowserSettings::from_config(config)
        .ok()
        .flatten()
        .map(|settings| settings.server)
}

impl BuiltinHost {
    fn browser(&self, session_id: &str) -> Option<Arc<SessionBrowser>> {
        self.browsers
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }

    /// Start the Session's browser when its Location enables one and none runs with the
    /// current definition. A changed definition replaces the old process.
    pub(crate) fn start_browser(&self, info: &SessionInfo) {
        let (Some(config), Ok(directory)) = (
            self.hook_config.get(),
            Path::new(&info.directory).canonicalize(),
        ) else {
            return;
        };
        let Ok(resolved) = (config.resolve)(&directory) else {
            return;
        };
        let current = self.browser(&info.id);
        let Some((name, digest)) = selected(&resolved, &config.trust, &directory) else {
            if let Some(old) = current {
                self.replace_browser(&info.id, old);
            }
            return;
        };
        if current.as_ref().is_some_and(|old| {
            old.name == name
                && old.digest == digest
                && old.directory == directory
                && !old.cancel.is_cancelled()
        }) {
            return;
        }
        let (Some(host), Ok(executor)) =
            (self.weak.upgrade(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        if self.browsers.closed.load(Ordering::Acquire) {
            return;
        }
        let profile = profiles::Profile::new(&self.browser_root(), &info.id);
        let (settled, _) = tokio::sync::watch::channel(false);
        let entry = Arc::new(SessionBrowser {
            session_id: info.id.clone(),
            name,
            directory,
            digest,
            cancel: CancellationToken::new(),
            settled,
            server: tokio::sync::Mutex::new(None),
            published: Mutex::new(None),
            profile,
            visit: Mutex::default(),
        });
        self.set_browser(&info.id, Some(entry.clone()));
        let info = info.clone();
        executor.spawn(async move {
            // The replaced process releases the Session's profile before the new one
            // takes it.
            if let Some(old) = current {
                old.stop().await;
            }
            host.connect_browser(&entry, &info).await;
        });
    }

    fn set_browser(&self, session_id: &str, entry: Option<Arc<SessionBrowser>>) {
        let mut sessions = self
            .browsers
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match entry {
            Some(entry) => sessions.insert(session_id.to_string(), entry),
            None => sessions.remove(session_id),
        };
    }

    /// Forget the Session's browser and stop `old` in the background.
    fn replace_browser(&self, session_id: &str, old: Arc<SessionBrowser>) {
        self.set_browser(session_id, None);
        if let Ok(executor) = tokio::runtime::Handle::try_current() {
            executor.spawn(async move { old.stop().await });
        }
    }

    /// `<data>/browser`.
    pub(crate) fn browser_root(&self) -> PathBuf {
        self.opts
            .tool_output_dir
            .parent()
            .unwrap_or(&self.opts.tool_output_dir)
            .join("browser")
    }

    async fn connect_browser(self: Arc<Self>, entry: &Arc<SessionBrowser>, info: &SessionInfo) {
        let result = self.launch_browser(entry, info).await;
        match result {
            Ok(server) if !entry.cancel.is_cancelled() => {
                // Only vetted tools are published, so the rest are neither offered nor
                // callable.
                let tools = server
                    .tools()
                    .iter()
                    .filter(|tool| {
                        cyber_core::browser::EXPOSED_TOOLS.contains(&tool.remote_name.as_str())
                    })
                    .cloned()
                    .collect();
                *entry
                    .published
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) =
                    Some((server.record().id.clone(), tools));
                *entry.server.lock().await = Some(server);
            }
            Ok(mut server) => {
                if server.shutdown_gracefully().await.is_ok() {
                    entry.profile.remove();
                }
            }
            Err(error) => cyber_core::log::error(
                "browser",
                &error,
                json!({"session_id": entry.session_id, "server": entry.name}),
            ),
        }
        entry.settle();
    }

    async fn launch_browser(
        &self,
        entry: &SessionBrowser,
        info: &SessionInfo,
    ) -> Result<OwnedLocalServer, String> {
        let config = self.hook_config.get().ok_or("MCP resolver unavailable")?;
        let resolved = (config.resolve)(&entry.directory)?;
        let temp_dir = entry.profile.create()?;
        let credentials = self
            .opts
            .models
            .as_ref()
            .map(|models| models.credential_env_names())
            .unwrap_or_default();
        let launcher = LocalLauncher {
            resolved: &resolved,
            trust: &config.trust,
            location: &entry.directory,
            home: &self.opts.home,
            temp_dir: &temp_dir,
            helper: self.opts.sandbox_helper.as_deref(),
            credential_env_names: &credentials,
            credentials: self.mcp_credentials(),
        };
        let digest = entry.digest.clone();
        let launched = launcher
            .connect_owned_named(
                &entry.name,
                &owner_name(&entry.name, &entry.session_id),
                self.opts.store.clone(),
                |id| async move {
                    // The definition approved when the Session started is the one launched.
                    let (current, _) = self.browser_definition(&entry.directory)?;
                    if current != digest {
                        return Err("browser server definition changed".to_string());
                    }
                    self.claim_mcp_location(info, id, entry.cancel.child_token())
                        .await
                },
            )
            .await;
        launched.map_err(|error| {
            if error.retry_safe() {
                entry.profile.remove();
            }
            error.diagnostic
        })
    }

    /// The digest and name of the browser server `directory` selects now.
    fn browser_definition(&self, directory: &Path) -> Result<(String, String), String> {
        let config = self.hook_config.get().ok_or("MCP resolver unavailable")?;
        let resolved = (config.resolve)(directory)?;
        selected(&resolved, &config.trust, directory)
            .map(|(name, digest)| (digest, name))
            .ok_or_else(|| "browser server is not authorized".into())
    }

    /// Wait until the Session's browser finished starting (connected or failed).
    pub(crate) async fn wait_browser(&self, session_id: &str, cancel: CancellationToken) {
        let Some(entry) = self.browser(session_id) else {
            return;
        };
        let mut settled = entry.settled.subscribe();
        tokio::select! {
            _ = settled.wait_for(|settled| *settled) => {}
            _ = cancel.cancelled() => {}
            _ = entry.cancel.cancelled() => {}
        }
    }

    /// The live browser of `session_id` when its definition is still the authorized one.
    pub(crate) fn live_browser(
        &self,
        session_id: &str,
        directory: &str,
    ) -> Option<Arc<SessionBrowser>> {
        let entry = self.browser(session_id)?;
        let directory = Path::new(directory).canonicalize().ok()?;
        let (digest, name) = self.browser_definition(&directory).ok()?;
        let live = !entry.cancel.is_cancelled()
            && entry.directory == directory
            && entry.digest == digest
            && entry.name == name;
        live.then_some(entry)
    }

    /// The Session's browser tools, as MCP tools; read-only ones only in `plan`.
    pub(crate) fn browser_definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let Some(entry) = self.live_browser(&turn.session_id, &turn.directory) else {
            return Vec::new();
        };
        let plan = crate::permissions::Mode::parse(&turn.mode) == crate::permissions::Mode::Plan;
        let published = entry
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some((id, tools)) = published.as_ref() else {
            return Vec::new();
        };
        tools
            .iter()
            .filter(|tool| !plan || tool.read_only_hint())
            .map(|tool| ToolDef {
                scope: ToolScope::Mcp,
                deferred: false,
                registration: Some(crate::mcp::pool::tool_binding(id, tool)),
                spec: tool.spec(),
                retry_safety: RetrySafety::Never,
                concurrency_safe: false,
            })
            .collect()
    }

    /// The configured call timeout of the browser server.
    pub(crate) fn browser_timeout(
        &self,
        directory: &str,
        name: &str,
    ) -> Result<std::time::Duration, String> {
        let config = self.hook_config.get().ok_or("MCP resolver unavailable")?;
        let resolved = (config.resolve)(Path::new(directory))?;
        let settings = McpSettings::from_config(&resolved.value)?;
        Ok(std::time::Duration::from_secs(
            settings.call_timeout(name).into(),
        ))
    }

    /// Remember a dev server Job of the Session, which its end stops.
    pub(crate) fn own_dev_server(&self, session_id: &str, job_id: &str) {
        self.browsers
            .dev_servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session_id.to_string())
            .or_default()
            .push(job_id.to_string());
    }

    /// The Session ended: stop its browser, remove its profile and stop its dev servers.
    pub(crate) async fn end_browser_session(&self, session_id: &str) {
        let entry = self
            .browsers
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session_id);
        if let Some(entry) = entry {
            entry.stop().await;
        }
        let jobs = self
            .browsers
            .dev_servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session_id)
            .unwrap_or_default();
        let Some(runtime) = self.runtime() else {
            return;
        };
        for job in jobs {
            if let Err(error) = runtime.cancel_job_quietly(&job).await {
                cyber_core::log::warn(
                    "browser",
                    "dev server stop failed",
                    json!({"job_id": job, "error": error.to_string()}),
                );
            }
        }
    }

    /// Stop every browser; none starts afterwards.
    pub(crate) async fn shutdown_browsers(&self) {
        self.browsers.closed.store(true, Ordering::Release);
        let entries: Vec<_> = self
            .browsers
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        futures::future::join_all(entries.iter().map(|entry| entry.stop())).await;
    }

    /// A browser whose transport failed is stopped; the next step starts a fresh one.
    pub(crate) fn drop_browser(&self, entry: &Arc<SessionBrowser>) {
        let current = self.browser(&entry.session_id);
        if current.is_some_and(|current| Arc::ptr_eq(&current, entry)) {
            self.replace_browser(&entry.session_id, entry.clone());
        }
    }
}
