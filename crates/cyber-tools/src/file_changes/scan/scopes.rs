//! Revalidate directory incarnations after traversal, including partial scans.
//! A retained descriptor follows a moved directory; it is not proof that later
//! metadata still belongs beneath the observation root.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cap_fs_ext::DirExt;
use cap_std::fs::Dir;

use super::super::{links, state::Scan};

struct Node {
    path: PathBuf,
    identity: Option<(u64, u64)>,
}
pub(super) struct Catalog(Vec<Node>);
struct Opened {
    index: usize,
    directory: Dir,
}
fn identity(directory: &Dir) -> Option<(u64, u64)> {
    directory
        .dir_metadata()
        .ok()
        .and_then(|metadata| links::identity(&metadata))
}
fn rejected(path: &Path, prefixes: &BTreeSet<PathBuf>) -> bool {
    path.ancestors().any(|parent| prefixes.contains(parent))
}
impl Catalog {
    pub fn new(root: &Dir) -> Self {
        Self(vec![Node {
            path: PathBuf::new(),
            identity: identity(root),
        }])
    }
    pub fn record(&mut self, path: PathBuf, directory: &Dir) {
        self.0.push(Node {
            path,
            identity: identity(directory),
        });
    }

    /// The first pass records directories in preorder. Replay that bounded
    /// shape without enumerating files, keeping only its open ancestor chain.
    pub fn retain(self, scan: &mut Scan, root: &Dir, cancel: &AtomicBool) {
        let Ok(directory) = root.try_clone() else {
            return discard(scan);
        };
        if self.0[0].identity.is_none() || identity(&directory) != self.0[0].identity {
            return discard(scan);
        }
        let mut stack = vec![Opened {
            index: 0,
            directory,
        }];
        let mut invalid = BTreeSet::new();
        for (index, node) in self.0.iter().enumerate().skip(1) {
            if cancel.load(Ordering::Acquire) {
                return discard(scan);
            }
            if rejected(&node.path, &invalid) {
                continue;
            }
            let parent = node.path.parent().unwrap_or(Path::new(""));
            while stack.len() > 1 && self.0[stack.last().unwrap().index].path != parent {
                self.close(&mut stack, &mut invalid);
            }
            let current = stack.last().unwrap();
            if self.0[current.index].path != parent || rejected(&node.path, &invalid) {
                invalid.insert(node.path.clone());
                continue;
            }
            let opened = current
                .directory
                .open_dir_nofollow(node.path.file_name().unwrap());
            match opened {
                Ok(directory)
                    if node.identity.is_some() && identity(&directory) == node.identity =>
                {
                    stack.push(Opened { index, directory })
                }
                _ => {
                    invalid.insert(node.path.clone());
                }
            }
        }
        while stack.len() > 1 {
            self.close(&mut stack, &mut invalid);
        }
        if cancel.load(Ordering::Acquire) {
            return discard(scan);
        }
        if !invalid.is_empty() {
            scan.complete = false;
            scan.files.retain(|path, _| !rejected(path, &invalid));
            scan.directories.retain(|path| !rejected(path, &invalid));
            scan.suppressed.retain(|path| !rejected(path, &invalid));
            scan.private_paths.retain(|path| !rejected(path, &invalid));
        }
    }
    fn close(&self, stack: &mut Vec<Opened>, invalid: &mut BTreeSet<PathBuf>) {
        let closed = stack.pop().unwrap();
        let node = &self.0[closed.index];
        let parent = &stack.last().unwrap().directory;
        let current = parent
            .symlink_metadata(node.path.file_name().unwrap())
            .ok()
            .filter(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            .and_then(|metadata| links::identity(&metadata));
        if current.is_none() || current != node.identity {
            invalid.insert(node.path.clone());
        }
    }
}
fn discard(scan: &mut Scan) {
    scan.complete = false;
    scan.files.clear();
    scan.directories.clear();
    scan.suppressed.clear();
    scan.private_paths.clear();
}
