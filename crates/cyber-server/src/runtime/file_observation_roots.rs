//! Native observation ownership. These live leases are intentionally absent from
//! serialized records: replaying scope metadata cannot recreate native authority.
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use same_file::Handle;

#[cfg(unix)]
fn directory_handle(path: &Path) -> io::Result<Handle> {
    use rustix::fs::{Mode, OFlags};
    // A stale Location may now name a database. O_DIRECTORY prevents even an
    // attempted independent regular-file open/close from releasing SQLite locks.
    let file = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Handle::from_file(file.into())
}
#[cfg(not(unix))]
fn directory_handle(path: &Path) -> io::Result<Handle> {
    let handle = Handle::from_path(path)?;
    if !handle.as_file().metadata()?.is_dir() {
        return Err(io::Error::other("observation Location is not a directory"));
    }
    Ok(handle)
}

pub(super) struct CapturedInput {
    pub scope: cyber_core::hooks::file_observations::InputScope,
    _handle: Handle,
}
impl CapturedInput {
    pub fn new(directory: &str) -> io::Result<Self> {
        let raw = Path::new(directory);
        let canonical = raw.canonicalize()?;
        let canonical_text = canonical.to_str().ok_or_else(|| {
            io::Error::other("observation Location requires a UTF-8 canonical path")
        })?;
        let handle = directory_handle(raw)?;
        if directory_handle(&canonical)? != handle {
            return Err(io::Error::other(
                "user input Location changed during capture",
            ));
        }
        let captured = Self {
            scope: cyber_core::hooks::file_observations::InputScope {
                session_directory: directory.into(),
                directory: canonical_text.into(),
                root_identity: cyber_core::hooks::file_observations::root_identity(&handle),
            },
            _handle: handle,
        };
        captured.scope.validate().map_err(io::Error::other)?;
        Ok(captured)
    }
}

pub(super) struct PinnedRoot {
    pub path: PathBuf,
    pub identity: String,
    handle: Handle,
}
impl PinnedRoot {
    pub fn new(path: &Path, file: File) -> io::Result<Self> {
        if !file.metadata()?.is_dir() {
            return Err(io::Error::other(
                "observation lease requires an opened directory",
            ));
        }
        let path = path.canonicalize()?;
        if path.to_str().is_none() {
            return Err(io::Error::other(
                "observation Location requires a UTF-8 canonical path",
            ));
        }
        let handle = Handle::from_file(file)?;
        let identity = cyber_core::hooks::file_observations::root_identity(&handle);
        let root = Self {
            path,
            identity,
            handle,
        };
        if !root.matches(&root.path) {
            return Err(io::Error::other(
                "observation root changed before native lease admission",
            ));
        }
        Ok(root)
    }

    /// The canonical owner path and the Session alias must both still name the
    /// pinned incarnation. Call only after releasing registry and Session locks.
    pub fn matches(&self, directory: &Path) -> bool {
        directory_handle(&self.path).is_ok_and(|current| current == self.handle)
            && directory_handle(directory).is_ok_and(|current| current == self.handle)
    }
}

const MAX_ROOTS: usize = 32;
#[derive(Default)]
pub(super) struct Roots(Mutex<BTreeMap<String, Weak<PinnedRoot>>>);
pub(super) struct Lease {
    pub generation: String,
    pub root: Arc<PinnedRoot>,
}
impl Lease {
    pub fn current(&self, roots: &Roots, directory: &Path) -> bool {
        roots
            .get(&self.generation)
            .is_some_and(|root| Arc::ptr_eq(&root, &self.root))
            && self.root.matches(directory)
    }
}
impl Roots {
    pub fn remove(&self, generation: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(generation);
    }
    pub fn revoke(&self, directory: &Path) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, root| root.upgrade().is_some_and(|root| root.path != directory));
    }
    pub fn register(&self, generation: String, root: &Arc<PinnedRoot>) -> Result<(), String> {
        let mut roots = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        roots.retain(|_, old| old.upgrade().is_some_and(|old| old.path != root.path));
        if roots.len() >= MAX_ROOTS {
            return Err("file observation native lease limit reached".into());
        }
        roots.insert(generation, Arc::downgrade(root));
        Ok(())
    }

    pub fn get(&self, generation: &str) -> Option<Arc<PinnedRoot>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(generation)
            .and_then(Weak::upgrade)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    struct LockedDatabase {
        path: PathBuf,
        connection: rusqlite::Connection,
        _temporary: tempfile::TempDir,
    }
    #[cfg(unix)]
    impl LockedDatabase {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let path = temporary.path().join("database");
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE fixture(value); BEGIN EXCLUSIVE; INSERT INTO fixture VALUES(1)").unwrap();
            let fixture = Self {
                path,
                connection,
                _temporary: temporary,
            };
            fixture.probe();
            fixture
        }
        fn probe(&self) {
            let module = module_path!().split_once("::").unwrap().1;
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("{module}::database_lock_probe"),
                    "--ignored",
                ])
                .env("CYBER_LOCATION_LOCK_FIXTURE", &self.path)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("running 1 test"), "{stdout}");
            assert!(
                output.status.success(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!self.connection.is_autocommit());
        }
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "subprocess fixture invoked by Location identity lock regressions"]
    fn database_lock_probe() {
        let path = std::env::var_os("CYBER_LOCATION_LOCK_FIXTURE").unwrap();
        let database = rusqlite::Connection::open(path).unwrap();
        database.busy_timeout(std::time::Duration::ZERO).unwrap();
        let error = database
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("Location identity check released the owner database lock");
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
    }
    #[cfg(unix)]
    #[test]
    fn input_capture_rejects_a_file_without_releasing_database_locks() {
        let database = LockedDatabase::new();
        assert!(CapturedInput::new(database.path.to_str().unwrap()).is_err());
        database.probe();
    }
    #[cfg(unix)]
    #[test]
    fn native_root_rejects_a_file_alias_without_releasing_database_locks() {
        let database = LockedDatabase::new();
        let path = database.path.with_file_name("location");
        std::fs::create_dir(&path).unwrap();
        let root = PinnedRoot::new(&path, opened(&path)).unwrap();
        assert!(!root.matches(&database.path));
        database.probe();
    }
    #[cfg(unix)]
    #[test]
    fn native_root_replaced_by_a_file_preserves_database_locks() {
        let database = LockedDatabase::new();
        let path = database.path.with_file_name("location");
        std::fs::create_dir(&path).unwrap();
        let root = PinnedRoot::new(&path, opened(&path)).unwrap();
        std::fs::rename(&path, path.with_file_name("old-location")).unwrap();
        std::fs::hard_link(&database.path, &path).unwrap();
        assert!(!root.matches(&path));
        database.probe();
    }
    fn opened(path: &Path) -> File {
        cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
            .unwrap()
            .into_std_file()
    }
    #[test]
    fn native_root_requires_the_actual_directory_and_identity_matches_scanner_contract() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root = PinnedRoot::new(temp.path(), opened(temp.path())).unwrap();
        assert!(root.matches(temp.path()));
        assert!(!root.matches(other.path()));
        assert!(PinnedRoot::new(temp.path(), opened(other.path())).is_err());
        let file = temp.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert!(PinnedRoot::new(&file, File::open(&file).unwrap()).is_err());
        let handle = Handle::from_path(temp.path()).unwrap();
        assert_eq!(
            root.identity,
            cyber_core::hooks::file_observations::root_identity(&handle)
        );
        let captured = CapturedInput::new(&temp.path().display().to_string()).unwrap();
        assert_eq!(captured.scope.root_identity, root.identity);
        captured.scope.validate().unwrap();
        assert!(CapturedInput::new(&file.display().to_string()).is_err());
    }

    // APFS rejects the invalid byte sequence before a native lease can inspect it.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_root_aliases_are_refused_instead_of_recording_lossy_scope_paths() {
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(std::ffi::OsStr::from_bytes(b"root-\xff"));
        std::fs::create_dir(&path).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(PinnedRoot::new(&alias, opened(&alias)).is_err());
        assert!(CapturedInput::new(&alias.display().to_string()).is_err());
    }
    #[test]
    fn leases_are_weak_bounded_and_replacing_a_generation_revokes_its_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let roots = Roots::default();
        let mut owners = Vec::new();
        for index in 0..32 {
            let path = temp.path().join(index.to_string());
            std::fs::create_dir(&path).unwrap();
            let root = Arc::new(PinnedRoot::new(&path, opened(&path)).unwrap());
            roots.register(index.to_string(), &root).unwrap();
            owners.push(root);
        }
        assert!(roots.get("0").is_some());
        let lease = Lease {
            generation: "0".into(),
            root: roots.get("0").unwrap(),
        };
        assert!(lease.current(&roots, &owners[0].path));
        let extra_path = temp.path().join("extra");
        std::fs::create_dir(&extra_path).unwrap();
        let extra = Arc::new(PinnedRoot::new(&extra_path, opened(&extra_path)).unwrap());
        assert!(roots.register("extra".into(), &extra).is_err());
        let replacement =
            Arc::new(PinnedRoot::new(&owners[0].path, opened(&owners[0].path)).unwrap());
        roots.register("replacement".into(), &replacement).unwrap();
        assert!(roots.get("0").is_none());
        assert!(
            !lease.current(&roots, &owners[0].path),
            "superseded leases cannot authorize launch"
        );
        drop(replacement);
        assert!(
            roots.get("replacement").is_none(),
            "registry must not keep an owner alive"
        );
        roots.register("extra".into(), &extra).unwrap();
        roots.revoke(&extra.path);
        assert!(roots.get("extra").is_none());
        roots.register("extra".into(), &extra).unwrap();
        roots.remove("extra");
        assert!(roots.get("extra").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_retarget_and_canonical_root_replacement_lose_membership() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("root");
        let other = temp.path().join("other");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let root = PinnedRoot::new(&path, opened(&path)).unwrap();
        let input = CapturedInput::new(&alias.display().to_string()).unwrap();
        assert!(root.matches(&alias));
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&other, &alias).unwrap();
        assert!(!root.matches(&alias));
        let moved = CapturedInput::new(&alias.display().to_string()).unwrap();
        assert_ne!(
            input.scope.root_identity, moved.scope.root_identity,
            "an old acknowledgement cannot follow a retargeted alias"
        );
        std::fs::rename(&path, temp.path().join("retired")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(!root.matches(&path));
    }
}
