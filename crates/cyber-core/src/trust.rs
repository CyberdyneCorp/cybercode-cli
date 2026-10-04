//! Checkout-scoped trust approvals (`workspace-trust`).
//!
//! Approvals live outside the repository, keyed by the canonical checkout root and the
//! digest of the approved security-sensitive definitions. Approving a new digest for a root
//! replaces the previous approval, so a changed definition is untrusted until re-approved.

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
        let mut file = self.load()?;
        let key = root.to_string_lossy().into_owned();
        file.approvals.retain(|a| a.checkout_root != key);
        file.approvals.push(Approval {
            checkout_root: key,
            digest: digest.to_string(),
            approved_at: unix_seconds(),
        });
        self.save(&file)
    }

    /// Returns whether an approval existed.
    pub fn revoke(&self, root: &Path) -> io::Result<bool> {
        let mut file = self.load()?;
        let key = root.to_string_lossy();
        let before = file.approvals.len();
        file.approvals.retain(|a| a.checkout_root != key);
        let removed = file.approvals.len() != before;
        if removed {
            self.save(&file)?;
        }
        Ok(removed)
    }

    fn load(&self) -> io::Result<TrustFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(TrustFile {
                version: 1,
                approvals: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    fn save(&self, file: &TrustFile) -> io::Result<()> {
        let text = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
        write_private_atomic(&self.path, text.as_bytes())
    }
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
