//! Shared configuration and names for managed worktrees.

mod activity;
mod changes;
mod includes;
mod location;
mod removal;
mod repository;
mod setup;
pub use activity::{CheckoutActivity, CheckoutLease};
pub use changes::{ChangedFile, Changes};
pub use includes::IncludedFile;
pub use removal::{RemovalActivity, RemovalPhase, RemovalRecord};
pub use repository::{
    Branch, GitExecution, GitFuture, ListedWorktree, Managed, Repository, WorktreeStatus,
};
pub use setup::{SetupEvent, SetupExecution, SetupFuture, SetupOutcome, SetupSink, SetupStream};

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cleanup {
    #[default]
    Auto,
    Keep,
    Ask,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub root: Option<PathBuf>,
    pub branch_prefix: String,
    pub base: String,
    pub setup: Vec<String>,
    pub cleanup: Cleanup,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            root: None,
            branch_prefix: "cyber/".into(),
            base: "HEAD".into(),
            setup: Vec::new(),
            cleanup: Cleanup::Auto,
        }
    }
}

impl Settings {
    /// Read only a resolved configuration, after layering and trust filtering.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.get("worktrees") else {
            return Ok(Self::default());
        };
        let mut settings: Self =
            serde_json::from_value(value.clone()).map_err(|error| format!("worktrees: {error}"))?;
        if let Some(keep) = value.get("keep") {
            if keep != "always" {
                return Err("worktrees.keep: expected always".into());
            }
            settings.cleanup = Cleanup::Keep;
        }
        if settings
            .root
            .as_ref()
            .is_some_and(|root| root.as_os_str().is_empty())
        {
            return Err("worktrees.root: expected a nonempty path".into());
        }
        if settings.base.trim().is_empty() {
            return Err("worktrees.base: expected a nonempty Git revision".into());
        }
        if settings
            .setup
            .iter()
            .any(|command| command.trim().is_empty())
        {
            return Err("worktrees.setup: commands must not be empty".into());
        }
        Ok(settings)
    }
}

/// A name matching `^[a-z0-9][a-z0-9._-]{0,62}$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name(String);

impl Name {
    pub fn parse(value: &str) -> Result<Self, String> {
        let bytes = value.as_bytes();
        let initial = bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        let rest = bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte)
        });
        if !initial || bytes.len() > 63 || !rest {
            return Err("Invalid worktree name: expected ^[a-z0-9][a-z0-9._-]{0,62}$".into());
        }
        Ok(Self(value.into()))
    }

    /// Names are candidates; the lifecycle manager must still reserve them atomically.
    pub fn generate() -> Self {
        const ADJECTIVES: [&str; 8] = [
            "calm", "clear", "quick", "bright", "quiet", "steady", "bold", "fresh",
        ];
        const NOUNS: [&str; 8] = [
            "cedar", "maple", "birch", "pine", "fox", "owl", "river", "lake",
        ];
        let bytes = ulid::Ulid::new().to_bytes();
        Self(format!(
            "{}-{}-{:02x}{:02x}",
            ADJECTIVES[usize::from(bytes[12]) % ADJECTIVES.len()],
            NOUNS[usize::from(bytes[13]) % NOUNS.len()],
            bytes[14],
            bytes[15]
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exclusive lock for cooperating lifecycle operations in a shared repository.
/// Keep its persistent file in place; deleting it can split lock ownership.
#[derive(Debug)]
pub struct RepositoryLock {
    _file: std::fs::File,
}

impl RepositoryLock {
    /// The caller must supply the repository's resolved common Git directory.
    /// `None` denotes contention; filesystem and locking failures remain errors.
    pub fn try_acquire(git_dir: &std::path::Path) -> std::io::Result<Option<Self>> {
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(git_dir.join("cyber-worktree.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        // Release explicitly before close, including transient inherited descriptors.
        let _ = self._file.unlock();
    }
}
