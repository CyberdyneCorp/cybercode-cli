//! Which installed plugins load for one Location, and why the others do not.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;

use super::manifest::Manifest;
use super::registry::{Entry, Locations, PluginScope, Registry, Source};
use crate::config::ManagedPolicy;
use crate::trust::TrustStore;

/// Why a listed plugin is or is not loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Active,
    /// `--pure` or `CYBER_PURE=1`.
    Pure,
    /// A managed `policy.plugins.disabled` entry, or a malformed managed policy.
    Policy,
    Disabled,
    NotInstalled,
    Invalid,
    /// The tree no longer matches the installed digest.
    Changed,
    Untrusted,
    /// Another scope supplies the plugin of this name.
    Shadowed,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PluginStatus {
    pub name: String,
    pub scope: PluginScope,
    pub state: PluginState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Signatures are not verified yet, so every plugin is listed as `unsigned`.
    pub unsigned: bool,
    /// The host process of a loaded plugin that runs code (`main`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostStatus>,
}

/// Where a plugin host process is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    /// No Session has opened this Location yet.
    NotStarted,
    Starting,
    Running,
    /// Waiting for its backoff after a crash.
    Restarting,
    /// Crashed too often; off for the rest of the process.
    Disabled,
    /// Cannot start: a protocol mismatch, invalid settings, a missing runtime or sandbox.
    Failed,
}

/// A plugin host's runtime status (`plugins-marketplace` → Plugin host process).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct HostStatus {
    pub state: HostState,
    pub runtime: super::Runtime,
    pub capabilities: super::Capabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Tools the host contributed in `initialize`.
    pub tools: Vec<String>,
    /// Crashes within the last 10 minutes.
    pub crashes: usize,
}

/// Every listed plugin of a Location and whether pure mode is on (`GET /plugins`).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PluginList {
    pub plugins: Vec<PluginStatus>,
    pub pure: bool,
}

#[derive(Debug, Clone)]
pub struct ActivePlugin {
    pub name: String,
    pub scope: PluginScope,
    pub root: PathBuf,
    pub manifest: Manifest,
    pub digest: String,
}

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub active: Vec<ActivePlugin>,
    pub statuses: Vec<PluginStatus>,
    pub warnings: Vec<String>,
}

/// Every scope's entries, most specific scope first; the first loadable entry of a name
/// wins and later ones are `shadowed`. Without `project` (`CYBER_DISABLE_PROJECT_CONFIG`)
/// the project and local scope files are not read.
pub fn resolve(
    locations: &Locations,
    managed: &ManagedPolicy,
    pure: bool,
    project: bool,
) -> Resolution {
    let store = TrustStore::new(&locations.trust_file);
    let mut out = Resolution::default();
    let mut taken = BTreeSet::new();
    let scopes = PluginScope::ALL
        .into_iter()
        .filter(|scope| project || *scope == PluginScope::User);
    for scope in scopes {
        let registry = match Registry::read(locations, scope) {
            Ok(registry) => registry,
            Err(error) => {
                out.warnings.push(format!("plugins ignored: {error}"));
                continue;
            }
        };
        for (name, entry) in &registry.entries {
            let mut status = status(name, scope, entry, locations);
            let checked = if pure {
                Err((PluginState::Pure, "disabled by --pure".into()))
            } else if taken.contains(name) {
                Err((PluginState::Shadowed, "another scope supplies it".into()))
            } else {
                check(name, scope, entry, locations, managed, &store)
            };
            match checked {
                Ok(plugin) => {
                    status.host = plugin.manifest.host.as_ref().map(|host| HostStatus {
                        state: HostState::NotStarted,
                        runtime: host.runtime,
                        capabilities: plugin.manifest.capabilities.clone(),
                        error: None,
                        tools: Vec::new(),
                        crashes: 0,
                    });
                    taken.insert(name.clone());
                    out.active.push(plugin);
                }
                Err((state, reason)) => {
                    if matches!(state, PluginState::Changed | PluginState::Invalid) {
                        out.warnings
                            .push(format!("plugin {name} not loaded: {reason}"));
                    }
                    status.state = state;
                    status.reason = Some(reason);
                }
            }
            out.statuses.push(status);
        }
    }
    out
}

fn status(name: &str, scope: PluginScope, entry: &Entry, locations: &Locations) -> PluginStatus {
    let installed = entry.installed.as_ref();
    PluginStatus {
        name: name.into(),
        scope,
        state: PluginState::Active,
        reason: None,
        source: entry.source.clone(),
        version: installed.map(|lock| lock.version.clone()),
        commit: installed.and_then(|lock| lock.commit.clone()),
        digest: installed.map(|lock| lock.digest.clone()),
        root: locations.root(scope, name, entry),
        unsigned: true,
        host: None,
    }
}

type Refusal = (PluginState, String);

fn check(
    name: &str,
    scope: PluginScope,
    entry: &Entry,
    locations: &Locations,
    managed: &ManagedPolicy,
    store: &TrustStore,
) -> Result<ActivePlugin, Refusal> {
    if let Some(reason) = managed.blocks_plugin(name) {
        return Err((PluginState::Policy, reason));
    }
    if !entry.enabled {
        return Err((PluginState::Disabled, "disabled".into()));
    }
    let not_installed = || {
        (
            PluginState::NotInstalled,
            format!("not installed; run `cyber plugins enable {name}`"),
        )
    };
    let lock = entry.installed.as_ref().ok_or_else(not_installed)?;
    let root = locations
        .root(scope, name, entry)
        .filter(|root| root.is_dir())
        .ok_or_else(not_installed)?;
    let invalid = |reason: String| (PluginState::Invalid, reason);
    let manifest = Manifest::read(&root).map_err(|error| invalid(error.to_string()))?;
    if manifest.name != name || manifest.version != lock.version {
        return Err(invalid(format!(
            "{} declares {}@{}, installed as {name}@{}",
            root.display(),
            manifest.name,
            manifest.version,
            lock.version
        )));
    }
    let digest = super::digest::tree(&root).map_err(invalid)?;
    if digest != lock.digest {
        return Err((
            PluginState::Changed,
            format!("its content changed since install; run `cyber plugins update {name}`"),
        ));
    }
    let approved = locations
        .trust_key(scope)
        .and_then(|key| {
            store.is_plugin_approved(&key, &super::trust_digest(name, &lock.version, &digest))
        })
        .unwrap_or(false);
    if !approved {
        return Err((
            PluginState::Untrusted,
            format!("not trusted; run `cyber plugins enable {name}` to review it"),
        ));
    }
    Ok(ActivePlugin {
        name: name.into(),
        scope,
        root,
        manifest,
        digest,
    })
}
