//! Real file/parent namespace operations; no runtime acknowledgement is asserted.
use super::*;
use cap_std::fs::Dir;
use std::path::PathBuf;

struct Prepared {
    _temp: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    dir: Dir,
    record: Journal,
}

impl Prepared {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir_all(root.join("existing")).unwrap();
        std::fs::write(root.join("existing/replace"), b"before").unwrap();
        std::fs::write(root.join("existing/delete"), b"deleted").unwrap();
        let store = Store::open(
            &temp.path().join("data"),
            &root.to_string_lossy(),
            super::super::Options {
                max_file: 65536,
                max_total: 4 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        let original = store.current("existing/replace").unwrap().unwrap();
        let mut after = store.object(b"after").unwrap();
        after.uid = original.uid;
        after.gid = original.gid;
        let changes = vec![
            Change {
                path: "existing/replace".into(),
                before: Some(original),
                after: Some(after.clone()),
            },
            Change {
                path: "existing/delete".into(),
                before: store.current("existing/delete").unwrap(),
                after: None,
            },
            Change {
                path: "new/deep/file".into(),
                before: None,
                after: Some(after),
            },
        ];
        let mut record = store
            .prepare_journal(changes, "restore_fixture", None)
            .unwrap();
        let dir = storage::private_child(&store.journals, "restore_fixture").unwrap();
        Store::record(&dir, &record).unwrap();
        for (index, change) in record.changes.iter().enumerate() {
            if let Some(after) = &change.after {
                let file = storage::create(
                    &dir,
                    &format!("stage_{index}"),
                    &store.bytes(after).unwrap(),
                )
                .unwrap();
                Store::stage_metadata(&file, after).unwrap();
            }
        }
        record.ownership = Some(Ownership::prepare(&store, &dir, &record.changes).unwrap());
        Store::record(&dir, &record).unwrap();
        Self {
            _temp: temp,
            root,
            store,
            dir,
            record,
        }
    }

    fn proof(&self) -> &Ownership {
        self.record.ownership.as_ref().unwrap()
    }

    fn inspect(&self) -> Result<Vec<Effect>> {
        self.proof()
            .inspect(&self.store, &self.dir, &self.record.changes)
    }

    fn apply(&self, index: usize) {
        self.store
            .install(
                &self.dir,
                index,
                &self.record.changes[index],
                self.proof(),
                &|| Ok(()),
            )
            .unwrap();
    }

    fn replace_same_bytes(&self, path: &Path, bytes: &[u8]) {
        let foreign = self.root.join("foreign");
        std::fs::write(&foreign, bytes).unwrap();
        std::fs::set_permissions(&foreign, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::rename(foreign, path).unwrap();
    }

    fn private_path(&self, name: &str) -> PathBuf {
        let key = storage::digest(
            format!("{}\0{}", self.store.root.display(), self.store.root_id).as_bytes(),
        );
        self.store
            .data_path
            .join("snapshot/global")
            .join(key)
            .join("journals/restore_fixture")
            .join(name)
    }
}

#[test]
fn missing_progress_does_not_hide_replace_delete_or_create_after_reopen() {
    let mut p = Prepared::new();
    assert_eq!(
        p.inspect().unwrap(),
        [Effect::Unapplied, Effect::Unapplied, Effect::Unapplied]
    );
    for index in 0..3 {
        p.apply(index);
    }
    assert!(p.record.applied.is_empty());
    // Reload the durable prepared record, without ever publishing progress.
    p.record = p.store.journal("restore_fixture").unwrap().1;
    assert!(!p.record.completed);
    assert!(p.record.applied.is_empty());
    assert_eq!(
        p.inspect().unwrap(),
        [Effect::Applied, Effect::Applied, Effect::Applied]
    );
    assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    assert_eq!(storage::read(&p.dir, "stage_1", 65536).unwrap(), b"deleted");
    assert_eq!(
        std::fs::read(p.root.join("new/deep/file")).unwrap(),
        b"after"
    );
    let data = p.store.data_path.clone();
    let options = p.store.options.clone();
    drop(p.store);
    p.store = Store::open(&data, &p.root.to_string_lossy(), options).unwrap();
    assert_eq!(
        p.inspect().unwrap(),
        [Effect::Applied, Effect::Applied, Effect::Applied]
    );
    assert!(p.store.ensure_recovered().is_err());
}

#[test]
fn same_byte_foreign_source_is_refused_before_and_after_replacement() {
    for applied in [false, true] {
        let p = Prepared::new();
        let bytes = if applied {
            p.apply(0);
            b"after".as_slice()
        } else {
            b"before".as_slice()
        };
        let path = p.root.join("existing/replace");
        p.replace_same_bytes(&path, bytes);
        assert_eq!(
            p.store
                .inspect_entry(
                    &p.store.parent("existing/replace").unwrap(),
                    std::ffi::OsStr::new("replace")
                )
                .unwrap(),
            if applied {
                p.record.changes[0].after.clone().unwrap()
            } else {
                p.record.changes[0].before.clone().unwrap()
            }
        );
        assert!(p.inspect().is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(p.root.join("existing/delete")).unwrap(),
            b"deleted"
        );
    }
}

#[test]
fn same_byte_foreign_preimage_and_stage_are_not_owned() {
    for applied in [false, true] {
        let p = Prepared::new();
        if applied {
            p.apply(0);
        }
        let path = p.private_path("stage_0");
        let bytes = if applied {
            b"before".as_slice()
        } else {
            b"after".as_slice()
        };
        p.replace_same_bytes(&path, bytes);
        assert!(p.inspect().is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn foreign_parent_and_symlink_to_displaced_parent_are_refused() {
    for symlink in [false, true] {
        let p = Prepared::new();
        let old = p.root.join("old-parent");
        std::fs::rename(p.root.join("existing"), &old).unwrap();
        if symlink {
            std::os::unix::fs::symlink(&old, p.root.join("existing")).unwrap();
        } else {
            std::fs::create_dir(p.root.join("existing")).unwrap();
            std::fs::copy(old.join("replace"), p.root.join("existing/replace")).unwrap();
            std::fs::copy(old.join("delete"), p.root.join("existing/delete")).unwrap();
        }
        assert!(p.inspect().is_err());
        assert!(
            p.store
                .install(&p.dir, 0, &p.record.changes[0], p.proof(), &|| Ok(()))
                .is_err()
        );
        assert_eq!(std::fs::read(old.join("replace")).unwrap(), b"before");
        assert_eq!(
            std::fs::read(p.root.join("existing/replace")).unwrap(),
            b"before"
        );
    }
}

#[test]
fn durable_parent_preparation_explains_a_cut_before_file_publication() {
    let p = Prepared::new();
    let parent = p
        .proof()
        .create_parents(&p.store, &p.dir, "new/deep/file", &|| Ok(()))
        .unwrap();
    assert!(!p.root.join("new/deep/file").exists());
    assert_eq!(
        p.inspect().unwrap(),
        [Effect::Unapplied, Effect::Unapplied, Effect::Unapplied]
    );
    assert_eq!(
        storage::dir_identity(&parent).unwrap(),
        storage::dir_identity(&p.store.parent("new/deep/file").unwrap()).unwrap()
    );
    // Equal empty directories do not replace the recorded created parents.
    std::fs::rename(p.root.join("new/deep"), p.root.join("displaced-deep")).unwrap();
    std::fs::create_dir(p.root.join("new/deep")).unwrap();
    assert!(p.inspect().is_err());
}

#[test]
fn inspection_of_changed_owner_bytes_never_populates_snapshot_objects() {
    let p = Prepared::new();
    let before = storage::names(&p.store.objects, 100).unwrap();
    std::fs::write(p.root.join("existing/replace"), b"new owner content").unwrap();
    assert!(p.inspect().is_err());
    assert_eq!(storage::names(&p.store.objects, 100).unwrap(), before);
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"new owner content"
    );
}

#[test]
fn aliases_in_source_or_private_stage_refuse_native_ownership() {
    for stage in [false, true] {
        let p = Prepared::new();
        let path = if stage {
            p.private_path("stage_0")
        } else {
            p.root.join("existing/replace")
        };
        std::fs::hard_link(path, p.root.join("alias")).unwrap();
        assert!(p.inspect().is_err());
        assert!(
            p.store
                .install(&p.dir, 0, &p.record.changes[0], p.proof(), &|| Ok(()))
                .is_err()
        );
        assert_eq!(
            std::fs::read(p.root.join("existing/replace")).unwrap(),
            b"before"
        );
    }
}

#[test]
fn malformed_prepared_inventory_does_not_acquire_effect_authority() {
    let p = Prepared::new();
    let original = serde_json::to_value(p.proof()).unwrap();
    let mut cases = vec![];
    let mut bad = original.clone();
    bad["version"] = 0.into();
    cases.push(bad);
    let mut bad = original.clone();
    bad["files"][0]["source"] = "0:0:0:1000000000".into();
    cases.push(bad);
    let mut bad = original.clone();
    bad["files"][0]["source"] = original["files"][0]["stage"].clone();
    cases.push(bad);
    let mut bad = original.clone();
    bad["parents"][0]["path"] = "../foreign".into();
    cases.push(bad);
    let mut bad = original;
    bad["parents"][1]["stage"] = "other".into();
    cases.push(bad);
    for case in cases {
        let proof: Ownership = serde_json::from_value(case).unwrap();
        assert!(proof.validate(&p.record.changes).is_err());
    }
    let mut changes = p.record.changes.clone();
    changes[0].before.as_mut().unwrap().object = "invalid-object".into();
    assert!(p.proof().validate(&changes).is_err());
}

#[test]
fn foreign_or_populated_parent_stages_refuse_before_publication() {
    for foreign in [false, true] {
        let p = Prepared::new();
        let path = p.private_path("parent_1");
        if foreign {
            std::fs::rename(&path, p.private_path("foreign-parent")).unwrap();
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(path.join("owner-file"), b"retain owner data").unwrap();
        }
        assert!(p.inspect().is_err());
        assert!(
            p.store
                .install(&p.dir, 2, &p.record.changes[2], p.proof(), &|| Ok(()))
                .is_err()
        );
        assert!(!p.root.join("new").exists());
    }
}

#[test]
fn revocation_is_rechecked_before_each_parent_and_file_publication() {
    for before_file in [false, true] {
        let p = Prepared::new();
        let stop_path = p.root.join(if before_file { "new/deep" } else { "new" });
        let authorize = || {
            if stop_path.exists() {
                Err("admission revoked".into())
            } else {
                Ok(())
            }
        };
        assert!(
            p.store
                .install(&p.dir, 2, &p.record.changes[2], p.proof(), &authorize)
                .is_err()
        );
        assert!(p.root.join("new").is_dir());
        assert_eq!(p.root.join("new/deep").is_dir(), before_file);
        assert!(!p.root.join("new/deep/file").exists());
        assert_eq!(
            p.inspect().unwrap(),
            [Effect::Unapplied, Effect::Unapplied, Effect::Unapplied]
        );
    }
    let p = Prepared::new();
    assert!(
        p.store
            .install(&p.dir, 0, &p.record.changes[0], p.proof(), &|| Err(
                "admission revoked".into()
            ))
            .is_err()
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"before"
    );
}

#[test]
fn created_parent_permission_changes_refuse_while_existing_parent_modes_are_preserved() {
    for published in [false, true] {
        let p = Prepared::new();
        let path = if published {
            p.proof()
                .create_parents(&p.store, &p.dir, "new/deep/file", &|| Ok(()))
                .unwrap();
            p.root.join("new")
        } else {
            p.private_path("parent_1")
        };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(p.inspect().is_err());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    let p = Prepared::new();
    std::fs::set_permissions(
        p.root.join("existing"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(p.inspect().is_ok());
    p.apply(0);
    assert_eq!(
        std::fs::metadata(p.root.join("existing"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn added_created_parent_attributes_refuse_without_discarding_metadata() {
    let p = Prepared::new();
    p.proof()
        .create_parents(&p.store, &p.dir, "new/deep/file", &|| Ok(()))
        .unwrap();
    let dir = std::fs::File::open(p.root.join("new")).unwrap();
    #[cfg(target_os = "linux")]
    let name = c"user.cyber_owner";
    #[cfg(target_os = "macos")]
    let name = c"com.cybercode.owner";
    rustix::fs::fsetxattr(
        &dir,
        name,
        b"owner metadata",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    assert!(p.inspect().is_err());
    let mut bytes = [0u8; 64];
    let size = rustix::fs::fgetxattr(&dir, name, bytes.as_mut_slice()).unwrap();
    assert_eq!(&bytes[..size], b"owner metadata");
    assert!(!p.root.join("new/deep/file").exists());
}

#[test]
fn unprepared_and_legacy_records_remain_readable_without_native_file_proof() {
    let mut p = Prepared::new();
    p.record.ownership = None;
    for version in [1, 3] {
        p.record.version = version;
        Store::record(&p.dir, &p.record).unwrap();
        let (_, record) = p.store.journal("restore_fixture").unwrap();
        assert!(p.store.verify_applied(&p.dir, &record).is_err());
        assert!(p.store.ensure_recovered().is_err());
    }
    p.record.completed = true;
    Store::record(&p.dir, &p.record).unwrap();
    assert!(p.store.journal("restore_fixture").is_err());
    p.record.completed = false;
    p.record.applied.push("wrong-progress".into());
    Store::record(&p.dir, &p.record).unwrap();
    assert!(p.store.journal("restore_fixture").is_err());
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"before"
    );
}

#[test]
fn native_ownership_cut_worker() {
    let Some(base) = std::env::var_os("CYBER_OWNERSHIP_CUT_BASE") else {
        return;
    };
    let base = PathBuf::from(base);
    let cut: usize = std::env::var("CYBER_OWNERSHIP_CUT_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let store = Store::open(
        &base.join("data"),
        &base.join("work").to_string_lossy(),
        super::super::Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        },
    )
    .unwrap();
    let (dir, record) = store.journal("restore_fixture").unwrap();
    let ownership = record.ownership.as_ref().unwrap();
    if cut >= 1 {
        ownership
            .create_parents(&store, &dir, "new/deep/file", &|| Ok(()))
            .unwrap();
    }
    for index in 0..cut.saturating_sub(1) {
        store
            .install(&dir, index, &record.changes[index], ownership, &|| Ok(()))
            .unwrap();
    }
    // No progress record is written. Parent observes the actual namespace before
    // killing this process, then opens the storage through a fresh owner.
    std::fs::write(base.join("cut-ready"), cut.to_string()).unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn process_kill_at_parent_and_file_cuts_recovers_ownership_without_progress_or_replay() {
    for cut in 0..5 {
        verify_killed_cut(cut);
    }
}

fn verify_killed_cut(cut: usize) {
    let mut p = Prepared::new();
    let base = p._temp.path().to_owned();
    drop(p.store);
    drop(p.dir);
    let log = std::fs::File::create(base.join("worker.log")).unwrap();
    let mut child = ChildGuard(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "copy::restore::ownership_tests::native_ownership_cut_worker",
                "--nocapture",
            ])
            .env("CYBER_OWNERSHIP_CUT_BASE", &base)
            .env("CYBER_OWNERSHIP_CUT_PHASE", cut.to_string())
            .env("PATH", "")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !base.join("cut-ready").exists() {
        assert!(
            std::time::Instant::now() < deadline && child.0.try_wait().unwrap().is_none(),
            "native worker missed cut {cut}: {}",
            std::fs::read_to_string(base.join("worker.log")).unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_live_cut(&p.root, cut);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    p.store = Store::open(
        &base.join("data"),
        &p.root.to_string_lossy(),
        super::super::Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        },
    )
    .unwrap();
    (p.dir, p.record) = p.store.journal("restore_fixture").unwrap();
    assert_recovered_cut(&p, cut);
}

fn assert_live_cut(root: &Path, cut: usize) {
    // Independently observe the real child effect; its marker alone is not proof.
    assert_eq!(root.join("new/deep").exists(), cut >= 1);
    assert_eq!(
        std::fs::read(root.join("existing/replace")).unwrap(),
        if cut >= 2 {
            b"after".as_slice()
        } else {
            b"before".as_slice()
        }
    );
    assert_eq!(root.join("existing/delete").exists(), cut < 3);
    assert_eq!(root.join("new/deep/file").exists(), cut >= 4);
}

fn assert_recovered_cut(p: &Prepared, cut: usize) {
    assert!(p.record.applied.is_empty());
    assert!(!p.record.completed);
    let effects = p.inspect().unwrap();
    for (index, effect) in effects.iter().enumerate() {
        assert_eq!(
            *effect,
            if cut >= index + 2 {
                Effect::Applied
            } else {
                Effect::Unapplied
            }
        );
    }
    assert!(p.store.ensure_recovered().is_err());
    if cut >= 2 {
        assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    }
    if cut >= 3 {
        assert_eq!(storage::read(&p.dir, "stage_1", 65536).unwrap(), b"deleted");
    }
}
