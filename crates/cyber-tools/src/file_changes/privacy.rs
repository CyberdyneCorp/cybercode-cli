//! Retain private inode ownership while a host is live, including after moves.
//! Never evict a pin to admit another: exhaustion disables observation instead.
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use cap_std::fs::{File, Metadata};

const MAX_PINS: usize = 128;
struct Pin {
    identity: (u64, u64),
    file: Option<File>,
}
#[derive(Clone, Default)]
pub(super) struct Snapshot(Vec<Arc<Pin>>);
impl Snapshot {
    pub fn excludes(&self, metadata: &Metadata) -> bool {
        super::links::identity(metadata)
            .is_some_and(|identity| self.0.iter().any(|pin| pin.identity == identity))
    }
}
#[derive(Default)]
pub(crate) struct PrivatePins(Mutex<Registry>);
#[derive(Default)]
struct Registry {
    pins: Vec<Arc<Pin>>,
    unavailable: Option<&'static str>,
}
impl PrivatePins {
    pub(super) fn unavailable(&self, reason: &'static str) -> String {
        let mut registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        registry.unavailable.get_or_insert(reason);
        registry.unavailable.unwrap().into()
    }
    pub(super) fn capture<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<Snapshot, String> {
        let mut registry = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(reason) = registry.unavailable {
            return Err(reason.into());
        }
        // Only retained handles can prove that an object has no remaining links.
        // Unix file identities have no handle: never forget them after a move or
        // infer retirement from a missing pathname. Inode reuse may over-exclude.
        use cap_fs_ext::MetadataExt;
        registry.pins.retain(|pin| {
            !pin.file
                .as_ref()
                .is_some_and(|file| file.metadata().is_ok_and(|metadata| metadata.nlink() == 0))
        });
        for path in paths {
            let pin = match pin(path) {
                Ok(Some(pin)) => pin,
                Ok(None) => continue,
                Err(_error) => {
                    #[cfg(test)]
                    eprintln!("private identity capture failed: {_error:?}");
                    let reason = "private file observation identity unavailable; restart required";
                    registry.unavailable = Some(reason);
                    return Err(reason.into());
                }
            };
            if registry.pins.iter().any(|old| old.identity == pin.identity) {
                continue;
            }
            if registry.pins.len() >= MAX_PINS {
                let reason = "private file observation identity limit reached; restart required";
                registry.unavailable = Some(reason);
                return Err(reason.into());
            }
            registry.pins.push(Arc::new(pin));
        }
        Ok(Snapshot(registry.pins.clone()))
    }
}
#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // Never open regular files, even if a directory pathname is replaced while
    // capture is in flight. Closing a second descriptor could release SQLite's
    // process-wide POSIX record locks. O_NONBLOCK also guards unexpected devices.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
}
#[cfg(windows)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    // BACKUP_SEMANTICS admits directories as well as files through this atomic
    // open. Retained metadata ownership must not prevent native rename/unlink.
    for attempt in 0..8 {
        let result = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path);
        // CreateFile may map a pending delete to ACCESS_DENIED. Retry capture
        // briefly without publishing a scan or treating an ambiguous denial as
        // absence. Persistent denial still latches unavailable in the caller.
        let transient = result.as_ref().is_err_and(|error| {
            matches!(error.raw_os_error(), Some(code)
                if code == ERROR_ACCESS_DENIED as i32 || code == ERROR_SHARING_VIOLATION as i32)
        });
        if !transient || attempt == 7 {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    unreachable!("the final private capture attempt always returns")
}
#[cfg(not(any(unix, windows)))]
fn open_private(_path: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private metadata ownership is unsupported",
    ))
}
#[cfg(unix)]
fn pin(path: &Path) -> std::io::Result<Option<Pin>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => Metadata::from_just_metadata(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let identity = super::links::identity(&metadata)
        .ok_or_else(|| std::io::Error::other("private observation identity unavailable"))?;
    if metadata.is_file() {
        // stat does not create or close a database descriptor. Retain this
        // exclusion conservatively; a pathname cannot prove an inode retired.
        return Ok(Some(Pin {
            identity,
            file: None,
        }));
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::other(
            "private observation path is not regular",
        ));
    }
    let file = File::from_std(open_private(path)?);
    if super::links::identity(&file.metadata()?) != Some(identity) {
        return Err(std::io::Error::other(
            "private directory changed while opening",
        ));
    }
    Ok(Some(Pin {
        identity,
        file: Some(file),
    }))
}
#[cfg(not(unix))]
fn pin(path: &Path) -> std::io::Result<Option<Pin>> {
    let file = match open_private(path) {
        Ok(file) => File::from_std(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(std::io::Error::other(
            "private observation path is not regular",
        ));
    }
    let identity = super::links::identity(&metadata)
        .ok_or_else(|| std::io::Error::other("private observation identity unavailable"))?;
    Ok(Some(Pin {
        identity,
        file: Some(file),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::fs::Dir;
    #[cfg(windows)]
    #[test]
    fn private_metadata_capture_does_not_require_file_data_read_access() {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private-file");
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let pins = PrivatePins::default();
        let snapshot = pins.capture([path.as_path()]).unwrap();
        writer.write_all(b"fixture").unwrap();
        assert!(snapshot.excludes(&Metadata::from_file(&writer).unwrap()));
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "subprocess fixture invoked by private_capture_preserves_existing_record_locks"]
    fn private_file_lock_probe() {
        let path = std::env::var_os("CYBER_PRIVATE_LOCK_FIXTURE").unwrap();
        let database = rusqlite::Connection::open(path).unwrap();
        database.busy_timeout(std::time::Duration::ZERO).unwrap();
        let error = database
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("owner database lock was released");
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
    }
    #[cfg(unix)]
    #[test]
    fn private_capture_preserves_existing_record_locks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("database");
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE fixture(value); BEGIN EXCLUSIVE; INSERT INTO fixture VALUES(1)").unwrap();
        let probe = || {
            let module = module_path!().split_once("::").unwrap().1;
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("{module}::private_file_lock_probe"),
                    "--ignored",
                ])
                .env("CYBER_PRIVATE_LOCK_FIXTURE", &path)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("running 1 test"), "{stdout}");
            assert!(
                output.status.success(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        probe();
        let pins = PrivatePins::default();
        pins.capture([path.as_path()]).unwrap();
        pins.capture([path.as_path()]).unwrap();
        assert!(
            open_private(&path).is_err(),
            "directory capture opened a file"
        );
        probe();
        drop(pins);
        probe();
        let location = temp.path().join("location");
        std::fs::create_dir(&location).unwrap();
        let root = super::super::root::Root::open(&location).unwrap();
        std::fs::rename(&location, temp.path().join("old-location")).unwrap();
        std::fs::hard_link(&path, &location).unwrap();
        assert!(!root.current());
        probe();
        assert!(!database.is_autocommit());
    }
    #[test]
    fn moved_private_directory_and_file_aliases_retain_their_pins() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let database = root.join("private.db");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("secret"), "fixture").unwrap();
        std::fs::write(&database, "fixture").unwrap();
        let pins = PrivatePins::default();
        pins.capture([private.as_path(), database.as_path()])
            .unwrap();
        std::fs::rename(&private, root.join("moved")).unwrap();
        std::fs::rename(&database, root.join("moved.db")).unwrap();
        std::fs::hard_link(root.join("moved.db"), root.join("alias.db")).unwrap();
        let current = pins
            .capture([private.as_path(), database.as_path()])
            .unwrap();
        let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        for name in ["moved", "moved.db", "alias.db"] {
            assert!(
                current.excludes(&directory.symlink_metadata(name).unwrap()),
                "{name}"
            );
        }
    }
    #[test]
    fn renamed_private_subtree_is_not_traversed_but_ordinary_files_stay_live() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let private = path.join("owner-data");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("credential-metadata"), "fixture").unwrap();
        std::fs::write(path.join("public"), "fixture").unwrap();
        let root = super::super::root::Root::open(&path).unwrap();
        let pins = PrivatePins::default();
        pins.capture([private.as_path()]).unwrap();
        std::fs::rename(&private, path.join("relocated-owner-data")).unwrap();
        let excluded = super::super::root::Exclusions::new(
            &path,
            &path,
            vec![private.clone()],
            vec![],
            |_, _, _| false,
        )
        .with_private(pins.capture([private.as_path()]).unwrap());
        let scan = super::super::scan::scan_with_metadata(
            &root.directory,
            &|path| excluded.excludes(path),
            &|metadata| excluded.excludes_metadata(metadata),
            &super::super::scan::Limits {
                entries: 100,
                depth: 10,
                path_bytes: 4096,
            },
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(scan.complete);
        assert_eq!(
            scan.files.keys().cloned().collect::<Vec<_>>(),
            vec![std::path::PathBuf::from("public")]
        );
        assert!(!scan.directories.contains(Path::new("relocated-owner-data")));
        let private_root = Dir::open_ambient_dir(
            path.join("relocated-owner-data"),
            cap_std::ambient_authority(),
        )
        .unwrap();
        let private_scan = super::super::scan::scan_with_metadata(
            &private_root,
            &|_| false,
            &|metadata| excluded.excludes_metadata(metadata),
            &super::super::scan::Limits {
                entries: 100,
                depth: 10,
                path_bytes: 4096,
            },
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(!private_scan.complete);
        assert!(private_scan.files.is_empty());
        assert!(private_scan.directories.is_empty());
    }
    #[test]
    fn opened_directory_is_rechecked_after_a_private_root_replaces_public_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let private = path.join("private");
        let public = path.join("public");
        std::fs::create_dir(&private).unwrap();
        std::fs::create_dir(&public).unwrap();
        std::fs::write(private.join("secret"), "fixture").unwrap();
        let root = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        let public_id = super::super::links::identity(&root.symlink_metadata("public").unwrap());
        let pins = PrivatePins::default();
        let snapshot = pins.capture([private.as_path()]).unwrap();
        let swapped = std::sync::atomic::AtomicBool::new(false);
        let scan = super::super::scan::scan_with_metadata(
            &root,
            &|path| path == Path::new("private"),
            &|metadata| {
                if super::super::links::identity(metadata) == public_id
                    && !swapped.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    std::fs::rename(&public, path.join("old-public")).unwrap();
                    std::fs::rename(&private, &public).unwrap();
                }
                snapshot.excludes(metadata)
            },
            &super::super::scan::Limits {
                entries: 100,
                depth: 10,
                path_bytes: 4096,
            },
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(swapped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(scan.files.is_empty(), "{:?}", scan.files);
        assert!(!scan.directories.contains(Path::new("public")));
    }
    #[cfg(unix)]
    #[test]
    fn private_file_identity_is_metadata_only_and_is_not_forgotten_after_unlink() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("database");
        std::fs::write(&path, "fixture").unwrap();
        let metadata = Metadata::from_just_metadata(std::fs::metadata(&path).unwrap());
        let pins = PrivatePins::default();
        pins.capture([path.as_path()]).unwrap();
        assert!(pins.0.lock().unwrap().pins[0].file.is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(pins.capture([path.as_path()]).unwrap().excludes(&metadata));
    }
    #[test]
    fn directory_retirement_requires_handle_evidence_of_zero_links() {
        use cap_fs_ext::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        std::fs::create_dir(&path).unwrap();
        let pins = PrivatePins::default();
        pins.capture([path.as_path()]).unwrap();
        std::fs::rename(&path, temp.path().join("moved")).unwrap();
        assert_eq!(pins.capture([path.as_path()]).unwrap().0.len(), 1);
        std::fs::remove_dir(temp.path().join("moved")).unwrap();
        let proven_retired = pins.0.lock().unwrap().pins[0]
            .file
            .as_ref()
            .unwrap()
            .metadata()
            .unwrap()
            .nlink()
            == 0;
        // Some filesystems retain a nonzero directory link count after unlink.
        // Such a handle provides no proof of retirement and must remain retained.
        assert_eq!(
            pins.capture([path.as_path()]).unwrap().0.len(),
            usize::from(!proven_retired)
        );
    }
    #[test]
    fn sidecar_turnover_never_confuses_a_missing_path_with_failed_identity_capture() {
        let temp = tempfile::tempdir().unwrap();
        let sidecar = temp.path().join("database-wal");
        let writer = std::thread::spawn({
            let sidecar = sidecar.clone();
            move || {
                for _ in 0..2000 {
                    std::fs::write(&sidecar, "fixture").unwrap();
                    std::fs::remove_file(&sidecar).unwrap();
                }
            }
        });
        let pins = PrivatePins::default();
        let mut failure = None;
        while !writer.is_finished() {
            if let Err(error) = pins.capture([sidecar.as_path()]) {
                failure = Some(error);
                break;
            }
        }
        writer.join().unwrap();
        if let Some(error) = failure {
            // Unix metadata-only file identities cannot prove unlink retirement.
            // Their bounded history may fill, but a stat/open race must not be
            // reported as unknown capture or silently evict prior ownership.
            assert_eq!(
                error,
                "private file observation identity limit reached; restart required"
            );
            assert_eq!(pins.0.lock().unwrap().pins.len(), MAX_PINS);
            assert!(pins.capture(std::iter::empty()).is_err());
        } else {
            assert!(pins.capture([sidecar.as_path()]).is_ok());
        }
    }
    #[cfg(unix)]
    #[test]
    fn failed_private_identity_capture_cannot_resume_after_the_unknown_path_disappears() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("unknown-private-object");
        let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let pins = PrivatePins::default();
        assert!(pins.capture([path.as_path()]).is_err());
        drop(socket);
        std::fs::remove_file(path).unwrap();
        assert!(pins.capture(std::iter::empty()).is_err());
    }
    #[test]
    fn exhaustion_retains_previous_ownership_instead_of_evicting_a_private_pin() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let pins = PrivatePins::default();
        for index in 0..MAX_PINS {
            let path = root.join(index.to_string());
            std::fs::write(&path, "fixture").unwrap();
            pins.capture([path.as_path()]).unwrap();
        }
        let previous = pins.capture(std::iter::empty()).unwrap();
        let overflow = root.join("overflow");
        std::fs::write(&overflow, "fixture").unwrap();
        assert!(pins.capture([overflow.as_path()]).is_err());
        assert!(pins.capture(std::iter::empty()).is_err());
        assert_eq!(pins.0.lock().unwrap().pins.len(), MAX_PINS);
        let root = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        assert!(previous.excludes(&root.symlink_metadata("0").unwrap()));
        assert_eq!(previous.0.len(), MAX_PINS);
    }
}
