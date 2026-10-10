//! Actual owned-process death while filesystem and SQLite evidence are paired.
use super::*;
use std::{
    io::Write,
    process::Child,
    time::{Duration, Instant},
};

const ROOT: &str = "CYBER_TEST_PAIRED_MEMORY_ROOT";
const PHASE: &str = "CYBER_TEST_PAIRED_MEMORY_PHASE";
const DELETE: &str = "CYBER_TEST_PAIRED_MEMORY_DELETE";
const CONTENT: &str = "---\nname: policy\ndescription: Policy\ntype: user\n---\nFact\n";

fn database(root: &Path) -> Arc<Store> {
    let db = Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(root.join("events.db")),
        crate::runtime::Runtime::registry(),
    ))
    .unwrap();
    assert_eq!(db.durability(), cyber_store::Durability::Full);
    Arc::new(db)
}

fn request(root: &Path, deleted: bool) -> MemoryWrite<'_> {
    MemoryWrite {
        directory: root,
        project_id: "global",
        name: "policy",
        deleted,
        identity: Some("paired-death-original"),
        content: if deleted { "" } else { CONTENT },
        http_hash: None,
    }
}

fn synced(root: &Path, name: &str, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name))
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn boundary(root: &Path, phase: &str) {
    if std::env::var(PHASE).unwrap() != phase {
        return;
    }
    synced(root, "ready.tmp", phase.as_bytes());
    std::fs::rename(root.join("ready.tmp"), root.join("ready")).unwrap();
    loop {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn paired_memory_owner_child() {
    let Some(root) = std::env::var_os(ROOT) else {
        return;
    };
    let root = std::fs::canonicalize(PathBuf::from(root)).unwrap();
    let deleted = std::env::var_os(DELETE).is_some();
    let db = database(&root);
    let memory = MemoryStore::open(&root, "global").unwrap();
    let mut scope = memory.claim().unwrap();
    scope.write(&CONTENT.replace("Fact", "Original")).unwrap();
    let MemoryAdmission::Owned(mut owner) =
        admit(db.clone(), Bus::new(), request(&root, deleted)).unwrap()
    else {
        panic!("fresh owner");
    };
    let prepared = if deleted {
        scope.prepare_delete("policy").unwrap()
    } else {
        scope.prepare_write(CONTENT).unwrap()
    };
    let journal = prepared.journal_identity().unwrap();
    owner.bind_journal(journal.clone()).unwrap();
    synced(&root, "journal", &serde_json::to_vec(&journal).unwrap());
    boundary(&root, "bound");
    prepared
        .commit_with_acknowledgement(|receipt| {
            boundary(&root, "before-ack");
            let change = owner.finish(receipt.clone()).unwrap();
            assert_eq!(
                lookup(&db, &request(&root, deleted)).unwrap(),
                Some(change.clone())
            );
            synced(&root, "receipt", &serde_json::to_vec(&change).unwrap());
            boundary(&root, "after-ack");
            Ok(())
        })
        .unwrap();
    boundary(&root, "archived");
    panic!("paired owner missed its interruption boundary");
}

struct Owner(Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(root: &Path, phase: &str, deleted: bool) -> Owner {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "runtime::memory::recovery::tests::death::paired_memory_owner_child",
            "--nocapture",
        ])
        .env(ROOT, root)
        .env(PHASE, phase)
        .stdout(std::process::Stdio::null());
    if deleted {
        command.env(DELETE, "yes");
    }
    let mut owner = Owner(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join("ready").exists() {
        assert!(
            owner.0.try_wait().unwrap().is_none(),
            "paired owner exited before {phase}"
        );
        assert!(
            Instant::now() < deadline,
            "paired owner did not reach {phase}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(root.join("ready")).unwrap(), phase.as_bytes());
    owner
}

fn assert_outcome(memory: &MemoryStore, scope: &mut MemoryScope<'_>, deleted: bool) {
    assert!(scope.inspect_recovery().unwrap().is_none());
    if deleted {
        assert!(scope.list().unwrap().memories.is_empty());
        assert!(scope.index().unwrap().text.is_empty());
    } else {
        assert_eq!(scope.read("policy").unwrap().body, "Fact");
    }
    assert_eq!(
        std::fs::read_dir(memory.path().join(".memory-history"))
            .unwrap()
            .count(),
        2
    );
}

fn reconcile(
    root: &Path,
    phase: &str,
    deleted: bool,
    journal: &MemoryJournalIdentity,
) -> MemoryChange {
    let db = database(root);
    let memory = MemoryStore::existing(root, "global").unwrap().unwrap();
    let mut scope = memory.claim().unwrap();
    let bus = Bus::new();
    let mut live = bus.subscribe();
    let change = if let Some(storage) = scope.inspect_recovery().unwrap() {
        assert_eq!(storage.journal, *journal);
        let admission = review(&db, root, "global", journal).unwrap();
        assert_eq!(admission.completed.is_some(), phase == "after-ack");
        recover(db.clone(), bus, &mut scope, &storage, &admission).unwrap()
    } else {
        assert_eq!(phase, "archived");
        lookup(&db, &request(root, deleted)).unwrap().unwrap()
    };
    assert_eq!(change.receipt, journal.receipt);
    if matches!(phase, "bound" | "before-ack") {
        assert!(matches!(
            live.try_recv().unwrap(),
            LiveEvent::MemoryUpdated { .. }
        ));
    }
    assert!(live.try_recv().is_err());
    assert_eq!(
        db.read_events(&change.id, -1, 20)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == super::super::super::UPDATED)
            .count(),
        1
    );
    assert_outcome(&memory, &mut scope, deleted);
    change
}

fn kill_and_recover(phase: &str, deleted: bool) {
    eprintln!("paired owner death: {phase}, deleted={deleted}");
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let mut owner = spawn(&root, phase, deleted);
    let journal: MemoryJournalIdentity =
        serde_json::from_slice(&std::fs::read(root.join("journal")).unwrap()).unwrap();
    let memory = MemoryStore::existing(&root, "global").unwrap().unwrap();
    assert!(matches!(memory.claim(), Err(MemoryStorageError::Busy)));
    let db = database(&root);
    let admitted = review(&db, &root, "global", &journal).unwrap();
    assert_eq!(
        admitted.completed.is_some(),
        matches!(phase, "after-ack" | "archived")
    );
    drop(db);
    drop(memory);
    owner.0.kill().unwrap();
    assert!(!owner.0.wait().unwrap().success());
    let change = reconcile(&root, phase, deleted, &journal);
    if matches!(phase, "after-ack" | "archived") {
        let witnessed: MemoryChange =
            serde_json::from_slice(&std::fs::read(root.join("receipt")).unwrap()).unwrap();
        assert_eq!(change, witnessed);
    }
    let db = database(&root);
    let bus = Bus::new();
    let mut live = bus.subscribe();
    let before = db.read_events(&change.id, -1, 20).unwrap().events.len();
    let MemoryAdmission::Replay(replay) = admit(db.clone(), bus, request(&root, deleted)).unwrap()
    else {
        panic!("durable replay");
    };
    assert_eq!(replay, change);
    assert_eq!(
        db.read_events(&change.id, -1, 20).unwrap().events.len(),
        before
    );
    assert!(live.try_recv().is_err());
    let memory = MemoryStore::existing(&root, "global").unwrap().unwrap();
    let mut scope = memory.claim().unwrap();
    assert_outcome(&memory, &mut scope, deleted);
}

#[test]
fn paired_write_delete_owner_death_recovers_binding_acknowledgement_and_archive() {
    for deleted in [false, true] {
        for phase in ["bound", "before-ack", "after-ack", "archived"] {
            kill_and_recover(phase, deleted);
        }
    }
}
