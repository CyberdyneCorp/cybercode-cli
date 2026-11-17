//! Two preallocated intent inodes; torn spare bytes never replace the active intent.
use super::{restore::Journal, storage};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
};
use storage::{Result, fail};

pub(super) const ACTIVE: &str = "intent.json";
pub(super) const SPARE: &str = "intent.next";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    version: u32,
    format: u32,
    first: String,
    second: String,
}
impl Binding {
    pub fn budget(format: u32) -> Self {
        Self {
            version: 1,
            format,
            first: "0".repeat(storage::IDENTITY_MAX_BYTES),
            second: "1".repeat(storage::IDENTITY_MAX_BYTES),
        }
    }
    pub fn format(&self) -> Result<u32> {
        if self.version != 1
            || !matches!(self.format, 3 | 5 | 6)
            || !super::ownership::valid_identity(&self.first)
            || !super::ownership::valid_identity(&self.second)
            || self.first == self.second
        {
            return Err("Invalid intent slot binding".into());
        }
        Ok(self.format)
    }
    fn matches(&self, first: &File, second: &File) -> Result<()> {
        self.format()?;
        storage::validate_lock(first)?;
        storage::validate_lock(second)?;
        let (a, b) = (storage::identity(first)?, storage::identity(second)?);
        if !((a == self.first && b == self.second) || (a == self.second && b == self.first)) {
            return Err("Intent slot identity changed".into());
        }
        Ok(())
    }
    pub fn verify(&self, dir: &Dir) -> Result<()> {
        self.matches(
            &storage::open(dir, Path::new(ACTIVE))?,
            &storage::open(dir, Path::new(SPARE))?,
        )
    }
}

/// Retain the actual creating descriptors until the prepared binding is published.
/// A cut before publication remains non-admitted construction evidence.
pub(super) struct Construction {
    active: File,
    spare: File,
    active_generation: String,
    spare_generation: String,
}
impl Construction {
    pub fn create(dir: &Dir, journal: &Journal) -> Result<Self> {
        // Keep the initial intent absent until its complete bytes are durable,
        // preserving pre-admission cancellation semantics for a torn creation.
        let temporary = format!("tmp_{}", ulid::Ulid::new());
        let active = storage::create(dir, &temporary, &encoded(journal)?)?;
        rustix::fs::renameat_with(
            dir,
            &temporary,
            dir,
            ACTIVE,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(fail)?;
        storage::sync(dir)?;
        let spare = storage::create(dir, SPARE, b"")?;
        storage::sync(dir)?;
        Ok(Self {
            active_generation: super::ownership::generation(&active)?,
            spare_generation: super::ownership::generation(&spare)?,
            active,
            spare,
        })
    }
    pub fn binding(&self, dir: &Dir, format: u32) -> Result<Binding> {
        if super::ownership::generation(&self.active)? != self.active_generation
            || super::ownership::generation(&self.spare)? != self.spare_generation
        {
            return Err("Intent construction metadata changed".into());
        }
        let binding = Binding {
            version: 1,
            format,
            first: storage::identity(&self.active)?,
            second: storage::identity(&self.spare)?,
        };
        binding.verify(dir)?;
        Ok(binding)
    }
    pub fn publish(&self, dir: &Dir, journal: &Journal) -> Result<()> {
        let expected = self.binding(dir, journal.format_version()?)?;
        if journal.updates.as_ref() != Some(&expected) {
            return Err("Prepared intent differs from its creating descriptors".into());
        }
        write_bound(dir, journal, true, |_| Ok(()))
    }
}
fn encoded(journal: &Journal) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(journal).map_err(fail)?;
    if bytes.len() as u64 > super::RECORD_LIMIT {
        return Err("Restore journal exceeds limit".into());
    }
    Ok(bytes)
}
fn writable(dir: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK);
    let file = dir.open_with(name, &options).map_err(fail)?.into_std();
    storage::validate_lock(&file)?;
    Ok(file)
}
pub(super) fn write(dir: &Dir, journal: &Journal) -> Result<()> {
    write_before_exchange(dir, journal, |_| Ok(()))
}
fn write_before_exchange(
    dir: &Dir,
    journal: &Journal,
    before_exchange: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    write_bound(dir, journal, false, before_exchange)
}
fn write_bound(
    dir: &Dir,
    journal: &Journal,
    initializing: bool,
    before_exchange: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    journal.format_version()?;
    let binding = journal
        .updates
        .as_ref()
        .ok_or("Missing intent slot binding")?;
    let active = writable(dir, ACTIVE)?;
    let mut spare = writable(dir, SPARE)?;
    binding.matches(&active, &spare)?;
    if !initializing {
        let current: Journal =
            serde_json::from_slice(&storage::read(dir, ACTIVE, super::RECORD_LIMIT)?)
                .map_err(fail)?;
        current.format_version()?;
        if current.version != 7
            || current.updates.as_ref() != Some(binding)
            || current.origin != journal.origin
        {
            return Err("Active intent no longer owns these update slots".into());
        }
    }
    let bytes = encoded(journal)?;
    spare.seek(SeekFrom::Start(0)).map_err(fail)?;
    spare.set_len(0).map_err(fail)?;
    spare.write_all(&bytes).map_err(fail)?;
    before_exchange(&mut spare)?;
    storage::sync_file(&spare)?;
    binding.verify(dir)?;
    rustix::fs::renameat_with(dir, ACTIVE, dir, SPARE, rustix::fs::RenameFlags::EXCHANGE)
        .map_err(fail)?;
    storage::sync(dir)?;
    binding.verify(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy::{Options, Store, now};
    use cyber_server::runtime::{SnapshotBinding, SnapshotOrigin};
    const ID: &str = "restore_slot_fixture";
    fn options() -> Options {
        Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        }
    }
    fn fixture() -> (tempfile::TempDir, Store, Dir, Journal) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("owner"), b"owner bytes").unwrap();
        let store =
            Store::open(&temp.path().join("data"), root.to_str().unwrap(), options()).unwrap();
        let dir = storage::private_child(&store.journals, ID).unwrap();
        let mut journal = Journal {
            version: 3,
            updates: None,
            backend: None,
            location: store.root.clone(),
            root: store.root_id.clone(),
            created: now(),
            completed: false,
            changes: vec![],
            applied: vec![],
            acknowledged: false,
            settled_by: None,
            ownership: None,
            context: None,
            resolution: None,
            origin: Some(SnapshotOrigin {
                version: 1,
                activity_id: "op_slots".into(),
                session_id: "ses_slots".into(),
                session_created_id: "evt_slots".into(),
                worktree_id: None,
                binding: SnapshotBinding {
                    directory: store.root.clone(),
                    root_identity: store.root_id.clone(),
                    namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                    journal_id: ID.into(),
                    journal_identity: storage::dir_identity(&dir).unwrap(),
                    lock_identity: Some(store.lock_identity().unwrap()),
                },
            }),
        };
        let construction = Construction::create(&dir, &journal).unwrap();
        journal.ownership =
            Some(crate::copy::ownership::Ownership::prepare(&store, &dir, &[]).unwrap());
        journal.updates = Some(construction.binding(&dir, journal.version).unwrap());
        journal.version = 7;
        construction.publish(&dir, &journal).unwrap();
        (temp, store, dir, journal)
    }
    #[test]
    fn repeated_updates_keep_two_bound_inodes_and_the_immutable_digest() {
        let (_temp, store, dir, mut journal) = fixture();
        let before = store.worker_digest(journal.origin.as_ref()).unwrap();
        let binding = journal.updates.clone();
        for _ in 0..8 {
            journal.completed = !journal.completed;
            Store::record(&dir, &journal).unwrap();
            let (_, reopened) = store.journal(ID).unwrap();
            assert_eq!(reopened.completed, journal.completed);
            assert_eq!(reopened.updates, binding);
            assert_eq!(
                store.worker_digest(journal.origin.as_ref()).unwrap(),
                before
            );
            assert_eq!(storage::names(&dir, 10).unwrap(), [ACTIVE, SPARE]);
        }
    }
    #[test]
    fn slot_native_identities_are_immutable_receipt_input() {
        let (_temp, _store, _dir, journal) = fixture();
        let digest = crate::copy::resolution::immutable_digest(&journal).unwrap();
        let mut changed = journal.clone();
        changed.updates.as_mut().unwrap().first = "1:2:3:4".into();
        assert_ne!(
            crate::copy::resolution::immutable_digest(&changed).unwrap(),
            digest
        );
    }
    #[test]
    fn equal_byte_foreign_slot_missing_route_and_alias_refuse_without_writes() {
        let (temp, _store, dir, journal) = fixture();
        let original = storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap();
        let spare = storage::read(&dir, SPARE, super::super::RECORD_LIMIT).unwrap();
        let path = temp.path().join("retained-spare");
        dir.rename(
            SPARE,
            &Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap(),
            "retained-spare",
        )
        .unwrap();
        assert!(write(&dir, &journal).is_err());
        storage::create(&dir, SPARE, &spare).unwrap();
        assert!(
            write(&dir, &journal)
                .unwrap_err()
                .contains("identity changed")
        );
        assert_eq!(
            storage::read(&dir, SPARE, super::super::RECORD_LIMIT).unwrap(),
            spare
        );
        dir.remove_file(SPARE).unwrap();
        rustix::fs::symlinkat(&path, &dir, SPARE).unwrap();
        assert!(write(&dir, &journal).is_err());
        assert_eq!(
            storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap(),
            original
        );
        assert_eq!(std::fs::read(path).unwrap(), spare);
    }
    #[test]
    fn legacy_records_cannot_claim_a_spare_by_name_or_partial_binding() {
        let (_temp, store, dir, mut journal) = fixture();
        journal.version = 3;
        let before = storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap();
        assert!(Store::record(&dir, &journal).is_err());
        assert_eq!(
            storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap(),
            before
        );
        let bytes = serde_json::to_vec(&journal).unwrap();
        // Test corruption only: overwrite the current inode instead of invoking
        // the guarded production writer with an invalid version/binding pair.
        let mut active = writable(&dir, ACTIVE).unwrap();
        active.set_len(0).unwrap();
        active.write_all(&bytes).unwrap();
        storage::sync_file(&active).unwrap();
        assert!(store.journal(ID).is_err());
        journal.version = 7;
        journal.updates = None;
        assert!(Store::record(&dir, &journal).is_err());
        assert_eq!(storage::names(&dir, 10).unwrap(), [ACTIVE, SPARE]);
        active.seek(SeekFrom::Start(0)).unwrap();
        active.set_len(0).unwrap();
        active
            .write_all(&serde_json::to_vec(&journal).unwrap())
            .unwrap();
        storage::sync_file(&active).unwrap();
        assert!(store.journal(ID).is_err());
    }
    #[test]
    #[ignore = "subprocess fixture invoked by torn-spare process proof"]
    fn intent_update_child() {
        let root = std::path::PathBuf::from(std::env::var_os("CYBER_INTENT_SLOT_CHILD").unwrap());
        let store = Store::recover_existing(
            &root.join("data"),
            root.join("work").to_str().unwrap(),
            options(),
        )
        .unwrap();
        let (dir, mut journal) = store.journal(ID).unwrap();
        journal.completed = true;
        let _ = write_before_exchange(&dir, &journal, |spare| {
            spare.seek(SeekFrom::Start(0)).map_err(fail)?;
            spare.set_len(1).map_err(fail)?;
            spare.write_all(b"{").map_err(fail)?;
            storage::sync_file(spare)?;
            std::fs::write(root.join("ready"), b"partial spare, active unchanged").map_err(fail)?;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        panic!("child passed the cut");
    }
    #[test]
    fn process_death_during_spare_write_preserves_active_intent_and_reopens_without_replay() {
        use std::os::unix::process::ExitStatusExt;
        let (temp, store, dir, journal) = fixture();
        let expected = storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap();
        let binding = journal.updates.clone();
        drop(dir);
        drop(store);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "copy::intent_slots::tests::intent_update_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CYBER_INTENT_SLOT_CHILD", temp.path())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let ready = loop {
            if temp.path().join("ready").exists() {
                break true;
            }
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert!(ready, "child did not reach the cut: {status}");
        assert_eq!(status.signal(), Some(9));
        let store = Store::recover_existing(
            &temp.path().join("data"),
            temp.path().join("work").to_str().unwrap(),
            options(),
        )
        .unwrap();
        let (dir, mut reopened) = store.journal(ID).unwrap();
        assert!(!reopened.completed);
        assert_eq!(
            storage::read(&dir, ACTIVE, super::super::RECORD_LIMIT).unwrap(),
            expected
        );
        assert_eq!(storage::read(&dir, SPARE, 100).unwrap(), b"{");
        reopened.completed = true;
        Store::record(&dir, &reopened).unwrap();
        assert!(store.journal(ID).unwrap().1.completed);
        assert_eq!(store.journal(ID).unwrap().1.updates, binding);
        assert_eq!(
            std::fs::read(temp.path().join("work/owner")).unwrap(),
            b"owner bytes"
        );
        assert_eq!(storage::names(&dir, 10).unwrap(), [ACTIVE, SPARE]);
    }
}
