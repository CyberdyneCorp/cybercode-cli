//! Actual directory/descriptor ownership; note mutations are not implemented yet.
use cyber_core::memory::{MemoryStorageError, MemoryStore};
use std::io::Write;
use std::path::Path;

fn write(path: &Path, content: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).unwrap().write_all(content).unwrap();
}
fn note(name: &str) -> String {
    format!("---\nname: {name}\ndescription: useful note\ntype: user\n---\nA preference\n")
}

#[test]
fn private_project_and_global_storage_is_separate_and_existing_review_does_not_create_it() {
    let data = tempfile::tempdir().unwrap();
    assert!(
        MemoryStore::existing(data.path(), "global")
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    let project = MemoryStore::open(data.path(), "prj_test").unwrap();
    let global = MemoryStore::open(data.path(), "global").unwrap();
    assert_ne!(project.path(), global.path());
    let project_guard = project.claim().unwrap();
    let global_guard = global.claim().unwrap();
    write(&project.path().join("rule.md"), note("rule").as_bytes());
    assert_eq!(project_guard.read("rule").unwrap().body, "A preference");
    assert!(matches!(
        global_guard.read("rule"),
        Err(MemoryStorageError::NotFound)
    ));
    assert!(
        MemoryStore::existing(data.path(), "prj_test")
            .unwrap()
            .is_some()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(project.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(project.path().join(".memory.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn catalog_is_sorted_and_reports_invalid_notes_without_echoing_body_text() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    for name in ["z-rule", "a-rule"] {
        write(
            &store.path().join(format!("{name}.md")),
            note(name).as_bytes(),
        );
    }
    write(
        &store.path().join("invalid.md"),
        b"---\nname: private supplied body\n---\nprivate supplied body",
    );
    write(&store.path().join("mismatch.md"), note("other").as_bytes());
    std::fs::create_dir(store.path().join("directory.md")).unwrap();
    let guard = store.claim().unwrap();
    let catalog = guard.list().unwrap();
    assert_eq!(
        catalog
            .memories
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a-rule", "z-rule"]
    );
    assert_eq!(catalog.invalid.len(), 3);
    for warning in catalog.invalid {
        assert!(!warning.diagnostic.contains("private supplied body"));
    }
    assert!(guard.read("../escape").is_err());
    assert!(guard.read("missing").is_err());
}

#[test]
fn index_reads_only_a_bounded_prefix_without_opening_individual_notes() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let guard = store.claim().unwrap();
    assert_eq!(guard.index().unwrap().text, "");
    write(&store.path().join("not-readable.md"), &[0xff]);
    let prefix = "line\n".repeat(200);
    let bytes = [prefix.as_bytes(), &[0xff]].concat();
    write(&store.path().join("MEMORY.md"), &bytes);
    let snapshot = guard.index().unwrap();
    assert!(snapshot.truncated);
    assert!(snapshot.text.starts_with(&prefix));
    write(
        &store.path().join("MEMORY.md"),
        format!("{}éremaining", "a".repeat(24_999)).as_bytes(),
    );
    let snapshot = guard.index().unwrap();
    assert!(snapshot.truncated);
    assert_eq!(
        snapshot.text,
        format!(
            "{}\n[memory index truncated; read MEMORY.md for more]",
            "a".repeat(24_999)
        )
    );
    write(
        &store.path().join("MEMORY.md"),
        &["a".repeat(25_000).as_bytes(), &[0xff]].concat(),
    );
    assert!(guard.index().unwrap().truncated);
    write(&store.path().join("MEMORY.md"), &[0xff]);
    assert!(guard.index().is_err());
}

#[test]
fn pending_transactions_fence_all_reads_without_deleting_recovery_evidence() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let guard = store.claim().unwrap();
    write(&store.path().join("rule.md"), note("rule").as_bytes());
    write(
        &store.path().join(".memory-transaction"),
        b"retained transaction",
    );
    assert!(matches!(
        guard.read("rule"),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    assert!(matches!(
        guard.list(),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    assert!(matches!(
        guard.index(),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    assert_eq!(
        std::fs::read(store.path().join(".memory-transaction")).unwrap(),
        b"retained transaction"
    );
}

#[test]
fn note_size_and_metadata_name_are_checked_before_returning_contents() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let guard = store.claim().unwrap();
    write(&store.path().join("huge.md"), &vec![b'a'; 1_048_577]);
    assert!(matches!(
        guard.read("huge"),
        Err(MemoryStorageError::TooLarge)
    ));
    write(&store.path().join("wrong.md"), note("other").as_bytes());
    assert!(matches!(
        guard.read("wrong"),
        Err(MemoryStorageError::Unsafe(_))
    ));
}

#[test]
fn independent_opened_handles_cannot_borrow_or_replace_an_active_scope_claim() {
    let data = tempfile::tempdir().unwrap();
    let first = MemoryStore::open(data.path(), "global").unwrap();
    let second = MemoryStore::open(data.path(), "global").unwrap();
    let guard = first.claim().unwrap();
    assert!(matches!(second.claim(), Err(MemoryStorageError::Busy)));
    drop(guard);
    assert!(second.claim().is_ok());
}

#[test]
fn memory_lock_child() {
    let Some(data) = std::env::var_os("CYBER_MEMORY_LOCK_TEST") else {
        return;
    };
    let store = MemoryStore::open(Path::new(&data), "global").unwrap();
    let _guard = store.claim().unwrap();
    write(&Path::new(&data).join("lock-ready"), b"owned");
    std::thread::sleep(std::time::Duration::from_secs(30));
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn killed_process_releases_scope_lock_without_adopting_its_execution() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut child = OwnedChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "memory_lock_child"])
            .env("CYBER_MEMORY_LOCK_TEST", data.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !data.path().join("lock-ready").exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.0.kill();
            let _ = child.0.wait();
            panic!("memory lock worker did not become ready");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(store.claim(), Err(MemoryStorageError::Busy)));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(store.claim().is_ok());
}

#[cfg(unix)]
#[test]
fn symlinked_directories_files_and_lock_paths_never_touch_outside_data() {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), data.path().join("memory")).unwrap();
    assert!(MemoryStore::open(data.path(), "global").is_err());
    std::fs::remove_file(data.path().join("memory")).unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let target = outside.path().join("untouched");
    write(&target, b"outside data");
    symlink(&target, store.path().join(".memory.lock")).unwrap();
    assert!(store.claim().is_err());
    std::fs::remove_file(store.path().join(".memory.lock")).unwrap();
    let guard = store.claim().unwrap();
    symlink(&target, store.path().join("rule.md")).unwrap();
    symlink(&target, store.path().join("MEMORY.md")).unwrap();
    assert!(guard.read("rule").is_err());
    assert!(guard.index().is_err());
    assert_eq!(guard.list().unwrap().invalid.len(), 1);
    assert_eq!(std::fs::read(&target).unwrap(), b"outside data");
    symlink(store.path(), data.path().join("memory/prj_alias")).unwrap();
    assert!(MemoryStore::open(data.path(), "prj_alias").is_err());
}

#[cfg(unix)]
#[test]
fn hard_links_and_public_file_permissions_are_refused_before_read_or_permission_changes() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let outside = data.path().join("outside");
    write(&outside, note("rule").as_bytes());
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::hard_link(&outside, store.path().join(".memory.lock")).unwrap();
    assert!(store.claim().is_err());
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::remove_file(store.path().join(".memory.lock")).unwrap();
    let guard = store.claim().unwrap();
    std::fs::hard_link(&outside, store.path().join("rule.md")).unwrap();
    assert!(guard.read("rule").is_err());
    write(&store.path().join("public.md"), note("public").as_bytes());
    std::fs::set_permissions(
        store.path().join("public.md"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(guard.read("public").is_err());
}

#[test]
fn catalog_metadata_budget_refuses_large_notes_instead_of_returning_partial_success() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let description = "a".repeat(900_000);
    for index in 0..5 {
        let name = format!("note-{index}");
        let text =
            format!("---\nname: {name}\ndescription: {description}\ntype: user\n---\nA fact\n");
        write(&store.path().join(format!("{name}.md")), text.as_bytes());
    }
    let error = store.claim().unwrap().list().unwrap_err();
    assert!(error.to_string().contains("catalog exceeds 4 MiB"));
}

#[test]
fn directory_entry_budget_counts_non_memory_entries_as_well() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let guard = store.claim().unwrap();
    for index in 0..4095 {
        write(&store.path().join(format!("unrelated-{index}")), b"");
    }
    assert!(guard.list().unwrap().memories.is_empty());
    write(&store.path().join("overflow"), b"");
    assert!(
        guard
            .list()
            .unwrap_err()
            .to_string()
            .contains("too many entries")
    );
}
