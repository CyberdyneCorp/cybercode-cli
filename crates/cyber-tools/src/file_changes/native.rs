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

#[derive(Debug, Default)]
pub(super) struct Hints {
    pub paths: BTreeSet<PathBuf>,
    pub rescan: bool,
}

pub(super) struct Native {
    // Drop stops native observation before its receiver is disposed.
    _watcher: RecommendedWatcher,
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
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            receiver,
            overflow,
        })
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
}
