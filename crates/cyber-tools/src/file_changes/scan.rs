//! Handle-relative, metadata-only traversal. Directory symlinks are never followed;
//! a failed, cancelled or limited traversal cannot prove absent files were deleted.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use super::{
    links::{self, Aliases},
    state::{Scan, Stamp},
};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, Metadata};

pub(super) struct Limits {
    pub entries: usize,
    pub depth: usize,
    pub path_bytes: usize,
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
    let mut aliases = Aliases::default();
    while let Some((directory, relative, depth)) = pending.pop() {
        let Ok(entries) = directory.entries() else {
            out.complete = false;
            continue;
        };
        for entry in entries {
            if cancelled.load(Ordering::Acquire) || inspected >= limits.entries {
                out.complete = false;
                return finish(out, aliases);
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
            let relative_name = text.replace(std::path::MAIN_SEPARATOR, "/");
            if text.len() > limits.path_bytes
                || !cyber_core::hooks::file_observations::relative_name(&relative_name)
            {
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
                aliases.observe(&path, &metadata);
                out.files.insert(path, stamp(&metadata));
            }
        }
    }
    finish(out, aliases)
}

fn finish(mut scan: Scan, aliases: Aliases) -> Scan {
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
