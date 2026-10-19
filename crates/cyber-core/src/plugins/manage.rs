//! Install, enable, update, disable and remove (`plugins-marketplace` → Plugin CLI). Each
//! change is prepared first, so the caller can show what it trusts before committing it:
//! committing records the scope entry (and the project lockfile) and approves exactly the
//! fetched name, version and content digest.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use super::active::ActivePlugin;
use super::install::{Fetched, fetch};
use super::registry::{Entry, Locations, Lock, PluginScope, Registry, Source};
use crate::trust::TrustStore;

/// What a plugin contributes, for review before trusting it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub hooks: Vec<String>,
    pub mcp: Vec<String>,
    pub agents: Vec<String>,
    pub commands: usize,
    pub skills: usize,
    /// `<runtime> <main>` when the plugin runs a host process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The declared capabilities; the host is sandboxed to exactly these.
    pub capabilities: Vec<String>,
    pub warnings: Vec<String>,
}

/// A fetched, verified change waiting for [`Manager::commit`].
#[derive(Debug, Clone)]
pub struct Pending {
    pub scope: PluginScope,
    pub name: String,
    pub entry: Entry,
    pub fetched: Fetched,
    pub summary: Summary,
    /// The approval this change records.
    pub approval: String,
    /// Already approved: committing changes nothing a reviewer must see.
    pub approved: bool,
}

pub struct Manager {
    pub locations: Locations,
    store: TrustStore,
    running: String,
}

impl Manager {
    pub fn new(locations: Locations, running: &str) -> Self {
        Self {
            store: TrustStore::new(&locations.trust_file),
            locations,
            running: running.into(),
        }
    }

    /// Fetch `source` for `scope`. A project install of a source the lockfile already
    /// pins uses the pinned commit; `cyber plugins update` moves it.
    pub fn prepare_install(
        &self,
        source: Source,
        scope: PluginScope,
        force: bool,
    ) -> Result<Pending, String> {
        let source = self.relative(source, scope);
        let registry = Registry::read(&self.locations, scope)?;
        let pinned = registry
            .entries
            .values()
            .find(|entry| entry.source == source)
            .and_then(|entry| entry.installed.as_ref())
            .and_then(|lock| lock.commit.clone());
        let fetched = fetch(&self.locations, scope, &source, pinned.as_deref())?;
        let entry = Entry {
            source,
            enabled: true,
            version: None,
            installed: None,
        };
        self.pending(scope, fetched.manifest.name.clone(), entry, fetched, force)
    }

    /// Enable `name` where a scope lists it, installing it first when it is missing;
    /// the lockfile's commit and digest must still match.
    pub fn prepare_enable(
        &self,
        name: &str,
        scope: Option<PluginScope>,
        force: bool,
    ) -> Result<Pending, String> {
        let (scope, mut entry) = self.listed(name, scope)?;
        let lock = entry.installed.clone();
        let fetched = fetch(
            &self.locations,
            scope,
            &entry.source,
            lock.as_ref().and_then(|lock| lock.commit.as_deref()),
        )?;
        if let Some(lock) = &lock
            && (lock.digest != fetched.digest || lock.version != fetched.manifest.version)
        {
            self.discard_fetched(&fetched);
            return Err(format!(
                "{name}: the fetched content does not match the recorded {} ({}); \
                 run `cyber plugins update {name}` to review the new content",
                lock.digest,
                if scope == PluginScope::Project {
                    "plugins.lock"
                } else {
                    "install"
                }
            ));
        }
        entry.enabled = true;
        self.pending(scope, name.into(), entry, fetched, force)
    }

    /// Re-resolve `name` from its source (the git ref, not the pinned commit).
    pub fn prepare_update(
        &self,
        name: &str,
        scope: Option<PluginScope>,
        force: bool,
    ) -> Result<Pending, String> {
        let (scope, entry) = self.listed(name, scope)?;
        let fetched = fetch(&self.locations, scope, &entry.source, None)?;
        self.pending(scope, name.into(), entry, fetched, force)
    }

    /// The [`Pending`] change, or the refusal with the fetched copy discarded.
    fn pending(
        &self,
        scope: PluginScope,
        name: String,
        entry: Entry,
        fetched: Fetched,
        force: bool,
    ) -> Result<Pending, String> {
        let discarded = fetched.clone();
        self.verified(scope, name, entry, fetched, force)
            .inspect_err(|_| self.discard_fetched(&discarded))
    }

    fn verified(
        &self,
        scope: PluginScope,
        name: String,
        mut entry: Entry,
        fetched: Fetched,
        force: bool,
    ) -> Result<Pending, String> {
        let manifest = &fetched.manifest;
        if manifest.name != name {
            return Err(format!(
                "{name}: the source provides plugin {}",
                manifest.name
            ));
        }
        let mismatch = manifest.engine_mismatch(&self.running);
        if let Some(mismatch) = &mismatch
            && !force
        {
            return Err(format!("{name}@{}: {mismatch}", manifest.version));
        }
        if let Some(range) = &entry.version {
            let accepted = semver::VersionReq::parse(range)
                .ok()
                .zip(semver::Version::parse(&manifest.version).ok())
                .is_some_and(|(range, version)| range.matches(&version));
            if !accepted {
                return Err(format!(
                    "{name}@{} does not satisfy the listed range {range}",
                    manifest.version
                ));
            }
        }
        entry.installed = Some(Lock {
            version: manifest.version.clone(),
            digest: fetched.digest.clone(),
            commit: fetched.commit.clone(),
            forced: mismatch.is_some(),
        });
        let approval = super::trust_digest(&name, &manifest.version, &fetched.digest);
        let key = self.key(scope)?;
        let approved = self
            .store
            .is_plugin_approved(&key, &approval)
            .map_err(|e| e.to_string())?;
        let summary = summarize(&name, scope, &fetched);
        Ok(Pending {
            scope,
            name,
            entry,
            fetched,
            summary,
            approval,
            approved,
        })
    }

    /// Record the entry and approve its digest, replacing the earlier approval. The cache
    /// tree it uses is retained for this scope; the one it replaces is released.
    pub fn commit(&self, pending: Pending) -> Result<(), String> {
        let cached = self.cached(pending.scope, &pending.name, &pending.entry);
        if cached.as_ref().is_some_and(|(tree, _)| !tree.is_dir()) {
            return Err(format!(
                "{}: the fetched copy was removed before it was recorded; run the command again",
                pending.name
            ));
        }
        let mut registry = Registry::read(&self.locations, pending.scope)?;
        let replaced = registry.entries.get(&pending.name).cloned();
        let previous: Vec<String> = replaced
            .as_ref()
            .and_then(|entry| entry.installed.as_ref())
            .map(|lock| super::trust_digest(&pending.name, &lock.version, &lock.digest))
            .into_iter()
            .collect();
        let key = self.key(pending.scope)?;
        self.store
            .approve_plugin(&key, &pending.approval, &previous)
            .map_err(|e| e.to_string())?;
        let (scope, name) = (pending.scope, pending.name.clone());
        registry.entries.insert(pending.name, pending.entry);
        registry.write(&self.locations)?;
        let holder = self.locations.digest_holder(scope);
        if let Some((tree, digest)) = &cached {
            super::cache_refs::retain(tree, digest, &holder)?;
        }
        match replaced.and_then(|entry| self.cached(scope, &name, &entry)) {
            Some(old) if Some(&old) != cached.as_ref() => {
                super::cache_refs::release(&old.0, &old.1, &holder)
            }
            _ => Ok(()),
        }
    }

    /// Drop a change that will not be committed: a cache tree its fetch created and
    /// nobody records is deleted, so a declined update leaves only the installed tree.
    pub fn discard(&self, pending: &Pending) {
        self.discard_fetched(&pending.fetched);
    }

    fn discard_fetched(&self, fetched: &Fetched) {
        if fetched.fresh {
            let _ = super::cache_refs::discard(&fetched.root, &fetched.digest);
        }
    }

    /// The cache tree (and its digest) of an installed git or npm entry.
    fn cached(
        &self,
        scope: PluginScope,
        name: &str,
        entry: &Entry,
    ) -> Option<(std::path::PathBuf, String)> {
        if matches!(entry.source, Source::Path { .. }) {
            return None;
        }
        let lock = entry.installed.as_ref()?;
        let tree = self.locations.root(scope, name, entry)?;
        Some((tree, lock.digest.clone()))
    }

    /// Turn `name` off in `scope`, or in every scope listing it.
    pub fn disable(
        &self,
        name: &str,
        scope: Option<PluginScope>,
    ) -> Result<Vec<PluginScope>, String> {
        self.each_listing(name, scope, |registry| {
            if let Some(entry) = registry.entries.get_mut(name) {
                entry.enabled = false;
            }
        })
    }

    /// Remove `name` from `scope`, or from every scope listing it, revoking its trust.
    /// Each removed entry releases its cache tree, which is deleted once no scope of any
    /// Location still records that digest.
    pub fn remove(
        &self,
        name: &str,
        scope: Option<PluginScope>,
    ) -> Result<Vec<PluginScope>, String> {
        let mut removed = Vec::new();
        let scopes = self.each_listing(name, scope, |registry| {
            if let Some(entry) = registry.entries.remove(name) {
                removed.push((registry.scope, entry));
            }
        })?;
        for (scope, entry) in removed {
            let Some(lock) = &entry.installed else {
                continue;
            };
            let key = self.key(scope)?;
            self.store
                .revoke_plugin(
                    &key,
                    &[super::trust_digest(name, &lock.version, &lock.digest)],
                )
                .map_err(|e| e.to_string())?;
            if let Some((tree, digest)) = self.cached(scope, name, &entry) {
                let holder = self.locations.digest_holder(scope);
                super::cache_refs::release(&tree, &digest, &holder)?;
            }
        }
        Ok(scopes)
    }

    fn each_listing(
        &self,
        name: &str,
        scope: Option<PluginScope>,
        mut change: impl FnMut(&mut Registry),
    ) -> Result<Vec<PluginScope>, String> {
        let mut changed = Vec::new();
        for candidate in PluginScope::ALL {
            if scope.is_some_and(|scope| scope != candidate) {
                continue;
            }
            let mut registry = Registry::read(&self.locations, candidate)?;
            if !registry.entries.contains_key(name) {
                continue;
            }
            change(&mut registry);
            registry.write(&self.locations)?;
            changed.push(candidate);
        }
        if changed.is_empty() {
            return Err(not_listed(name, scope));
        }
        Ok(changed)
    }

    /// The most specific scope listing `name` (or `scope`) and its entry.
    fn listed(
        &self,
        name: &str,
        scope: Option<PluginScope>,
    ) -> Result<(PluginScope, Entry), String> {
        for candidate in PluginScope::ALL {
            if scope.is_some_and(|scope| scope != candidate) {
                continue;
            }
            let registry = Registry::read(&self.locations, candidate)?;
            if let Some(entry) = registry.entries.get(name) {
                return Ok((candidate, entry.clone()));
            }
        }
        Err(not_listed(name, scope))
    }

    /// Every listed `(scope, name)`, most specific scope first.
    pub fn names(&self) -> Result<Vec<(PluginScope, String)>, String> {
        let mut out = Vec::new();
        for scope in PluginScope::ALL {
            let registry = Registry::read(&self.locations, scope)?;
            out.extend(registry.entries.into_keys().map(|name| (scope, name)));
        }
        Ok(out)
    }

    fn key(&self, scope: PluginScope) -> Result<String, String> {
        if scope == PluginScope::User {
            std::fs::create_dir_all(&self.locations.config_dir).map_err(|e| e.to_string())?;
        }
        self.locations.trust_key(scope).map_err(|e| e.to_string())
    }

    /// A path source inside the project is recorded relative to it for project and
    /// local scope, so the committed file works in every checkout.
    fn relative(&self, source: Source, scope: PluginScope) -> Source {
        let Source::Path { path } = &source else {
            return source;
        };
        if scope == PluginScope::User {
            return source;
        }
        let root = std::fs::canonicalize(&self.locations.project_root)
            .unwrap_or_else(|_| self.locations.project_root.clone());
        match Path::new(path).strip_prefix(&root) {
            Ok(relative) => Source::Path {
                path: relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            },
            Err(_) => source,
        }
    }
}

fn not_listed(name: &str, scope: Option<PluginScope>) -> String {
    match scope {
        Some(scope) => format!("plugin {name} is not listed in {} scope", scope.as_str()),
        None => format!("plugin {name} is not listed in any scope"),
    }
}

/// What `fetched` would contribute, with every name it would use.
pub fn summarize(name: &str, scope: PluginScope, fetched: &Fetched) -> Summary {
    let plugin = ActivePlugin {
        name: name.into(),
        scope,
        root: fetched.root.clone(),
        manifest: fetched.manifest.clone(),
        digest: fetched.digest.clone(),
    };
    let layer =
        super::components::plugin_layer(&plugin, &mut BTreeSet::new(), &mut BTreeSet::new());
    let keys = |key: &str| -> Vec<String> {
        layer.value[key]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    };
    let count = |kind: &str, matches: fn(&Path) -> bool| -> usize {
        super::component_dirs(
            &serde_json::json!({ super::COMPONENTS_KEY: [layer.components.clone()] }),
            kind,
        )
        .into_iter()
        .flat_map(|(_, dirs)| dirs)
        .filter_map(|dir| super::digest::files(&dir).ok())
        .flatten()
        .filter(|(_, path)| matches(path))
        .count()
    };
    Summary {
        hooks: keys("hooks"),
        mcp: keys("mcp"),
        agents: keys("agents"),
        commands: count("commands", |path| {
            path.extension().is_some_and(|e| e == "md")
        }),
        skills: count("skills", |path| {
            path.file_name().is_some_and(|n| n == "SKILL.md")
        }),
        host: fetched
            .manifest
            .host
            .as_ref()
            .map(|host| format!("{} {}", host.runtime.as_str(), host.main)),
        capabilities: fetched.manifest.capabilities.describe(),
        warnings: layer.warnings,
    }
}
