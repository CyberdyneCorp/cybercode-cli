//! XDG directory layout (`storage-events` → XDG directory layout, Database location).

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::env::EnvSource;
use crate::version::Channel;

/// The four base directories plus the temp directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub tmp: PathBuf,
}

impl Paths {
    /// Resolve directories. An explicit `XDG_*` variable or `CYBER_CONFIG_DIR` wins over
    /// `CYBER_HOME` for the directory it names; `CYBER_HOME` relocates the rest.
    pub fn resolve(env: &dyn EnvSource, home: &Path) -> Self {
        let root = env.get("CYBER_HOME").map(PathBuf::from);
        let base = |xdg: &str, default_rel: &str, sub: &str| -> PathBuf {
            if let Some(dir) = env.get(xdg) {
                return PathBuf::from(dir).join("cyber");
            }
            match &root {
                Some(r) => r.join(sub),
                None => home.join(default_rel).join("cyber"),
            }
        };
        let config = env
            .get("CYBER_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| base("XDG_CONFIG_HOME", ".config", "config"));
        Self {
            data: base("XDG_DATA_HOME", ".local/share", "data"),
            config,
            state: base("XDG_STATE_HOME", ".local/state", "state"),
            cache: base("XDG_CACHE_HOME", ".cache", "cache"),
            tmp: std::env::temp_dir().join("cyber"),
        }
    }

    /// Every directory created at startup.
    pub fn all_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![
            self.data.clone(),
            self.config.clone(),
            self.state.clone(),
            self.cache.clone(),
            self.tmp.clone(),
        ];
        for sub in [
            "log",
            "tool-output",
            "jobs",
            "snapshot",
            "memory",
            "worktrees",
        ] {
            dirs.push(self.data.join(sub));
        }
        for sub in ["bin", "models", "plugins"] {
            dirs.push(self.cache.join(sub));
        }
        dirs
    }

    pub fn ensure(&self) -> io::Result<()> {
        for dir in self.all_dirs() {
            #[cfg(windows)]
            if dir == self.data.join("memory") {
                continue;
            }
            std::fs::create_dir_all(dir)?;
        }
        #[cfg(windows)]
        crate::memory::MemoryStore::ensure_root(&self.data).map_err(|error| match error {
            crate::memory::MemoryStorageError::Io(error) => error,
            _ => io::Error::new(io::ErrorKind::PermissionDenied, "unsafe memory root"),
        })?;
        Ok(())
    }

    /// Advisory lock held by the registered server, the single writer owner.
    pub fn server_lock(&self) -> PathBuf {
        self.state.join("server.lock")
    }

    /// Checkout-scoped workspace-trust approvals.
    pub fn trust_file(&self) -> PathBuf {
        self.state.join("trust.json")
    }
}

/// Where the database lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseLocation {
    File(PathBuf),
    Memory,
}

/// `CYBER_DB` (`:memory:`, absolute, or relative to data), else `cyber.db` on the stable
/// channel and `cyber-<channel>.db` otherwise.
pub fn database_location(paths: &Paths, env: &dyn EnvSource, channel: Channel) -> DatabaseLocation {
    match env.get("CYBER_DB") {
        Some(v) if v == ":memory:" => DatabaseLocation::Memory,
        Some(v) => {
            let p = PathBuf::from(&v);
            if p.is_absolute() {
                DatabaseLocation::File(p)
            } else {
                DatabaseLocation::File(paths.data.join(p))
            }
        }
        None => DatabaseLocation::File(paths.data.join(default_db_name(channel))),
    }
}

pub fn default_db_name(channel: Channel) -> String {
    match channel {
        Channel::Stable => "cyber.db".into(),
        other => format!("cyber-{}.db", other.as_str()),
    }
}

/// The user's home directory from `HOME` (or `USERPROFILE` on Windows).
pub fn home_dir(env: &dyn EnvSource) -> Option<PathBuf> {
    env.get("HOME")
        .or_else(|| env.get("USERPROFILE"))
        .map(PathBuf::from)
}
