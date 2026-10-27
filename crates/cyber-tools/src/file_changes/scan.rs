//! Handle-relative, metadata-only traversal. Directory symlinks are never followed;
//! a failed, cancelled or limited traversal cannot prove absent files were deleted.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use super::state::{Scan, Stamp};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, Metadata};

pub(super) struct Limits {
    pub entries: usize,
    pub depth: usize,
    pub path_bytes: usize,
}

fn stamp(metadata: &Metadata) -> Stamp {
    #[cfg(unix)]
    let identity = {
        use cap_std::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    #[cfg(not(unix))]
    let identity = None;
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

pub(super) fn scan(
    root: &Dir,
    excluded: &dyn Fn(&Path) -> bool,
    limits: &Limits,
    cancelled: &AtomicBool,
) -> Scan {
    let mut out = Scan {
        complete: true,
        ..Scan::default()
    };
    let Ok(root) = root.try_clone() else {
        out.complete = false;
        return out;
    };
    out.directories.insert(PathBuf::new());
    let mut pending = vec![(root, PathBuf::new(), 0usize)];
    let mut inspected = 0usize;
    while let Some((directory, relative, depth)) = pending.pop() {
        let Ok(entries) = directory.entries() else {
            out.complete = false;
            continue;
        };
        for entry in entries {
            if cancelled.load(Ordering::Acquire) || inspected >= limits.entries {
                out.complete = false;
                return out;
            }
            inspected += 1;
            let Ok(entry) = entry else {
                out.complete = false;
                continue;
            };
            let name = entry.file_name();
            let path = relative.join(&name);
            if excluded(&path) {
                continue;
            }
            let Some(text) = path.to_str() else {
                out.complete = false;
                continue;
            };
            if text.len() > limits.path_bytes {
                out.complete = false;
                continue;
            }
            let Ok(metadata) = directory.symlink_metadata(&name) else {
                out.complete = false;
                continue;
            };
            if metadata.is_dir() {
                if depth >= limits.depth {
                    out.complete = false;
                    continue;
                }
                match directory.open_dir_nofollow(&name) {
                    Ok(child) => {
                        out.directories.insert(path.clone());
                        pending.push((child, path, depth + 1));
                    }
                    Err(_) => out.complete = false,
                }
            } else if metadata.is_file() {
                out.files.insert(path, stamp(&metadata));
            }
        }
    }
    out
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
}
