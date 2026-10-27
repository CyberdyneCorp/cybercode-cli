//! Native notifications are hints, not authoritative writes or writer identities.
//! Bound the retained queue and each hint; loss requests metadata reconciliation.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::time::Duration;

use super::state::relative_path;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

const PATHS_PER_HINT: usize = 128;
const PATH_BYTES: usize = 4096;
const MAX_DIRECTORIES: usize = 256;

#[derive(Debug, Default)]
pub(super) struct Hints {
    pub paths: BTreeSet<PathBuf>,
    pub rescan: bool,
}

impl Hints {
    /// A coalesced native/tool hint set has its own bound, independently of the
    /// native callback queue and the metadata inventory.
    pub fn record(&mut self, relative: &Path) {
        let valid = relative_path(relative)
            && relative.to_str().is_some_and(|name| {
                name.len() <= PATH_BYTES
                    && cyber_core::hooks::file_observations::relative_name(
                        &name.replace(std::path::MAIN_SEPARATOR, "/"),
                    )
            });
        if !valid {
            self.rescan = true;
            return;
        }
        if self.paths.contains(relative) {
            return;
        }
        if self.paths.len() >= 1024 {
            self.rescan = true;
            return;
        }
        self.paths.insert(relative.to_path_buf());
    }
    pub fn merge(&mut self, other: Self) {
        self.rescan |= other.rescan;
        for path in other.paths {
            self.record(&path);
        }
    }
}

pub(super) struct Native {
    // Drop stops native observation before its receiver is disposed.
    watcher: RecommendedWatcher,
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    directory_limit: usize,
    receiver: Receiver<Hints>,
    overflow: Arc<AtomicBool>,
}

fn enqueue(
    root: &Path,
    sender: &SyncSender<Hints>,
    overflow: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    let Ok(event) = event else {
        overflow.store(true, Ordering::Release);
        return;
    };
    if matches!(event.kind, EventKind::Access(_)) && !event.need_rescan() {
        return;
    }
    let mut hint = Hints {
        rescan: event.need_rescan(),
        ..Hints::default()
    };
    for (index, path) in event.paths.into_iter().enumerate() {
        if index >= PATHS_PER_HINT {
            hint.rescan = true;
            break;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            hint.rescan = true;
            continue;
        }
        if !relative_path(relative) || relative.to_str().is_none_or(|text| text.len() > PATH_BYTES)
        {
            hint.rescan = true;
            continue;
        }
        hint.paths.insert(relative.to_path_buf());
    }
    if hint.paths.is_empty() && !hint.rescan {
        return;
    }
    if sender.try_send(hint).is_err() {
        overflow.store(true, Ordering::Release);
    }
}

impl Native {
    pub fn start(root: &Path, capacity: usize) -> notify::Result<Self> {
        Self::with_limit(root, capacity, MAX_DIRECTORIES)
    }

    fn with_limit(root: &Path, capacity: usize, directory_limit: usize) -> notify::Result<Self> {
        if !(1..=MAX_DIRECTORIES).contains(&directory_limit) {
            return Err(notify::Error::generic(
                "file watch directory limit is invalid",
            ));
        }
        if !(1..=1024).contains(&capacity) {
            return Err(notify::Error::generic(
                "file observation queue capacity must be 1..=1024",
            ));
        }
        let root = root.canonicalize().map_err(notify::Error::io)?;
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let overflow = Arc::new(AtomicBool::new(false));
        let watch_root = root.clone();
        let lost = Arc::clone(&overflow);
        let mut watcher = RecommendedWatcher::new(
            move |event| enqueue(&watch_root, &sender, &lost, event),
            Config::default().with_follow_symlinks(false),
        )?;
        watcher.watch(&root, RecursiveMode::NonRecursive)?;
        Ok(Self {
            watcher,
            root,
            directories: BTreeSet::from([PathBuf::new()]),
            directory_limit,
            receiver,
            overflow,
        })
    }

    /// Register only directories actually reached by the no-follow scanner.
    /// An incomplete scan retains existing registrations: missing evidence must
    /// not silently remove nested coverage. False means visible partial coverage
    /// and reconciliation are required; it never increases the resource bound.
    pub fn update_directories(&mut self, scanned: &BTreeSet<PathBuf>, complete_scan: bool) -> bool {
        let mut complete = complete_scan;
        if complete_scan {
            let removed: Vec<_> = self
                .directories
                .iter()
                .filter(|path| !path.as_os_str().is_empty() && !scanned.contains(*path))
                .cloned()
                .collect();
            for path in removed {
                let absent = match self.watcher.unwatch(&self.root.join(&path)) {
                    Ok(()) => true,
                    Err(error) => matches!(&error.kind, notify::ErrorKind::WatchNotFound),
                };
                if absent {
                    self.directories.remove(&path);
                } else {
                    complete = false;
                }
            }
        }
        for relative in scanned {
            if self.directories.contains(relative) {
                continue;
            }
            if self.directories.len() >= self.directory_limit {
                complete = false;
                break;
            }
            if !relative_path(relative) {
                complete = false;
                continue;
            }
            let path = self.root.join(relative);
            // Do not intentionally register a symlink alias, even one pointing
            // back inside the root. The owning actor must also verify pinned-root
            // identity and current Session scope before effects.
            if path.canonicalize().ok().as_ref() != Some(&path) {
                complete = false;
                continue;
            }
            if self
                .watcher
                .watch(&path, RecursiveMode::NonRecursive)
                .is_ok()
            {
                self.directories.insert(relative.clone());
            } else {
                complete = false;
            }
        }
        if !complete {
            self.overflow.store(true, Ordering::Release);
        }
        complete
    }

    /// At most one bounded hint is consumed per call. The owner coalesces under
    /// its own bound and must reconcile on rescan, timeout or any native loss.
    pub fn receive(&self, timeout: Duration) -> Hints {
        let mut hint = self.receiver.recv_timeout(timeout).unwrap_or_default();
        hint.rescan |= self.overflow.swap(false, Ordering::AcqRel);
        hint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn coalesced_tool_and_native_hints_stay_bounded_and_signal_lost_names() {
        let mut hints = Hints::default();
        for index in 0..1100 {
            hints.record(Path::new(&format!("file-{index}")));
        }
        assert_eq!(hints.paths.len(), 1024);
        assert!(hints.rescan);
        hints.merge(Hints {
            paths: BTreeSet::from([PathBuf::from("../private"), PathBuf::from("file-1")]),
            rescan: false,
        });
        assert_eq!(hints.paths.len(), 1024);
        assert!(!hints.paths.contains(Path::new("../private")));
    }

    #[test]
    fn queue_loss_is_reported_even_without_another_successful_callback() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        for name in ["one", "two"] {
            enqueue(
                Path::new("/root"),
                &sender,
                &overflow,
                Ok(notify::Event::new(EventKind::Any).add_path(Path::new("/root").join(name))),
            );
        }
        assert!(overflow.load(Ordering::Acquire));
        assert_eq!(
            receiver.try_recv().unwrap().paths,
            BTreeSet::from([PathBuf::from("one")])
        );
    }

    #[test]
    fn oversized_and_outside_native_payloads_do_not_escape_bounds() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        let mut event = notify::Event::new(EventKind::Any).add_path("/outside/private".into());
        for index in 0..(PATHS_PER_HINT + 2) {
            event = event.add_path(format!("/root/{index}").into());
        }
        enqueue(Path::new("/root"), &sender, &overflow, Ok(event));
        let hint = receiver.try_recv().unwrap();
        assert!(hint.rescan);
        assert!(hint.paths.len() <= PATHS_PER_HINT);
        assert!(!hint.paths.contains(Path::new("private")));
    }

    #[test]
    fn actual_native_watch_observes_editor_replace_rename_and_removal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let native = Native::start(&root, 32).unwrap();
        std::fs::write(root.join("draft"), "first").unwrap();
        std::fs::rename(root.join("draft"), root.join("saved")).unwrap();
        std::fs::write(root.join("saved"), "second").unwrap();
        std::fs::remove_file(root.join("saved")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut paths = BTreeSet::new();
        while Instant::now() < deadline {
            paths.extend(native.receive(Duration::from_millis(100)).paths);
            if paths.contains(Path::new("draft")) && paths.contains(Path::new("saved")) {
                break;
            }
        }
        assert!(paths.contains(Path::new("draft")), "{paths:?}");
        assert!(paths.contains(Path::new("saved")), "{paths:?}");
    }

    #[cfg(unix)]
    #[test]
    fn actual_native_watch_does_not_follow_an_outside_directory_symlink() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = inside.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        let native = Native::start(&root, 32).unwrap();
        std::fs::write(root.join("positive"), "inside").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut started = false;
        while Instant::now() < deadline {
            if native
                .receive(Duration::from_millis(100))
                .paths
                .contains(Path::new("positive"))
            {
                started = true;
                break;
            }
        }
        assert!(started, "positive control must observe an in-scope change");
        std::fs::write(outside.path().join("hidden"), "outside").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let paths = native.receive(Duration::from_millis(100)).paths;
            assert!(!paths.contains(Path::new("escape/hidden")), "{paths:?}");
            assert!(!paths.contains(Path::new("hidden")), "{paths:?}");
        }
    }
    #[test]
    fn registrations_are_bounded_and_partial_scans_preserve_existing_coverage() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let mut native = Native::with_limit(temp.path(), 32, 2).unwrap();
        let scanned = BTreeSet::from([PathBuf::new(), PathBuf::from("a"), PathBuf::from("b")]);
        assert!(!native.update_directories(&scanned, true));
        assert_eq!(native.directories.len(), 2);
        assert!(native.receive(Duration::from_millis(1)).rescan);
        assert!(!native.update_directories(&BTreeSet::new(), false));
        assert!(native.directories.contains(Path::new("a")));
        assert!(
            native.update_directories(&BTreeSet::from([PathBuf::new(), PathBuf::from("c")]), true)
        );
        assert!(!native.directories.contains(Path::new("a")));
        assert!(native.directories.contains(Path::new("c")));
    }

    #[test]
    fn registered_nested_directory_delivers_actual_native_changes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("nested")).unwrap();
        let mut native = Native::start(temp.path(), 32).unwrap();
        assert!(native.update_directories(
            &BTreeSet::from([PathBuf::new(), PathBuf::from("nested")]),
            true
        ));
        std::fs::write(temp.path().join("nested/data"), "changed").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while Instant::now() < deadline {
            if native
                .receive(Duration::from_millis(100))
                .paths
                .contains(Path::new("nested/data"))
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "registered nested watch must deliver a native change"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_registration_refuses_scoped_symlink_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("escape")).unwrap();
        let mut native = Native::start(temp.path(), 32).unwrap();
        assert!(!native.update_directories(
            &BTreeSet::from([PathBuf::new(), PathBuf::from("escape")]),
            true
        ));
        assert_eq!(native.directories.len(), 1);
    }
    #[test]
    fn deleted_native_watch_releases_its_registration_slot() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["old", "new"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let mut native = Native::with_limit(temp.path(), 32, 2).unwrap();
        assert!(native.update_directories(
            &BTreeSet::from([PathBuf::new(), PathBuf::from("old")]),
            true
        ));
        std::fs::remove_dir(temp.path().join("old")).unwrap();
        // Let the actual backend consume its deletion/automatic removal before
        // reconciling our registrations. A known absent watch releases capacity.
        for _ in 0..3 {
            native.receive(Duration::from_millis(50));
        }
        assert!(native.update_directories(
            &BTreeSet::from([PathBuf::new(), PathBuf::from("new")]),
            true
        ));
        assert_eq!(native.directories.len(), 2);
        assert!(!native.directories.contains(Path::new("old")));
        std::fs::write(temp.path().join("new/data"), "new scope").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while Instant::now() < deadline {
            if native
                .receive(Duration::from_millis(100))
                .paths
                .contains(Path::new("new/data"))
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "a released slot must admit a functioning replacement watch"
        );
    }
}
