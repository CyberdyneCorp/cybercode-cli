//! Cross-process checkout use with explicit settlement and conservative crash recovery.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{GitExecution, Managed, Name, RemovalActivity, Repository, RepositoryLock};

#[derive(Debug, Serialize, Deserialize)]
struct UseRecord {
    worktree_id: String,
    session_id: String,
    settled: bool,
}

/// Dropping without `settle` releases the OS lock but retains unknown activity.
/// The caller must settle its owned work/process trees before calling `settle`.
pub struct CheckoutLease {
    file: File,
    record: UseRecord,
}

impl CheckoutLease {
    pub fn settle(self) -> io::Result<()> {
        self.settle_retained().map(drop)
    }

    /// Record acknowledgement while retaining the native lock through a later commit.
    pub fn settle_retained(mut self) -> io::Result<Self> {
        self.record.settled = true;
        write_record(&mut self.file, &self.record)?;
        Ok(self)
    }
}

impl Drop for CheckoutLease {
    fn drop(&mut self) {
        // Closing alone can leave a transient inherited descriptor holding the lock.
        // An unsettled record still refuses admission after this explicit release.
        let _ = self.file.unlock();
    }
}

/// Real removal admission. The repository lock must remain held for this guard's
/// lifetime; `Repository::remove` supplies it and prevents new lease creation.
#[derive(Default)]
pub struct CheckoutActivity;

struct RemovalFence {
    _files: Vec<File>,
}

impl Drop for RemovalFence {
    fn drop(&mut self) {
        for file in &self._files {
            let _ = file.unlock();
        }
    }
}

impl RemovalActivity for CheckoutActivity {
    fn reserve(&self, managed: &Managed) -> io::Result<Box<dyn Send>> {
        let directory = use_directory(managed, false)?;
        if !directory.exists() {
            return Ok(Box::new(RemovalFence { _files: Vec::new() }));
        }
        let mut paths = std::fs::read_dir(&directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let session = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("Invalid activity filename"))?;
            safe_id(session, "ses")?;
            if path.extension().is_none_or(|extension| extension != "lock") {
                return Err(invalid("Unexpected activity record"));
            }
            let mut file = open_record(&path, false)?;
            if !try_lock(&file)? {
                return Err(io::Error::other(format!("Worktree in use by {session}")));
            }
            let record = read_record(&mut file)?;
            check_record(&record, managed, session)?;
            if !record.settled {
                return Err(unknown(session));
            }
            files.push(file);
        }
        Ok(Box::new(RemovalFence { _files: files }))
    }
}

impl Repository {
    /// Admit Location use under the same lock as removal and verify exact ready
    /// ownership before creating a persistent per-Session activity lock.
    pub async fn claim(
        &self,
        execution: &dyn GitExecution,
        managed: &Managed,
        session: &str,
    ) -> io::Result<CheckoutLease> {
        safe_id(session, "ses")?;
        safe_id(&managed.id, "wt")?;
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
        })?;
        if managed.common_dir != self.common_dir {
            return Err(invalid("Worktree belongs to another repository"));
        }
        let name = Name::parse(&managed.name).map_err(invalid)?;
        let record = self
            .common_dir
            .join("cyber-worktrees")
            .join(format!("{}.json", name.as_str()));
        let super::ListedWorktree::Ready(stored) = self
            .inspect_record(execution, &record, name.as_str())
            .await?
        else {
            return Err(invalid("Worktree requires ready ownership"));
        };
        if stored != *managed {
            return Err(invalid("Worktree creation identity changed"));
        }
        let path = use_directory(managed, true)?.join(format!("{session}.lock"));
        let existed = path.try_exists()?;
        let mut file = open_record(&path, true)?;
        if !try_lock(&file)? {
            return Err(io::Error::other(format!("Worktree in use by {session}")));
        }
        if existed {
            let record = read_record(&mut file)?;
            check_record(&record, managed, session)?;
            if !record.settled {
                return Err(unknown(session));
            }
        }
        let record = UseRecord {
            worktree_id: managed.id.clone(),
            session_id: session.into(),
            settled: false,
        };
        write_record(&mut file, &record)?;
        sync_parent(&path)?;
        Ok(CheckoutLease { file, record })
    }
}

fn safe_id(id: &str, prefix: &str) -> io::Result<()> {
    if !crate::ids::has_prefix(id, prefix)
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(invalid("Invalid activity identity"));
    }
    Ok(())
}

fn use_directory(managed: &Managed, create: bool) -> io::Result<PathBuf> {
    safe_id(&managed.id, "wt")?;
    let root = managed.common_dir.join("cyber-worktree-activity");
    regular_directory(&root, create)?;
    let directory = root.join(&managed.id);
    regular_directory(&directory, create)?;
    Ok(directory)
}

fn regular_directory(path: &Path, create: bool) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("Activity directory is not a regular directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if create {
                std::fs::create_dir(path)?;
                sync_parent(path)?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn open_record(path: &Path, create: bool) -> io::Result<File> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            OpenOptions::new().read(true).write(true).open(path)
        }
        Ok(_) => Err(invalid("Activity record is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path),
        Err(error) => Err(error),
    }
}

fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

fn read_record(file: &mut File) -> io::Result<UseRecord> {
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("Activity record exceeds 4 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(invalid)
}

fn check_record(record: &UseRecord, managed: &Managed, session: &str) -> io::Result<()> {
    if record.worktree_id != managed.id || record.session_id != session {
        return Err(invalid("Activity record identity does not match"));
    }
    Ok(())
}

fn write_record(file: &mut File, record: &UseRecord) -> io::Result<()> {
    let bytes = serde_json::to_vec(record).map_err(invalid)?;
    file.rewind()?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| invalid("Missing activity parent"))?,
    )?
    .sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn unknown(session: &str) -> io::Error {
    io::Error::other(format!(
        "Worktree activity outcome unknown for {session}; recovery is required"
    ))
}
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_settlement_keeps_the_native_lock_until_proof_disposal() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lease.lock");
        let mut file = open_record(&path, true).unwrap();
        assert!(try_lock(&file).unwrap());
        let record = UseRecord {
            worktree_id: "wt_test".into(),
            session_id: "ses_test".into(),
            settled: false,
        };
        write_record(&mut file, &record).unwrap();
        let proof = CheckoutLease { file, record }.settle_retained().unwrap();
        let mut probe = open_record(&path, false).unwrap();
        assert!(read_record(&mut probe).unwrap().settled);
        assert!(!try_lock(&probe).unwrap());
        drop(proof);
        assert!(try_lock(&probe).unwrap());
        assert!(read_record(&mut probe).unwrap().settled);
        probe.unlock().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_retained_settlement_preserves_unknown_native_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lease.lock");
        let mut initial = open_record(&path, true).unwrap();
        let record = UseRecord {
            worktree_id: "wt_test".into(),
            session_id: "ses_test".into(),
            settled: false,
        };
        write_record(&mut initial, &record).unwrap();
        drop(initial);
        let file = File::open(&path).unwrap();
        assert!(try_lock(&file).unwrap());
        assert!(CheckoutLease { file, record }.settle_retained().is_err());
        let mut probe = open_record(&path, false).unwrap();
        assert!(try_lock(&probe).unwrap());
        assert!(!read_record(&mut probe).unwrap().settled);
        probe.unlock().unwrap();
    }

    #[test]
    fn explicit_release_does_not_depend_on_closing_duplicate_descriptors() {
        for settle in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("lease.lock");
            let mut file = open_record(&path, true).unwrap();
            assert!(try_lock(&file).unwrap());
            let duplicate = file.try_clone().unwrap();
            let record = UseRecord {
                worktree_id: "wt_test".into(),
                session_id: "ses_test".into(),
                settled: false,
            };
            write_record(&mut file, &record).unwrap();
            let lease = CheckoutLease { file, record };
            if settle {
                lease.settle().unwrap();
            } else {
                drop(lease);
            }
            let mut probe = open_record(&path, false).unwrap();
            assert!(try_lock(&probe).unwrap());
            assert_eq!(read_record(&mut probe).unwrap().settled, settle);
            probe.unlock().unwrap();
            drop(duplicate);
        }
    }
}
