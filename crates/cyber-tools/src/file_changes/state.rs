//! Bounded reconciliation state for file observations. No contents or writer
//! identities enter this state. Missing entries in an incomplete scan are unknown,
//! never evidence of deletion.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) use cyber_core::hooks::file_observations::Stamp;

#[derive(Clone, Debug, Default)]
pub(super) struct Scan {
    pub files: BTreeMap<PathBuf, Stamp>,
    pub directories: BTreeSet<PathBuf>,
    /// Known names with unproven alias scope or omitted by publication budgets.
    /// Their absence from `files` is not deletion evidence or a new metadata stamp.
    pub suppressed: BTreeSet<PathBuf>,
    /// Known private entry/subtree identities. Previously public names beneath
    /// them are retired without reporting an observed deletion.
    pub private_paths: BTreeSet<PathBuf>,
    /// False for entry limits, cancellation, unreadable directories or traversal
    /// errors. A later complete scan can resolve retained, unseen entries.
    /// `suppressed` names remain unknown independently of enumeration completeness.
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Created,
    Modified,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Change {
    pub path: PathBuf,
    pub kind: Kind,
}

/// Reconciliation computes a complete bounded delta before advancing state.
/// If that delta would exceed the limit, preserve the prior state for a retry;
/// taking only a prefix would silently lose the remaining observations.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reconciled {
    Changes {
        changes: Vec<Change>,
        complete: bool,
    },
    Overflow,
}

#[derive(Debug, Default)]
pub(super) struct State {
    files: BTreeMap<PathBuf, Stamp>,
}

impl State {
    pub fn baseline(files: BTreeMap<PathBuf, Stamp>) -> Self {
        Self { files }
    }

    #[cfg(test)]
    pub fn files(&self) -> &BTreeMap<PathBuf, Stamp> {
        &self.files
    }

    /// A newly protected path is no longer observable. Forgetting its old stamp
    /// must not disclose a synthetic deletion when the next scan omits it.
    #[cfg(test)]
    pub fn restrict(&mut self, permitted: impl Fn(&Path) -> bool) {
        self.files.retain(|path, _| permitted(path));
    }

    #[cfg(test)]
    pub fn reconcile(
        &mut self,
        scan: Scan,
        notified: &BTreeSet<PathBuf>,
        limit: usize,
    ) -> Reconciled {
        self.reconcile_bounded(scan, notified, limit, limit)
    }

    pub fn reconcile_bounded(
        &mut self,
        scan: Scan,
        notified: &BTreeSet<PathBuf>,
        files_limit: usize,
        changes_limit: usize,
    ) -> Reconciled {
        let mut changes = Vec::new();
        for (path, stamp) in &scan.files {
            let kind = match self.files.get(path) {
                None => Some(Kind::Created),
                Some(previous) if previous != stamp || notified.contains(path) => {
                    Some(Kind::Modified)
                }
                Some(_) => None,
            };
            if let Some(kind) = kind {
                changes.push(Change {
                    path: path.clone(),
                    kind,
                });
            }
        }
        if scan.complete {
            changes.extend(
                self.files
                    .keys()
                    .filter(|path| {
                        !scan.files.contains_key(*path) && !scan.suppressed.contains(*path)
                    })
                    .map(|path| Change {
                        path: path.clone(),
                        kind: Kind::Removed,
                    }),
            );
        }
        let retained = if scan.complete {
            scan.files.len()
                + self
                    .files
                    .keys()
                    .filter(|path| {
                        scan.suppressed.contains(*path) && !scan.files.contains_key(*path)
                    })
                    .count()
        } else {
            self.files.len()
                + scan
                    .files
                    .keys()
                    .filter(|path| !self.files.contains_key(*path))
                    .count()
        };
        if changes.len() > changes_limit || retained > files_limit {
            return Reconciled::Overflow;
        }
        if scan.complete {
            self.files.retain(|path, _| scan.suppressed.contains(path));
        }
        self.files.extend(scan.files);
        changes.sort_by(|left, right| left.path.cmp(&right.path));
        Reconciled::Changes {
            changes,
            complete: scan.complete,
        }
    }
}

/// A finite dispatch allowance survives quiet periods: a delayed/background hook
/// cannot evade the feedback bound by sleeping between writes. Only the owner of
/// a real user acknowledgement may replenish it. Exhaustion pauses handlers, not
/// recording/reconciliation, and must be visible to the user.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct DispatchBudget {
    remaining: usize,
}

#[cfg(test)]
impl DispatchBudget {
    pub fn new(remaining: usize) -> Self {
        Self { remaining }
    }

    pub fn admit(&mut self, count: usize) -> bool {
        if count > self.remaining {
            return false;
        }
        self.remaining -= count;
        true
    }

    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

/// A native callback can contain attacker-controlled absolute paths. Keep only
/// normalized relative names; scope and protected-root checks belong to the
/// filesystem adapter before an entry reaches this engine.
pub(super) fn relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(identity: u64) -> Stamp {
        Stamp {
            length: 8,
            modified_ns: Some(100),
            identity: Some((1, identity)),
        }
    }

    fn files(entries: &[(&str, u64)]) -> BTreeMap<PathBuf, Stamp> {
        entries
            .iter()
            .map(|(path, id)| (PathBuf::from(path), stamp(*id)))
            .collect()
    }

    #[test]
    fn atomic_replace_rename_and_delete_reconcile_without_writer_claims() {
        let mut state =
            State::baseline(files(&[("same-size", 1), ("old-name", 2), ("deleted", 3)]));
        let scan = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("same-size", 4), ("new-name", 2)]),
            complete: true,
        };
        let actual = state.reconcile(scan, &BTreeSet::new(), 20);
        assert_eq!(
            actual,
            Reconciled::Changes {
                changes: vec![
                    Change {
                        path: "deleted".into(),
                        kind: Kind::Removed
                    },
                    Change {
                        path: "new-name".into(),
                        kind: Kind::Created
                    },
                    Change {
                        path: "old-name".into(),
                        kind: Kind::Removed
                    },
                    Change {
                        path: "same-size".into(),
                        kind: Kind::Modified
                    },
                ],
                complete: true
            }
        );
    }

    #[test]
    fn incomplete_traversal_preserves_unseen_entries_until_complete_reconciliation() {
        let mut state = State::baseline(files(&[("unreadable/data", 1), ("kept", 2)]));
        let partial = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("kept", 3)]),
            complete: false,
        };
        assert!(matches!(
            state.reconcile(partial, &BTreeSet::new(), 20),
            Reconciled::Changes {
                complete: false,
                ..
            }
        ));
        assert!(state.files().contains_key(Path::new("unreadable/data")));
        let complete = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("kept", 3)]),
            complete: true,
        };
        assert_eq!(
            state.reconcile(complete, &BTreeSet::new(), 20),
            Reconciled::Changes {
                changes: vec![Change {
                    path: "unreadable/data".into(),
                    kind: Kind::Removed
                }],
                complete: true
            }
        );
    }

    #[test]
    fn delta_overflow_does_not_consume_unreported_changes() {
        let original = files(&[("before", 1)]);
        let mut state = State::baseline(original.clone());
        let scan = || Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("after", 2)]),
            complete: true,
        };
        assert_eq!(
            state.reconcile(scan(), &BTreeSet::new(), 1),
            Reconciled::Overflow
        );
        assert_eq!(state.files(), &original);
        let Reconciled::Changes { changes, .. } = state.reconcile(scan(), &BTreeSet::new(), 2)
        else {
            panic!("bounded retry must reconcile")
        };
        assert_eq!(changes.len(), 2);
        assert_eq!(state.files(), &files(&[("after", 2)]));
    }

    #[test]
    fn native_write_is_not_lost_when_size_and_timestamp_do_not_change() {
        let mut state = State::baseline(files(&[("data", 1)]));
        let notified = BTreeSet::from([PathBuf::from("data")]);
        assert_eq!(
            state.reconcile(
                Scan {
                    directories: Default::default(),
                    suppressed: Default::default(),
                    private_paths: Default::default(),
                    files: files(&[("data", 1)]),
                    complete: true
                },
                &notified,
                1
            ),
            Reconciled::Changes {
                changes: vec![Change {
                    path: "data".into(),
                    kind: Kind::Modified
                }],
                complete: true
            }
        );
    }

    #[test]
    fn feedback_allowance_cannot_reset_itself_or_partially_admit_a_batch() {
        let mut budget = DispatchBudget::new(3);
        assert!(budget.admit(2));
        assert!(!budget.admit(2));
        assert_eq!(budget.remaining(), 1);
        assert!(budget.admit(1));
        assert!(!budget.admit(1));
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn policy_revocation_does_not_emit_a_protected_path_as_deleted() {
        let mut state = State::baseline(files(&[("private/data", 1), ("public", 2)]));
        state.restrict(|path| !path.starts_with("private"));
        let scan = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("public", 2)]),
            complete: true,
        };
        assert_eq!(
            state.reconcile(scan, &BTreeSet::new(), 10),
            Reconciled::Changes {
                changes: vec![],
                complete: true
            }
        );
    }

    #[test]
    fn repeated_partial_scans_cannot_grow_retained_state_without_bound() {
        let mut state = State::baseline(files(&[("unseen", 1)]));
        let first = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("new", 2)]),
            complete: false,
        };
        assert!(matches!(
            state.reconcile(first, &BTreeSet::new(), 2),
            Reconciled::Changes { .. }
        ));
        let second = Scan {
            directories: Default::default(),
            suppressed: Default::default(),
            private_paths: Default::default(),
            files: files(&[("another", 3)]),
            complete: false,
        };
        assert_eq!(
            state.reconcile(second, &BTreeSet::new(), 2),
            Reconciled::Overflow
        );
        assert_eq!(state.files().len(), 2);
        assert!(!state.files().contains_key(Path::new("another")));
    }

    #[test]
    fn callback_names_reject_root_prefix_parent_and_empty_paths() {
        for path in ["", "..", "../secret", "/repo/data", "a/../../secret"] {
            assert!(!relative_path(Path::new(path)), "{path}");
        }
        assert!(relative_path(Path::new("nested/data")));
    }
    #[test]
    fn unproven_aliases_retain_prior_stamps_without_synthetic_deletions() {
        let original = files(&[("alias", 1), ("deleted", 2), ("public", 3)]);
        let mut state = State::baseline(original.clone());
        let actual = state.reconcile(
            Scan {
                files: files(&[("public", 4)]),
                directories: Default::default(),
                suppressed: BTreeSet::from([PathBuf::from("alias")]),
                private_paths: Default::default(),
                complete: true,
            },
            &BTreeSet::from([PathBuf::from("alias")]),
            3,
        );
        assert_eq!(
            actual,
            Reconciled::Changes {
                changes: vec![
                    Change {
                        path: "deleted".into(),
                        kind: Kind::Removed
                    },
                    Change {
                        path: "public".into(),
                        kind: Kind::Modified
                    },
                ],
                complete: true
            }
        );
        assert_eq!(
            state.files()[Path::new("alias")],
            original[Path::new("alias")]
        );
        assert_eq!(state.files().len(), 2);
    }
}
