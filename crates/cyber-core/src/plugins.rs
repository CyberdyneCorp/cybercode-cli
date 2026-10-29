//! Declarative plugins (`plugins-marketplace`): a directory with `cyber-plugin.json`
//! contributing skills, commands, agents, hooks and MCP servers. Installs are recorded
//! per scope; a plugin loads only while it is enabled, installed, unchanged since install,
//! trusted for its exact content digest and allowed by organization policy. `--pure` and
//! `CYBER_PURE=1` turn every plugin off for the process.

mod active;
mod cache_refs;
mod capabilities;
mod components;
mod digest;
pub mod image;
mod install;
mod manage;
mod manifest;
mod npm;
mod registry;
mod user_config;

use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

pub use active::{
    ActivePlugin, HostState, HostStatus, PluginList, PluginState, PluginStatus, Resolution, resolve,
};
pub use capabilities::{Capabilities, HostEntry, NetworkCapability, Runtime};
pub use components::{COMPONENTS_KEY, PluginLayer, plugin_layer, theme_dirs};
pub use digest::tree as tree_digest;
pub use install::{Fetched, fetch, parse_spec};
pub use manage::{Manager, Pending, Summary, summarize};
pub use manifest::{
    FILE as MANIFEST_FILE, Manifest, ManifestError, inside as manifest_path, valid_name,
};
pub use registry::{Entry, Locations, Lock, PluginScope, Registry, Source};
pub use user_config::{
    ConfigField, PluginSettings, fields as config_fields, resolve as resolve_settings,
};

/// The environment switch for pure mode.
pub const PURE_ENV: &str = "CYBER_PURE";

static PURE: AtomicBool = AtomicBool::new(false);

/// `--pure`: turn pure mode on for the rest of this process, whatever the environment.
pub fn force_pure() {
    PURE.store(true, Ordering::SeqCst);
}

/// Whether plugins, hooks and MCP servers outside organization policy are off.
pub fn pure(env: &dyn crate::env::EnvSource) -> bool {
    PURE.load(Ordering::SeqCst) || env.flag(PURE_ENV)
}

/// Who owns the stored secrets of plugin `name` in `scope` (`plugins-marketplace` → User
/// configuration): `<name>.<scope>.<digest>`, where the digest names the user (user
/// scope) or the checkout of `location` (project and local scope). A same-named plugin
/// from another scope or checkout, such as one a cloned repository lists, therefore never
/// reads or deletes these secrets.
pub fn secret_owner(name: &str, scope: PluginScope, location: &std::path::Path) -> String {
    let base = match scope {
        PluginScope::User => "user".to_owned(),
        PluginScope::Project | PluginScope::Local => {
            let root = crate::config::project_root(location);
            std::fs::canonicalize(&root)
                .unwrap_or(root)
                .display()
                .to_string()
        }
    };
    let digest = format!("{:x}", Sha256::digest(base.as_bytes()));
    format!("{name}.{}.{}", scope.as_str(), &digest[..16])
}

/// `(plugin, folders)` of one component kind (`skills` or `commands`) as the loader
/// recorded it under [`COMPONENTS_KEY`] in a resolved configuration.
pub fn component_dirs(
    config: &serde_json::Value,
    kind: &str,
) -> Vec<(String, Vec<std::path::PathBuf>)> {
    config
        .get(COMPONENTS_KEY)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_owned();
            let dirs = entry
                .get(kind)?
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(std::path::PathBuf::from)
                .collect();
            Some((name, dirs))
        })
        .collect()
}

/// The approval digest of one installed plugin: its name, version and content digest.
pub fn trust_digest(name: &str, version: &str, content: &str) -> String {
    let mut hasher = Sha256::new();
    for part in ["plugin", name, version, content] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}
