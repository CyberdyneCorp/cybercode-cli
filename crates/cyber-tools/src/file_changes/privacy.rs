//! Retain private inode ownership while a host is live, including after moves.
//! Never evict a pin to admit another: exhaustion disables observation instead.
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use cap_std::fs::{File, Metadata};

const MAX_PINS: usize = 128;
struct Pin {
    identity: (u64, u64),
    file: File,
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
        // An unlinked object cannot have an observable pathname. Keep any pin
        // whose metadata is uncertain; ordinary WAL cleanup may reclaim slots.
        use cap_fs_ext::MetadataExt;
        registry.pins.retain(|pin| {
            !pin.file
                .metadata()
                .is_ok_and(|metadata| metadata.nlink() == 0)
        });
        for path in paths {
            let pin = match pin(path) {
                Ok(Some(pin)) => pin,
                Ok(None) => continue,
                Err(_) => {
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
fn pin(path: &Path) -> std::io::Result<Option<Pin>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let (metadata, file) = if metadata.is_dir() {
        let directory = super::root::open_directory(&path.canonicalize()?)?;
        (
            directory.dir_metadata()?,
            File::from_std(directory.into_std_file()),
        )
    } else if metadata.is_file() {
        let file = File::open_ambient(path, cap_std::ambient_authority())?;
        (file.metadata()?, file)
    } else {
        return Err(std::io::Error::other(
            "private observation path is not regular",
        ));
    };
    let identity = super::links::identity(&metadata)
        .ok_or_else(|| std::io::Error::other("private observation identity unavailable"))?;
    Ok(Some(Pin { identity, file }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::fs::Dir;
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
    fn unlinking_all_aliases_reclaims_slots_without_dropping_live_private_files() {
        let temp = tempfile::tempdir().unwrap();
        let pins = PrivatePins::default();
        for index in 0..MAX_PINS + 8 {
            let path = temp.path().join(index.to_string());
            std::fs::write(&path, "fixture").unwrap();
            pins.capture([path.as_path()]).unwrap();
            std::fs::remove_file(path).unwrap();
        }
        assert!(pins.capture(std::iter::empty()).unwrap().0.is_empty());
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
