//! Install scopes and the lockfile (`plugins-marketplace` → Install scopes, Plugin
//! sources): `user` is `<config>/plugins.json`, `project` is `.cyber/plugins.json` (committed)
//! pinned by `.cyber/plugins.lock`, and `local` is `.cyber/plugins.local.json`. What a
//! scope records is only a request: nothing loads until its content digest is trusted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    User,
    Project,
    Local,
}

impl PluginScope {
    /// Most specific first: the order a name is resolved in.
    pub const ALL: [Self; 3] = [Self::Local, Self::Project, Self::User];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == text)
    }
}

/// Where a plugin comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Source {
    /// A local directory, used in place. A relative path is relative to the project root
    /// (project and local scope) or the config directory (user scope).
    Path { path: String },
    /// An npm package spec, packed with lifecycle scripts disabled: a registry name
    /// (`@scope/name@range`), a tarball or a local package directory.
    Npm { package: String },
    /// A git repository; `ref` is a branch, tag or commit and `path` a subdirectory.
    Git {
        url: String,
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path { path } => write!(f, "{path}"),
            Self::Npm { package } => write!(f, "npm:{package}"),
            Self::Git {
                url,
                reference,
                path,
            } => {
                write!(f, "git+{url}")?;
                if let Some(reference) = reference {
                    write!(f, "#{reference}")?;
                }
                if let Some(path) = path {
                    write!(f, " ({path})")?;
                }
                Ok(())
            }
        }
    }
}

/// The installed resolution of a plugin: what the trust and the digest check bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Lock {
    pub version: String,
    /// `sha256:<hex>` of the installed tree.
    pub digest: String,
    /// The resolved git commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Installed with `--force` despite `engines.cyber`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub source: Source,
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    /// A semver range the installed version must satisfy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// User and local scope keep the resolution here; project scope keeps it in the
    /// lockfile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Lock>,
}

fn enabled() -> bool {
    true
}

fn is_enabled(enabled: &bool) -> bool {
    *enabled
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    plugins: BTreeMap<String, Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LockEntry {
    source: Source,
    #[serde(flatten)]
    lock: Lock,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LockDocument {
    #[serde(default = "lock_version")]
    version: u32,
    #[serde(default)]
    plugins: BTreeMap<String, LockEntry>,
}

fn lock_version() -> u32 {
    1
}

/// The files of every scope for one Location.
#[derive(Debug, Clone)]
pub struct Locations {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub project_root: PathBuf,
    pub trust_file: PathBuf,
}

impl Locations {
    pub fn new(paths: &Paths, location: &Path) -> Self {
        Self {
            config_dir: paths.config.clone(),
            cache_dir: paths.cache.join("plugins"),
            project_root: crate::config::project_root(location),
            trust_file: paths.trust_file(),
        }
    }

    pub fn registry(&self, scope: PluginScope) -> PathBuf {
        match scope {
            PluginScope::User => self.config_dir.join("plugins.json"),
            PluginScope::Project => self.project_root.join(".cyber/plugins.json"),
            PluginScope::Local => self.project_root.join(".cyber/plugins.local.json"),
        }
    }

    pub fn lockfile(&self) -> PathBuf {
        self.project_root.join(".cyber/plugins.lock")
    }

    /// `<cache>/plugins/<name>/<version>-<digest>`. The cache is shared by every scope
    /// and checkout, so it is content-addressed: a fetch never replaces a tree some
    /// Location already trusts.
    pub fn cache(&self, name: &str, version: &str, digest: &str) -> PathBuf {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        let short: String = hex
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(32)
            .collect();
        self.cache_dir.join(name).join(format!("{version}-{short}"))
    }

    /// The file recording `scope`'s installed digests: the lockfile for project scope,
    /// else the scope file.
    pub fn digest_holder(&self, scope: PluginScope) -> PathBuf {
        match scope {
            PluginScope::Project => self.lockfile(),
            PluginScope::User | PluginScope::Local => self.registry(scope),
        }
    }

    /// What a relative path source resolves against.
    pub fn base(&self, scope: PluginScope) -> &Path {
        match scope {
            PluginScope::User => &self.config_dir,
            PluginScope::Project | PluginScope::Local => &self.project_root,
        }
    }

    /// A path source's directory: absolute as recorded, or relative to the scope base.
    pub fn path_root(&self, scope: PluginScope, path: &str) -> PathBuf {
        if Path::new(path).is_absolute() {
            return PathBuf::from(path);
        }
        // Component by component: a `/` is no separator under a `\\?\` base.
        path.split(['/', '\\'])
            .filter(|part| !part.is_empty() && *part != ".")
            .fold(self.base(scope).to_path_buf(), |dir, part| dir.join(part))
    }

    /// The directory of an installed entry, without checking that it exists.
    pub fn root(&self, scope: PluginScope, name: &str, entry: &Entry) -> Option<PathBuf> {
        match &entry.source {
            Source::Path { path } => Some(self.path_root(scope, path)),
            Source::Git { .. } | Source::Npm { .. } => entry
                .installed
                .as_ref()
                .map(|lock| self.cache(name, &lock.version, &lock.digest)),
        }
    }

    /// Approvals are keyed by the checkout for project and local scope, and by the user
    /// config directory for user scope.
    pub fn trust_key(&self, scope: PluginScope) -> std::io::Result<String> {
        Ok(match scope {
            PluginScope::User => format!(
                "user:{}",
                std::fs::canonicalize(&self.config_dir)?.display()
            ),
            PluginScope::Project | PluginScope::Local => std::fs::canonicalize(&self.project_root)?
                .display()
                .to_string(),
        })
    }
}

/// One scope's entries.
#[derive(Debug, Clone)]
pub struct Registry {
    pub scope: PluginScope,
    pub entries: BTreeMap<String, Entry>,
}

impl Registry {
    pub fn read(locations: &Locations, scope: PluginScope) -> Result<Self, String> {
        let path = locations.registry(scope);
        let document: Document = read_json(&path)?.unwrap_or_default();
        let mut entries = document.plugins;
        if let Some(name) = entries
            .keys()
            .find(|name| !super::manifest::valid_name(name))
        {
            return Err(format!("{}: invalid plugin name {name:?}", path.display()));
        }
        if scope == PluginScope::Project {
            let lock: LockDocument = read_json(&locations.lockfile())?.unwrap_or_default();
            for (name, entry) in &mut entries {
                entry.installed = lock
                    .plugins
                    .get(name)
                    .filter(|locked| locked.source == entry.source)
                    .map(|locked| locked.lock.clone());
            }
        }
        Ok(Self { scope, entries })
    }

    /// Write the scope file (and, for project scope, the lockfile) in place.
    pub fn write(&self, locations: &Locations) -> Result<(), String> {
        let mut document = Document::default();
        let mut lock = LockDocument {
            version: 1,
            plugins: BTreeMap::new(),
        };
        for (name, entry) in &self.entries {
            let mut entry = entry.clone();
            if self.scope == PluginScope::Project
                && let Some(installed) = entry.installed.take()
            {
                lock.plugins.insert(
                    name.clone(),
                    LockEntry {
                        source: entry.source.clone(),
                        lock: installed,
                    },
                );
            }
            document.plugins.insert(name.clone(), entry);
        }
        write_json(&locations.registry(self.scope), &document)?;
        if self.scope == PluginScope::Project {
            write_json(&locations.lockfile(), &lock)?;
        }
        Ok(())
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if linked {
        return Err(format!("{}: a linked file is not read", path.display()));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    let parent = path.parent().ok_or("registry path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let staged = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&staged, text).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::rename(&staged, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locations(root: &Path) -> Locations {
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        Locations {
            config_dir: root.join("config"),
            cache_dir: root.join("cache/plugins"),
            project_root: root.join("repo"),
            trust_file: root.join("state/trust.json"),
        }
    }

    #[test]
    fn project_resolution_lives_in_the_lockfile_and_must_match_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let locations = locations(dir.path());
        let source = Source::Git {
            url: "https://example.test/kit.git".into(),
            reference: Some("main".into()),
            path: None,
        };
        let lock = Lock {
            version: "1.0.0".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            commit: Some("c".repeat(40)),
            forced: false,
        };
        let registry = Registry {
            scope: PluginScope::Project,
            entries: BTreeMap::from([(
                "kit".into(),
                Entry {
                    source: source.clone(),
                    enabled: true,
                    version: None,
                    installed: Some(lock.clone()),
                },
            )]),
        };
        registry.write(&locations).unwrap();
        let shared = std::fs::read_to_string(locations.registry(PluginScope::Project)).unwrap();
        assert!(!shared.contains("digest"), "{shared}");
        let pinned = std::fs::read_to_string(locations.lockfile()).unwrap();
        assert!(pinned.contains(&"c".repeat(40)), "{pinned}");
        let read = Registry::read(&locations, PluginScope::Project).unwrap();
        assert_eq!(read.entries["kit"].installed.as_ref(), Some(&lock));

        // A source the lock does not describe is not installed.
        let mut shared: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(locations.registry(PluginScope::Project)).unwrap(),
        )
        .unwrap();
        shared["plugins"]["kit"]["source"]["ref"] = "other".into();
        std::fs::write(locations.registry(PluginScope::Project), shared.to_string()).unwrap();
        let read = Registry::read(&locations, PluginScope::Project).unwrap();
        assert_eq!(read.entries["kit"].installed, None);
    }
}
