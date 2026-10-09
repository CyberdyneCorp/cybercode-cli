//! Private client retention never installs a memory note or replays a request.
use cyber_core::memory::{MemoryClientStore, MemoryStorageError};

#[cfg(unix)]
#[test]
fn restart_preserves_atomic_bytes_and_exclusive_ownership() {
    use std::os::unix::fs::PermissionsExt;
    let state = tempfile::tempdir().unwrap();
    assert!(MemoryClientStore::existing(state.path()).unwrap().is_none());
    assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);
    let mut owner = MemoryClientStore::open(state.path()).unwrap();
    assert!(owner.checkpoint().is_none());
    assert!(matches!(
        MemoryClientStore::open(state.path()),
        Err(MemoryStorageError::Busy)
    ));
    owner.save(b"retained draft and request key").unwrap();
    assert_eq!(
        std::fs::metadata(state.path().join("memory-client"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(state.path().join("memory-client/state.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(owner);
    let mut reopened = MemoryClientStore::existing(state.path()).unwrap().unwrap();
    assert_eq!(
        reopened.checkpoint(),
        Some(b"retained draft and request key".as_slice())
    );
    reopened.save(b"new retained intent").unwrap();
    drop(reopened);
    let final_owner = MemoryClientStore::existing(state.path()).unwrap().unwrap();
    assert_eq!(
        final_owner.checkpoint(),
        Some(b"new retained intent".as_slice())
    );
    assert!(!state.path().join("memory").exists());
}

#[cfg(unix)]
#[test]
fn changed_checkpoint_is_preserved_and_refuses_overwrite() {
    let state = tempfile::tempdir().unwrap();
    let mut owner = MemoryClientStore::open(state.path()).unwrap();
    owner.save(b"original").unwrap();
    std::fs::write(
        state.path().join("memory-client/state.json"),
        b"external edit",
    )
    .unwrap();
    assert!(matches!(
        owner.save(b"draft"),
        Err(MemoryStorageError::ReviewConflict)
    ));
    assert_eq!(
        std::fs::read(state.path().join("memory-client/state.json")).unwrap(),
        b"external edit"
    );
    assert!(matches!(
        owner.save(&vec![0; 16 * 1024 * 1024 + 1]),
        Err(MemoryStorageError::TooLarge)
    ));
}

#[cfg(unix)]
#[test]
fn unsafe_checkpoint_aliases_or_privacy_are_refused_without_changes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for alias in ["symlink", "hardlink", "public"] {
        let state = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"outside user file").unwrap();
        drop(MemoryClientStore::open(state.path()).unwrap());
        let path = state.path().join("memory-client/state.json");
        match alias {
            "symlink" => symlink(outside.path(), &path).unwrap(),
            "hardlink" => std::fs::hard_link(outside.path(), &path).unwrap(),
            _ => {
                std::fs::write(&path, b"existing state").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        assert!(MemoryClientStore::existing(state.path()).is_err());
        assert_eq!(std::fs::read(outside.path()).unwrap(), b"outside user file");
        assert!(path.symlink_metadata().is_ok());
    }
}

#[cfg(unix)]
#[test]
fn replaced_scope_lock_or_state_directory_refuses_the_retained_owner() {
    for target in ["memory-client", ".client.lock", "state"] {
        let outer = tempfile::tempdir().unwrap();
        let state = outer.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let mut owner = MemoryClientStore::open(&state).unwrap();
        owner.save(b"original").unwrap();
        let path = match target {
            "state" => state.clone(),
            "memory-client" => state.join(target),
            _ => state.join("memory-client").join(target),
        };
        let moved = outer.path().join("retained-original");
        std::fs::rename(&path, &moved).unwrap();
        if target == ".client.lock" {
            std::fs::write(&path, b"replacement").unwrap();
        } else {
            std::fs::create_dir(&path).unwrap();
        }
        assert!(owner.save(b"draft").is_err());
        let original = match target {
            "state" => moved.join("memory-client/state.json"),
            "memory-client" => moved.join("state.json"),
            _ => state.join("memory-client/state.json"),
        };
        assert_eq!(std::fs::read(original).unwrap(), b"original");
    }
}

#[cfg(not(unix))]
#[test]
fn unsupported_native_privacy_refuses_before_creating_client_storage() {
    let state = tempfile::tempdir().unwrap();
    assert!(matches!(
        MemoryClientStore::open(state.path()),
        Err(MemoryStorageError::Unsafe(_))
    ));
    assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn identical_bytes_in_a_replaced_checkpoint_do_not_grant_overwrite_authority() {
    use std::os::unix::fs::PermissionsExt;
    let state = tempfile::tempdir().unwrap();
    let mut owner = MemoryClientStore::open(state.path()).unwrap();
    owner.save(b"original").unwrap();
    let file = state.path().join("memory-client/state.json");
    std::fs::rename(&file, state.path().join("memory-client/user-retained.json")).unwrap();
    std::fs::write(&file, b"original").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        owner.save(b"draft"),
        Err(MemoryStorageError::ReviewConflict)
    ));
    assert_eq!(std::fs::read(file).unwrap(), b"original");
}

#[cfg(unix)]
#[test]
fn client_state_child() {
    let Some(state) = std::env::var_os("CYBER_CLIENT_STATE_TEST_CHILD") else {
        return;
    };
    let state = std::path::PathBuf::from(state);
    let mut owner = MemoryClientStore::open(&state).unwrap();
    owner.save(b"synced client intent").unwrap();
    std::fs::write(state.join("ready"), b"ready").unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(unix)]
#[test]
fn abrupt_process_death_releases_ownership_and_reopens_only_the_synced_checkpoint() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let state = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "client_state_child", "--nocapture"])
        .env("CYBER_CLIENT_STATE_TEST_CHILD", state.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.path().join("ready").exists() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("client checkpoint worker did not become ready");
        }
        assert!(child.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        MemoryClientStore::existing(state.path()),
        Err(MemoryStorageError::Busy)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    let pending = state.path().join("memory-client/.uncommitted.pending");
    std::fs::write(&pending, b"incomplete staged intent").unwrap();
    std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o600)).unwrap();
    let owner = MemoryClientStore::existing(state.path()).unwrap().unwrap();
    assert_eq!(owner.checkpoint(), Some(b"synced client intent".as_slice()));
    assert_eq!(std::fs::read(pending).unwrap(), b"incomplete staged intent");
}

#[cfg(unix)]
#[test]
fn dangling_directory_and_checkpoint_aliases_are_not_treated_as_missing_state() {
    use std::os::unix::fs::symlink;
    for directory in [true, false] {
        let state = tempfile::tempdir().unwrap();
        let missing = state.path().join("outside-missing");
        let path = if directory {
            state.path().join("memory-client")
        } else {
            drop(MemoryClientStore::open(state.path()).unwrap());
            state.path().join("memory-client/state.json")
        };
        symlink(&missing, &path).unwrap();
        assert!(MemoryClientStore::existing(state.path()).is_err());
        assert!(path.symlink_metadata().unwrap().is_symlink());
        assert!(!missing.exists());
    }
}
