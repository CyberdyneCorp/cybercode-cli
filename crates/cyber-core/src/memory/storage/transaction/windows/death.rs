//! Actual write/delete interruption points; the test witness grants no database authority.
use super::*;
use std::path::{Path, PathBuf};
const DATA: &str = "CYBER_MEMORY_BOUNDARY_OWNER_DATA";
const PHASE: &str = "CYBER_MEMORY_BOUNDARY_OWNER_PHASE";
const DELETE: &str = "CYBER_MEMORY_BOUNDARY_OWNER_DELETE";
thread_local! { static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[derive(Serialize, Deserialize)]
struct Witness {
    intent: Intent,
    journal: native::FileIdentity,
    intent_file: native::FileIdentity,
    marker: Option<native::FileIdentity>,
    note: Option<native::FileIdentity>,
    index: Option<native::FileIdentity>,
}
fn optional_id(dir: &Dir, name: &str) -> Result<Option<native::FileIdentity>, MemoryStorageError> {
    optional_file(dir, name)?
        .map(|file| native::identity(&file))
        .transpose()
}
fn owned_fixture(root: &Dir) -> Result<Option<PathBuf>, MemoryStorageError> {
    if !ACTIVE.get() {
        return Ok(None);
    }
    let Some(data) = std::env::var_os(DATA) else {
        return Ok(None);
    };
    let data = PathBuf::from(data);
    let expected =
        MemoryStore::existing(&data, "global")?.ok_or(MemoryStorageError::RecoveryRequired)?;
    if native::identity(&root.try_clone()?.into_std_file())?
        != native::identity(&expected.dir.try_clone()?.into_std_file())?
    {
        return Ok(None);
    }
    Ok(Some(data))
}
fn folder(root: &Dir, data: &Path, phase: &str) -> Result<Dir, MemoryStorageError> {
    if phase != "archived" {
        return existing_private_directory(root, TRANSACTION)?
            .ok_or(MemoryStorageError::RecoveryRequired);
    }
    let prepared: Witness =
        serde_json::from_slice(&std::fs::read(data.join("note-prepared-witness"))?)
            .map_err(|_| MemoryStorageError::RecoveryRequired)?;
    let history =
        existing_private_directory(root, HISTORY)?.ok_or(MemoryStorageError::RecoveryRequired)?;
    existing_private_directory(&history, &prepared.intent.id)?
        .ok_or(MemoryStorageError::RecoveryRequired)
}
pub(crate) fn barrier(phase: &str, root: &Dir) -> Result<(), MemoryStorageError> {
    if !ACTIVE.get() {
        return Ok(());
    }
    let selected = std::env::var(PHASE).ok().as_deref() == Some(phase);
    if !selected && phase != "prepared" {
        return Ok(());
    }
    let Some(data) = owned_fixture(root)? else {
        return Ok(());
    };
    let folder = folder(root, &data, phase)?;
    let intent = serde_json::from_slice(
        &optional_bytes(&folder, "intent.json", INTENT_LIMIT)?
            .ok_or(MemoryStorageError::RecoveryRequired)?,
    )
    .map_err(|_| MemoryStorageError::RecoveryRequired)?;
    let witness = Witness {
        intent,
        journal: native::identity(&folder.try_clone()?.into_std_file())?,
        intent_file: optional_id(&folder, "intent.json")?
            .ok_or(MemoryStorageError::RecoveryRequired)?,
        marker: optional_id(&folder, "completed")?,
        note: optional_id(root, "rule.md")?,
        index: optional_id(root, "MEMORY.md")?,
    };
    drop(folder);
    if phase == "prepared" {
        write_witness(&data.join("note-prepared-witness"), &witness);
    }
    if !selected {
        return Ok(());
    }
    write_witness(&data.join("note-boundary-witness"), &witness);
    std::fs::write(data.join("note-boundary-ready"), b"ready")?;
    loop {
        std::thread::park();
    }
}
#[test]
fn native_note_boundary_owner_child() {
    let Some(data) = std::env::var_os(DATA) else {
        return;
    };
    let store = MemoryStore::open(Path::new(&data), "global").unwrap();
    let mut scope = store.claim().unwrap();
    prepare(&mut scope, "Before boundary death")
        .commit()
        .unwrap();
    ACTIVE.set(true);
    let pending = if std::env::var_os(DELETE).is_some() {
        scope.prepare("rule", None, None, None).unwrap()
    } else {
        prepare(&mut scope, "After boundary death")
    };
    pending.commit().unwrap();
    panic!("note owner missed configured interruption boundary");
}
fn inspect_live(data: &Path, phase: &str, witness: &Witness) {
    let store = MemoryStore::existing(data, "global").unwrap().unwrap();
    assert!(matches!(store.claim(), Err(MemoryStorageError::Busy)));
    let note = target_ids(&witness.intent, "note").unwrap();
    let index = target_ids(&witness.intent, "index").unwrap();
    let expected_note = match phase {
        "prepared" => note.0,
        "note-captured" => None,
        _ => note.1,
    };
    let expected_index = match phase {
        "prepared" | "note-captured" | "note-installed" => index.0,
        "index-captured" => None,
        _ => index.1,
    };
    assert_eq!(witness.note, expected_note);
    assert_eq!(witness.index, expected_index);
    assert_eq!(optional_id(&store.dir, "rule.md").unwrap(), expected_note);
    assert_eq!(
        optional_id(&store.dir, "MEMORY.md").unwrap(),
        expected_index
    );
    if matches!(phase, "completed" | "released" | "archived") {
        assert_frozen(&store.dir, &store.path().join("MEMORY.md"), "MEMORY.md");
        if expected_note.is_some() {
            assert_frozen(&store.dir, &store.path().join("rule.md"), "rule.md");
        }
    }
}
fn inspect_recovered(
    store: &MemoryStore,
    scope: &mut MemoryScope<'_>,
    witness: &Witness,
    deleted: bool,
) {
    let note = target_ids(&witness.intent, "note").unwrap();
    let index = target_ids(&witness.intent, "index").unwrap();
    assert_eq!(optional_id(&store.dir, "rule.md").unwrap(), note.1);
    assert_eq!(optional_id(&store.dir, "MEMORY.md").unwrap(), index.1);
    if deleted {
        assert!(scope.list().unwrap().memories.is_empty());
        assert_eq!(scope.index().unwrap().text, "");
    } else {
        assert_eq!(scope.read("rule").unwrap().body, "After boundary death");
    }
    let history = existing_private_directory(&store.dir, HISTORY)
        .unwrap()
        .unwrap();
    let archived = existing_private_directory(&history, &witness.intent.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        native::identity(&archived.try_clone().unwrap().into_std_file()).unwrap(),
        witness.journal
    );
    assert_eq!(
        optional_id(&archived, "intent.json").unwrap(),
        Some(witness.intent_file)
    );
    assert_eq!(optional_id(&archived, "note.before").unwrap(), note.0);
    assert_eq!(optional_id(&archived, "index.before").unwrap(), index.0);
    if let Some(marker) = witness.marker {
        assert_eq!(optional_id(&archived, "completed").unwrap(), Some(marker));
    }
    assert_eq!(history.entries().unwrap().count(), 2);
    assert!(scope.read_prepared().unwrap().is_none());
}
fn kill_note_owner(phase: &str, deleted: bool) {
    eprintln!("note owner boundary: phase={phase}, deleted={deleted}");
    let data = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "memory::storage::transaction::windows::tests::death::native_note_boundary_owner_child",
            "--nocapture",
        ])
        .env(DATA, data.path())
        .env(PHASE, phase)
        .stdout(std::process::Stdio::null());
    if deleted {
        command.env(DELETE, "yes");
    }
    let mut owner = Owner(command.spawn().unwrap());
    wait_ready(&mut owner, &data.path().join("note-boundary-ready"));
    let witness: Witness =
        serde_json::from_slice(&std::fs::read(data.path().join("note-boundary-witness")).unwrap())
            .unwrap();
    inspect_live(data.path(), phase, &witness);
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let store = MemoryStore::existing(data.path(), "global")
        .unwrap()
        .unwrap();
    let mut scope = store.claim().unwrap();
    if let Some(pending) = scope.read_prepared().unwrap() {
        assert_eq!(
            pending.journal_identity().unwrap().intent_fingerprint,
            hash(&serde_json::to_vec(&witness.intent).unwrap())
        );
        let receipt = pending.commit().unwrap();
        assert_eq!(receipt.id, witness.intent.id);
        assert_eq!(receipt.deleted, deleted);
    } else {
        assert_eq!(phase, "archived");
    }
    inspect_recovered(&store, &mut scope, &witness, deleted);
}
#[test]
fn native_note_write_and_delete_killed_owner_recover_every_install_and_archive_boundary() {
    for deleted in [false, true] {
        for phase in [
            "prepared",
            "note-captured",
            "note-installed",
            "index-captured",
            "index-installed",
            "completed",
            "released",
            "archived",
        ] {
            kill_note_owner(phase, deleted);
        }
    }
}
