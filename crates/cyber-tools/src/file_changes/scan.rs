//! Handle-relative, metadata-only traversal. Directory symlinks are never followed;
//! a failed, cancelled or limited traversal cannot prove absent files were deleted.
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::UNIX_EPOCH;

use super::{
    links::{self, Aliases},
    state::{Scan, Stamp},
};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, Metadata};
#[path = "scan/scopes.rs"]
mod scopes;

pub(super) struct Limits {
    pub entries: usize,
    pub depth: usize,
    pub path_bytes: usize,
}

/// Keep ownership of blocking traversal until it settles. Dropping the blocking
/// task handle on cancellation would leave filesystem work behind the actor.
pub(super) async fn run(
    root: Dir,
    excluded: impl Fn(&Path) -> bool + Send + 'static,
    excluded_metadata: impl Fn(&Metadata) -> bool + Send + 'static,
    limits: Limits,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Scan, String> {
    if cancel.is_cancelled() {
        return Err("file observation scan cancelled".into());
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let worker_flag = Arc::clone(&interrupted);
    let mut task = tokio::task::spawn_blocking(move || {
        scan_with_metadata(&root, &excluded, &excluded_metadata, &limits, &worker_flag)
    });
    tokio::select! {
        biased;
        () = cancel.cancelled() => {
            interrupted.store(true, Ordering::Release);
            // A synchronous metadata operation already in flight cannot be
            // interrupted, but traversal stops at the next cancellation check.
            task.await.map_err(|error| error.to_string())?;
            Err("file observation scan cancelled".into())
        }
        result = &mut task => result.map_err(|error| error.to_string()),
    }
}

fn stamp(metadata: &Metadata) -> Stamp {
    let identity = links::identity(metadata);
    Stamp {
        length: metadata.len(),
        modified_ns: metadata
            .modified()
            .ok()
            .and_then(|time| time.into_std().duration_since(UNIX_EPOCH).ok())
            .and_then(|time| time.as_nanos().try_into().ok()),
        identity,
    }
}

struct Frame {
    directory: Dir,
    entries: cap_std::fs::ReadDir,
    relative: PathBuf,
    depth: usize,
}
impl Frame {
    fn new(directory: Dir, relative: PathBuf, depth: usize) -> std::io::Result<Self> {
        let entries = directory.entries()?;
        Ok(Self {
            directory,
            entries,
            relative,
            depth,
        })
    }
}

#[cfg(test)]
pub(super) fn scan(
    root: &Dir,
    excluded: &dyn Fn(&Path) -> bool,
    limits: &Limits,
    cancelled: &AtomicBool,
) -> Scan {
    scan_with_metadata(root, excluded, &|_| false, limits, cancelled)
}

pub(super) fn scan_with_metadata(
    root: &Dir,
    excluded: &dyn Fn(&Path) -> bool,
    excluded_metadata: &dyn Fn(&Metadata) -> bool,
    limits: &Limits,
    cancelled: &AtomicBool,
) -> Scan {
    let mut out = Scan {
        complete: true,
        ..Scan::default()
    };
    match root.dir_metadata() {
        Ok(metadata) if excluded_metadata(&metadata) => {
            out.complete = false;
            return out;
        }
        Err(_) => {
            out.complete = false;
            return out;
        }
        _ => {}
    }
    let initial = root
        .try_clone()
        .and_then(|root| Frame::new(root, PathBuf::new(), 0));
    let Ok(first) = initial else {
        out.complete = false;
        return out;
    };
    out.directories.insert(PathBuf::new());
    // Retain only the open ancestor chain. Opening all siblings before visiting
    // them lets a shallow wide tree exhaust process descriptors and permanently
    // omit its tail on every reconciliation pass.
    let mut scopes = scopes::Catalog::new(root);
    let mut pending = vec![first];
    let mut inspected = 0usize;
    let mut aliases = Aliases::default();
    while let Some(frame) = pending.last_mut() {
        if cancelled.load(Ordering::Acquire) {
            out.complete = false;
            break;
        }
        let Some(entry) = frame.entries.next() else {
            pending.pop();
            continue;
        };
        if inspected >= limits.entries {
            out.complete = false;
            break;
        }
        inspected += 1;
        let Ok(entry) = entry else {
            out.complete = false;
            continue;
        };
        let name = entry.file_name();
        let path = frame.relative.join(&name);
        if excluded(&path) {
            continue;
        }
        let Some(text) = path.to_str() else {
            out.complete = false;
            continue;
        };
        let relative_name = text.replace(std::path::MAIN_SEPARATOR, "/");
        if text.len() > limits.path_bytes
            || !cyber_core::hooks::file_observations::relative_name(&relative_name)
        {
            out.complete = false;
            continue;
        }
        let Ok(metadata) = frame.directory.symlink_metadata(&name) else {
            out.complete = false;
            continue;
        };
        if excluded_metadata(&metadata) {
            out.private_paths.insert(path);
            continue;
        }
        if metadata.is_dir() {
            if frame.depth >= limits.depth {
                out.complete = false;
                continue;
            }
            let opened = frame.directory.open_dir_nofollow(&name);
            let child = opened.and_then(|child| {
                let current = child.dir_metadata()?;
                if excluded_metadata(&current) {
                    out.private_paths.insert(path.clone());
                    return Ok(None);
                }
                if super::links::identity(&current) != super::links::identity(&metadata) {
                    return Err(std::io::Error::other(
                        "observed directory changed while opening",
                    ));
                }
                Frame::new(child, path.clone(), frame.depth + 1).map(Some)
            });
            match child {
                Ok(Some(child)) => {
                    scopes.record(path.clone(), &child.directory);
                    out.directories.insert(path);
                    pending.push(child);
                }
                Ok(None) => {}
                Err(_) => out.complete = false,
            }
        } else if metadata.is_file() {
            aliases.observe(&path, &metadata);
            out.files.insert(path, stamp(&metadata));
        }
    }
    drop(pending);
    finish(out, aliases, scopes, root, cancelled)
}

fn finish(
    mut scan: Scan,
    mut aliases: Aliases,
    scopes: scopes::Catalog,
    root: &Dir,
    cancelled: &AtomicBool,
) -> Scan {
    scopes.retain(&mut scan, root, cancelled);
    aliases.retain_paths(|path| scan.files.contains_key(path));
    scan.suppressed = aliases.unproven();
    scan.files.retain(|path, _| !scan.suppressed.contains(path));
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> Limits {
        Limits {
            entries: 100,
            depth: 8,
            path_bytes: 4096,
        }
    }
    fn root(path: &Path) -> Dir {
        Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap()
    }

    // cap-std deliberately prevents this in-flight rename on Windows. The
    // portable finalization test below covers moves after traversal pins close.
    #[cfg(unix)]
    #[test]
    fn directories_moved_out_during_full_or_limited_traversal_publish_no_new_metadata() {
        for entries in [100, 2] {
            let inside = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let public = inside.path().join("public");
            let private = outside.path().join("private");
            std::fs::create_dir(&public).unwrap();
            for name in ["data", "tail"] {
                std::fs::write(public.join(name), "public").unwrap();
            }
            if entries == 100 {
                std::fs::write(inside.path().join("healthy"), "still scoped").unwrap();
            }
            let moved = std::cell::Cell::new(false);
            let actual = scan(
                &root(inside.path()),
                &|path| {
                    if path.parent() == Some(Path::new("public")) && !moved.replace(true) {
                        std::fs::rename(&public, &private).unwrap();
                        for name in ["data", "tail"] {
                            std::fs::write(
                                private.join(name),
                                "private metadata after leaving the Location",
                            )
                            .unwrap();
                        }
                    }
                    false
                },
                &Limits {
                    entries,
                    ..limits()
                },
                &AtomicBool::new(false),
            );
            assert!(moved.get());
            assert!(
                !actual.complete,
                "changed directory scope is incomplete evidence"
            );
            assert!(
                actual.files.keys().all(|path| !path.starts_with("public")),
                "moved directory metadata must not enter observations"
            );
            assert!(!actual.directories.contains(Path::new("public")));
            if entries == 100 {
                assert!(actual.files.contains_key(Path::new("healthy")));
            }
        }
    }

    // cap-std deliberately prevents this in-flight rename on Windows. The
    // portable finalization test below covers moves after traversal pins close.
    #[cfg(unix)]
    #[test]
    fn moved_directory_links_do_not_count_as_scoped_aliases_of_remaining_files() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let public = inside.path().join("public");
        std::fs::create_dir(&public).unwrap();
        std::fs::write(inside.path().join("remaining"), "linked").unwrap();
        std::fs::hard_link(inside.path().join("remaining"), public.join("linked")).unwrap();
        let moved = std::cell::Cell::new(false);
        let actual = scan(
            &root(inside.path()),
            &|path| {
                if path == Path::new("public/linked") && !moved.replace(true) {
                    std::fs::rename(&public, outside.path().join("private")).unwrap();
                }
                false
            },
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(moved.get());
        assert!(
            actual.files.is_empty(),
            "the remaining link now has an unaccounted outside alias"
        );
        assert!(actual.suppressed.contains(Path::new("remaining")));
        assert!(!actual.suppressed.contains(Path::new("public/linked")));
    }

    #[test]
    fn finalization_discards_moved_subtrees_before_counting_remaining_hard_links() {
        for complete in [true, false] {
            let inside = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::create_dir(inside.path().join("public")).unwrap();
            std::fs::write(inside.path().join("remaining"), "linked").unwrap();
            std::fs::hard_link(
                inside.path().join("remaining"),
                inside.path().join("public/linked"),
            )
            .unwrap();
            std::fs::write(inside.path().join("public/standalone"), "scoped initially").unwrap();
            std::fs::write(inside.path().join("healthy"), "still scoped").unwrap();
            let root = root(inside.path());
            let child = root.open_dir_nofollow("public").unwrap();
            let mut scopes = scopes::Catalog::new(&root);
            scopes.record("public".into(), &child);
            let mut candidate = Scan {
                complete,
                ..Scan::default()
            };
            candidate
                .directories
                .extend([PathBuf::new(), "public".into()]);
            let mut aliases = Aliases::default();
            // Capture real metadata while both aliases are in scope, then close
            // traversal pins exactly as scan() does before calling finish().
            for (directory, name, path) in [
                (&root, "remaining", "remaining"),
                (&root, "healthy", "healthy"),
                (&child, "linked", "public/linked"),
                (&child, "standalone", "public/standalone"),
            ] {
                let metadata = directory.symlink_metadata(name).unwrap();
                aliases.observe(Path::new(path), &metadata);
                candidate.files.insert(path.into(), stamp(&metadata));
            }
            drop(child);
            std::fs::rename(inside.path().join("public"), outside.path().join("private")).unwrap();
            let actual = finish(candidate, aliases, scopes, &root, &AtomicBool::new(false));
            assert!(!actual.complete);
            assert_eq!(
                actual.files.keys().cloned().collect::<Vec<_>>(),
                vec![PathBuf::from("healthy")]
            );
            assert!(!actual.directories.contains(Path::new("public")));
            assert_eq!(
                actual.suppressed,
                std::collections::BTreeSet::from([PathBuf::from("remaining")])
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_traversal_sharing_lock_releases_when_the_scan_settles() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let public = inside.path().join("public");
        let private = outside.path().join("private");
        std::fs::create_dir(&public).unwrap();
        std::fs::write(public.join("data"), "scoped").unwrap();
        let checked = std::cell::Cell::new(false);
        let actual = scan(
            &root(inside.path()),
            &|path| {
                if path == Path::new("public/data") {
                    let error = std::fs::rename(&public, &private).unwrap_err();
                    assert_eq!(
                        error.raw_os_error(),
                        Some(32),
                        "unexpected rename refusal: {error}"
                    );
                    checked.set(true);
                }
                false
            },
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(checked.get());
        assert!(actual.complete);
        assert!(actual.files.contains_key(Path::new("public/data")));
        std::fs::rename(&public, &private)
            .expect("settled scans must release transient rename locks");
        let after = scan(
            &root(inside.path()),
            &|_| false,
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(after.complete);
        assert!(
            after.files.is_empty(),
            "outside metadata must not follow the moved directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wide_tree_uses_depth_bounded_descriptors_and_keeps_tail_coverage() {
        // Isolate process-wide FD measurements from parallel native-watch tests.
        if std::env::var_os("CYBER_FILE_SCAN_FD_CHILD").is_none() {
            let name = std::thread::current().name().unwrap().to_owned();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--nocapture"])
                .env("CYBER_FILE_SCAN_FD_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "isolated scanner failed: {} {}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for index in 0..512 {
            let path = temp.path().join(format!("tree-{index:03}"));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("public"), "metadata").unwrap();
        }
        let count = || std::fs::read_dir("/dev/fd").unwrap().count();
        let before = count();
        let peak = std::cell::Cell::new(before);
        let actual = scan(
            &root(temp.path()),
            &|_| {
                peak.set(peak.get().max(count()));
                false
            },
            &Limits {
                entries: 2048,
                depth: 8,
                path_bytes: 4096,
            },
            &AtomicBool::new(false),
        );
        assert!(
            actual.complete,
            "a wide shallow tree must not lose its tail through descriptor pressure"
        );
        assert_eq!(actual.files.len(), 512);
        assert!(
            peak.get() <= before + 16,
            "descriptor count grew with tree width: before={before}, peak={}",
            peak.get()
        );
    }

    #[tokio::test]
    async fn normal_worker_returns_metadata_and_pre_cancelled_worker_does_not_traverse() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("public"), "metadata").unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let actual = run(root(temp.path()), |_| false, |_| false, limits(), &cancel)
            .await
            .unwrap();
        assert!(actual.complete);
        assert!(actual.files.contains_key(Path::new("public")));
        cancel.cancel();
        let cancelled = run(
            root(temp.path()),
            |_| panic!("cancelled worker must not traverse"),
            |_| false,
            limits(),
            &cancel,
        )
        .await;
        assert!(cancelled.is_err());
    }

    #[tokio::test]
    async fn cancellation_interrupts_traversal_but_joins_the_in_flight_metadata_step() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        for index in 0..20 {
            std::fs::write(temp.path().join(format!("entry-{index}")), "metadata").unwrap();
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let inspected = Arc::new(AtomicUsize::new(0));
        let (release, held) = std::sync::mpsc::channel();
        let directory = root(temp.path());
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let entered = Arc::clone(&entered);
            let inspected = Arc::clone(&inspected);
            async move {
                run(
                    directory,
                    move |_| {
                        if inspected.fetch_add(1, Ordering::SeqCst) == 0 {
                            entered.notify_one();
                            held.recv_timeout(Duration::from_secs(5)).unwrap();
                        }
                        false
                    },
                    |_| false,
                    limits(),
                    &cancel,
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        cancel.cancel();
        tokio::task::yield_now().await;
        assert!(
            !task.is_finished(),
            "cancellation must join the in-flight step"
        );
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_err(),
            "a cancelled scan cannot supply a baseline or delta"
        );
        assert_eq!(
            inspected.load(Ordering::SeqCst),
            1,
            "do not continue traversal after cancellation"
        );
    }

    #[test]
    fn protected_subtrees_are_excluded_without_reading_contents() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("private")).unwrap();
        std::fs::write(temp.path().join("private/data"), "not observed").unwrap();
        std::fs::write(temp.path().join("public"), "observed").unwrap();
        let actual = scan(
            &root(temp.path()),
            &|path| path.starts_with("private"),
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(actual.complete);
        assert!(!actual.directories.contains(Path::new("private")));
        assert_eq!(
            actual.files.keys().collect::<Vec<_>>(),
            vec![Path::new("public")]
        );
    }

    #[test]
    fn limits_and_cancellation_leave_incomplete_evidence() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("one"), "one").unwrap();
        std::fs::write(temp.path().join("two"), "two").unwrap();
        let root = root(temp.path());
        let mut bound = limits();
        bound.entries = 1;
        let actual = scan(&root, &|_| false, &bound, &AtomicBool::new(false));
        assert!(!actual.complete);
        assert_eq!(actual.files.len(), 1);
        let actual = scan(&root, &|_| false, &limits(), &AtomicBool::new(true));
        assert!(!actual.complete);
        assert!(actual.files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn directory_and_file_symlinks_do_not_expose_outside_metadata() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), inside.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("private"),
            inside.path().join("linked-file"),
        )
        .unwrap();
        let actual = scan(
            &root(inside.path()),
            &|_| false,
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(actual.complete);
        assert!(actual.files.is_empty());
    }

    #[test]
    fn depth_limit_cannot_look_like_a_nested_deletion() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("nested/deep")).unwrap();
        std::fs::write(temp.path().join("nested/deep/data"), "data").unwrap();
        let mut bound = limits();
        bound.depth = 0;
        let actual = scan(
            &root(temp.path()),
            &|_| false,
            &bound,
            &AtomicBool::new(false),
        );
        assert!(!actual.complete);
        assert!(actual.files.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn unsupported_names_are_not_reinterpreted_as_nested_or_stream_paths() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["a\\b", "a:b"] {
            std::fs::write(temp.path().join(name), "opaque").unwrap();
        }
        let actual = scan(
            &root(temp.path()),
            &|_| false,
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(!actual.complete);
        assert!(actual.files.is_empty());
    }
    #[test]
    fn nested_metadata_uses_each_entry_name_relative_to_its_pinned_directory() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("nested/deep")).unwrap();
        std::fs::write(temp.path().join("nested/deep/data"), "observed").unwrap();
        let actual = scan(
            &root(temp.path()),
            &|_| false,
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(
            actual.complete,
            "valid nested entries must not appear unreadable"
        );
        assert_eq!(
            actual.files.keys().collect::<Vec<_>>(),
            vec![Path::new("nested/deep/data")]
        );
        assert_eq!(actual.directories.len(), 3);
        assert_eq!(actual.files[Path::new("nested/deep/data")].length, 8);
    }
    #[test]
    fn fully_accounted_in_scope_hard_links_remain_observable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("nested")).unwrap();
        std::fs::write(temp.path().join("original"), "public").unwrap();
        std::fs::hard_link(
            temp.path().join("original"),
            temp.path().join("nested/alias"),
        )
        .unwrap();
        let actual = scan(
            &root(temp.path()),
            &|_| false,
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(actual.complete);
        assert!(actual.suppressed.is_empty());
        assert_eq!(actual.files.len(), 2);
        assert_eq!(
            actual.files[Path::new("original")].identity,
            actual.files[Path::new("nested/alias")].identity
        );
    }
    #[test]
    fn aliases_of_excluded_or_outside_files_do_not_publish_private_metadata() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("private")).unwrap();
        std::fs::write(temp.path().join("private/state"), "private metadata").unwrap();
        std::fs::hard_link(temp.path().join("private/state"), temp.path().join("alias")).unwrap();
        let actual = scan(
            &root(temp.path()),
            &|path| path.starts_with("private"),
            &limits(),
            &AtomicBool::new(false),
        );
        assert!(
            actual.complete,
            "permitted directory enumeration is complete"
        );
        assert!(actual.files.is_empty());
        assert_eq!(
            actual.suppressed,
            std::collections::BTreeSet::from([PathBuf::from("alias")])
        );
    }
    #[test]
    fn bounded_traversal_finalizes_alias_scope_before_returning_partial_evidence() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("first"), "public").unwrap();
        std::fs::hard_link(temp.path().join("first"), temp.path().join("second")).unwrap();
        let mut bound = limits();
        bound.entries = 1;
        let actual = scan(
            &root(temp.path()),
            &|_| false,
            &bound,
            &AtomicBool::new(false),
        );
        assert!(!actual.complete);
        assert!(actual.files.is_empty());
        assert_eq!(actual.suppressed.len(), 1);
    }
}
