//! Checkout-scoped trust approvals (`workspace-trust`).
//!
//! Approvals live outside the repository, keyed by the canonical checkout root and the
//! digest of the approved security-sensitive definitions. Approving a new digest for a root
//! replaces the previous approval, so a changed definition is untrusted until re-approved.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub checkout_root: String,
    pub digest: String,
    pub approved_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    approvals: Vec<Approval>,
    #[serde(default)]
    hook_approvals: Vec<Approval>,
    #[serde(default)]
    mcp_approvals: Vec<Approval>,
}

/// Explicitly reviewed handler digests for one invocation; never persisted.
pub struct HookInvocationTrust {
    checkout_root: PathBuf,
    digests: BTreeSet<String>,
}

impl HookInvocationTrust {
    pub fn new(root: &Path, digests: &[String]) -> io::Result<Self> {
        for digest in digests {
            validate_hook_digest(digest)?;
        }
        Ok(Self {
            checkout_root: std::fs::canonicalize(root)?,
            digests: digests.iter().cloned().collect(),
        })
    }

    pub fn is_approved(&self, root: &Path, digest: &str) -> io::Result<bool> {
        validate_hook_digest(digest)?;
        Ok(std::fs::canonicalize(root)? == self.checkout_root && self.digests.contains(digest))
    }
}

struct TrustWriteLock(File);

impl Drop for TrustWriteLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub struct TrustStore {
    path: PathBuf,
}

impl TrustStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn is_approved(&self, root: &Path, digest: &str) -> io::Result<bool> {
        let key = root.to_string_lossy();
        Ok(self
            .load()?
            .approvals
            .iter()
            .any(|a| a.checkout_root == key && a.digest == digest))
    }

    pub fn approval(&self, root: &Path) -> io::Result<Option<Approval>> {
        let key = root.to_string_lossy();
        Ok(self
            .load()?
            .approvals
            .into_iter()
            .find(|a| a.checkout_root == key))
    }

    pub fn approve(&self, root: &Path, digest: &str) -> io::Result<()> {
        let key = root.to_string_lossy().into_owned();
        self.update(|file| {
            file.approvals.retain(|a| a.checkout_root != key);
            file.approvals.push(Approval {
                checkout_root: key,
                digest: digest.to_string(),
                approved_at: unix_seconds(),
            });
            ((), true)
        })
    }

    /// Returns whether an approval existed.
    pub fn revoke(&self, root: &Path) -> io::Result<bool> {
        let key = root.to_string_lossy();
        self.update(|file| {
            let before =
                file.approvals.len() + file.hook_approvals.len() + file.mcp_approvals.len();
            file.approvals.retain(|a| a.checkout_root != key);
            file.hook_approvals.retain(|a| a.checkout_root != key);
            file.mcp_approvals.retain(|a| a.checkout_root != key);
            let removed =
                file.approvals.len() + file.hook_approvals.len() + file.mcp_approvals.len()
                    != before;
            (removed, removed)
        })
    }

    /// Workspace approval does not grant permission to execute an individual hook.
    pub fn is_hook_approved(&self, root: &Path, digest: &str) -> io::Result<bool> {
        validate_hook_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        Ok(self
            .load()?
            .hook_approvals
            .iter()
            .any(|a| a.checkout_root == key && a.digest == digest))
    }

    pub fn approve_hook(&self, root: &Path, digest: &str) -> io::Result<()> {
        validate_hook_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        self.update(|file| {
            file.hook_approvals
                .retain(|a| a.checkout_root != key || a.digest != digest);
            file.hook_approvals.push(Approval {
                checkout_root: key,
                digest: digest.to_string(),
                approved_at: unix_seconds(),
            });
            ((), true)
        })
    }

    pub fn revoke_hook(&self, root: &Path, digest: &str) -> io::Result<bool> {
        validate_hook_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        self.update(|file| {
            let before = file.hook_approvals.len();
            file.hook_approvals
                .retain(|a| a.checkout_root != key || a.digest != digest);
            let removed = file.hook_approvals.len() != before;
            (removed, removed)
        })
    }

    pub fn is_mcp_approved(&self, root: &Path, digest: &str) -> io::Result<bool> {
        validate_mcp_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        Ok(self
            .load()?
            .mcp_approvals
            .iter()
            .any(|a| a.checkout_root == key && a.digest == digest))
    }

    pub fn approve_mcp(&self, root: &Path, digest: &str) -> io::Result<()> {
        validate_mcp_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        self.update(|file| {
            file.mcp_approvals
                .retain(|a| a.checkout_root != key || a.digest != digest);
            file.mcp_approvals.push(Approval {
                checkout_root: key,
                digest: digest.into(),
                approved_at: unix_seconds(),
            });
            ((), true)
        })
    }

    pub fn revoke_mcp(&self, root: &Path, digest: &str) -> io::Result<bool> {
        validate_mcp_digest(digest)?;
        let key = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
        self.update(|file| {
            let before = file.mcp_approvals.len();
            file.mcp_approvals
                .retain(|a| a.checkout_root != key || a.digest != digest);
            let removed = file.mcp_approvals.len() != before;
            (removed, removed)
        })
    }

    fn update<T>(&self, change: impl FnOnce(&mut TrustFile) -> (T, bool)) -> io::Result<T> {
        let lock_path = self.path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        set_private(&lock_path)?;
        file.lock()?;
        let _lock = TrustWriteLock(file);
        let mut file = self.load()?;
        let (result, changed) = change(&mut file);
        if changed {
            self.save(&file)?;
        }
        Ok(result)
    }

    fn load(&self) -> io::Result<TrustFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(TrustFile {
                version: 1,
                approvals: Vec::new(),
                hook_approvals: Vec::new(),
                mcp_approvals: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    fn save(&self, file: &TrustFile) -> io::Result<()> {
        let text = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
        write_private_atomic(&self.path, text.as_bytes())
    }
}

fn validate_hook_digest(digest: &str) -> io::Result<()> {
    if digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid hook SHA-256 digest",
    ))
}

fn validate_mcp_digest(digest: &str) -> io::Result<()> {
    validate_hook_digest(digest)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid MCP SHA-256 digest"))
}

/// Write via a temporary file and rename, with mode 0600 on Unix.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    set_private(&tmp)?;
    std::fs::rename(&tmp, path)
}

#[cfg(unix)]
fn set_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
