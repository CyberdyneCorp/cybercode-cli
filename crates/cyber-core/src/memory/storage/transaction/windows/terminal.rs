//! Readonly terminal ownership through receipt acknowledgement and history disposal.
use super::*;
pub(crate) struct TerminalOwnership {
    // These files remain frozen until the complete commit returns, including archival.
    _live: Vec<File>,
    journal: Vec<File>,
}
impl TerminalOwnership {
    pub(crate) fn acquire(pending: &PreparedMemory<'_, '_>) -> Result<Self, MemoryStorageError> {
        let mut live = Vec::new();
        let mut journal = Vec::new();
        for proof in objects::terminal_files(&pending.intent)? {
            let dir = if proof.journal {
                &pending.dir
            } else {
                &pending.scope.store.dir
            };
            let file =
                freeze_checked(dir, &proof.name, proof.identity, &proof.digest, proof.limit)?;
            if proof.journal {
                journal.push(file);
            } else {
                live.push(file);
            }
        }
        Ok(Self {
            _live: live,
            journal,
        })
    }
    pub(crate) fn marker(&mut self, dir: &Dir) -> Result<(), MemoryStorageError> {
        let current =
            optional_file(dir, "completed")?.ok_or(MemoryStorageError::RecoveryRequired)?;
        let identity = native::identity(&current)?;
        let file = freeze_checked(dir, "completed", identity, &hash(b"committed\n"), 10)?;
        self.journal.push(file);
        Ok(())
    }
    pub(crate) fn release_journal(&mut self) {
        // Frozen descendants must close before exclusive directory rename.
        self.journal.clear();
    }
}
fn freeze_checked(
    dir: &Dir,
    name: &str,
    identity: native::FileIdentity,
    digest: &str,
    limit: u64,
) -> Result<File, MemoryStorageError> {
    let mut file = native::freeze_private_file(&dir.try_clone()?.into_std_file(), name, identity)?;
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit || hash(&bytes) != digest {
        return Err(MemoryStorageError::Conflict);
    }
    Ok(file)
}
