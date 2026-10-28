//! Admission budgets for metadata, independent of filesystem enumeration proof.
//! Omitting a known name preserves its previous stamp; it is not a deletion.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::state::{Change, Scan};
use cyber_core::hooks::file_observations::{self as records, Stamp, Update};

// A maximum-length JSON-escaped Location/identity and the checkpoint fields fit
// here. Leave record validation authoritative, including for older checkpoints.
const HEADER_RESERVE: usize = 32 * 1024;
const FILE_BYTES: usize = records::MAX_RECORD_BYTES - HEADER_RESERVE;
const DELTA_HEADER_RESERVE: usize = 1024;

fn name(path: &Path) -> Option<String> {
    path.to_str()
        .map(|text| text.replace(std::path::MAIN_SEPARATOR, "/"))
}
fn cost(path: &Path, stamp: &Stamp) -> Option<usize> {
    records::entry_bytes(&name(path)?, stamp)
        .ok()?
        .checked_add(1)
}
struct Budget {
    used: usize,
    files: usize,
    bytes_limit: usize,
    files_limit: usize,
}
impl Budget {
    fn replace(&mut self, old: usize, new: Option<usize>, insertion: bool) -> bool {
        let Some(candidate) = new.and_then(|new| self.used.checked_sub(old)?.checked_add(new))
        else {
            return false;
        };
        if insertion && self.files >= self.files_limit {
            return false;
        }
        // A legacy valid checkpoint can exceed the new conservative allowance.
        // Preserve it and permit same-size/reducing updates, never additional growth.
        if candidate > self.bytes_limit && candidate > self.used {
            return false;
        }
        self.used = candidate;
        self.files += usize::from(insertion);
        true
    }
}

pub(super) fn retain(scan: &mut Scan, previous: &BTreeMap<PathBuf, Stamp>) {
    constrain(scan, previous, FILE_BYTES, records::MAX_FILES);
}
fn constrain(
    scan: &mut Scan,
    previous: &BTreeMap<PathBuf, Stamp>,
    bytes_limit: usize,
    files_limit: usize,
) {
    let mut budget = Budget {
        used: 2,
        files: 0,
        bytes_limit,
        files_limit,
    };
    for (path, stamp) in previous {
        if !scan.complete || scan.files.contains_key(path) || scan.suppressed.contains(path) {
            budget.used = budget
                .used
                .saturating_add(cost(path, stamp).unwrap_or(records::MAX_RECORD_BYTES));
            budget.files += 1;
        }
    }
    let mut omitted = BTreeSet::new();
    // Reserve old unknown stamps first and prioritize existing-file updates over
    // new lexically earlier names. Full enumeration can reclaim confirmed absence.
    existing(scan, previous, &mut budget, &mut omitted, false);
    existing(scan, previous, &mut budget, &mut omitted, true);
    for (path, stamp) in &scan.files {
        if !previous.contains_key(path) && !budget.replace(0, cost(path, stamp), true) {
            omitted.insert(path.clone());
        }
    }
    scan.files.retain(|path, _| !omitted.contains(path));
    // Completeness still describes enumeration. Known omitted names are tracked
    // separately so a byte limit cannot manufacture deletions or hide real ones.
    scan.suppressed.extend(omitted);
}

fn existing(
    scan: &Scan,
    previous: &BTreeMap<PathBuf, Stamp>,
    budget: &mut Budget,
    omitted: &mut BTreeSet<PathBuf>,
    growing: bool,
) {
    for (path, stamp) in &scan.files {
        let Some(old) = previous.get(path) else {
            continue;
        };
        let before = cost(path, old).unwrap_or(records::MAX_RECORD_BYTES);
        let after = cost(path, stamp);
        let grows = after.is_none_or(|after| after > before);
        if grows == growing && !budget.replace(before, after, false) {
            omitted.insert(path.clone());
        }
    }
}

fn update(change: &Change, files: &BTreeMap<String, Stamp>) -> Update {
    let path = name(&change.path).unwrap_or_default();
    Update {
        stamp: files.get(&path).cloned(),
        path,
    }
}
pub(super) fn updates(changes: &[Change], files: &BTreeMap<String, Stamp>) -> Vec<Update> {
    changes.iter().map(|change| update(change, files)).collect()
}
/// Reclaim confirmed removals and shrinking updates before growth. Every
/// intermediate journal candidate must fit, not only the final reconciled map.
pub(super) fn order(
    changes: &mut [Change],
    files: &BTreeMap<String, Stamp>,
    previous: &BTreeMap<String, Stamp>,
) {
    changes.sort_by_cached_key(|change| {
        let path = name(&change.path).unwrap_or_default();
        let priority = if matches!(change.kind, super::state::Kind::Removed) {
            0
        } else if let (Some(old), Some(new)) = (previous.get(&path), files.get(&path)) {
            if cost(&change.path, new)
                .zip(cost(&change.path, old))
                .is_some_and(|(new, old)| new <= old)
            {
                1
            } else {
                2
            }
        } else {
            2
        };
        (priority, change.path.clone())
    });
}

pub(super) fn batch_len(
    changes: &[Change],
    files: &BTreeMap<String, Stamp>,
    limit: usize,
) -> Result<usize, String> {
    let mut bytes = DELTA_HEADER_RESERVE;
    let mut count = 0;
    for change in changes.iter().take(limit) {
        let length = serde_json::to_vec(&update(change, files))
            .map_err(|error| error.to_string())?
            .len()
            + 1;
        if bytes.saturating_add(length) > records::MAX_RECORD_BYTES {
            break;
        }
        bytes += length;
        count += 1;
    }
    if count == 0 && !changes.is_empty() {
        return Err("file observation update cannot fit its bounded record".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::super::state::{Kind, Reconciled, State};
    use super::*;
    fn stamp(length: u64) -> Stamp {
        Stamp {
            length,
            modified_ns: Some(1),
            identity: Some((1, 1)),
        }
    }
    #[test]
    fn incomplete_scan_prioritizes_existing_updates_and_preserves_unknown_preimages() {
        let previous = BTreeMap::from([
            (PathBuf::from("z-old"), stamp(1)),
            (PathBuf::from("unseen"), stamp(1)),
        ]);
        let next = stamp(2);
        let limit = 2
            + cost(Path::new("z-old"), &next).unwrap()
            + cost(Path::new("unseen"), &stamp(1)).unwrap();
        let mut scan = Scan {
            files: BTreeMap::from([("a-new".into(), stamp(3)), ("z-old".into(), next.clone())]),
            complete: false,
            ..Scan::default()
        };
        constrain(&mut scan, &previous, limit, 2);
        assert!(scan.suppressed.contains(Path::new("a-new")));
        let mut state = State::baseline(previous);
        let Reconciled::Changes { changes, complete } =
            state.reconcile_bounded(scan, &BTreeSet::new(), 2, 4)
        else {
            panic!("budgeted metadata must remain reconcilable")
        };
        assert!(!complete);
        assert_eq!(
            changes,
            vec![Change {
                path: "z-old".into(),
                kind: Kind::Modified
            }]
        );
        assert_eq!(state.files()[Path::new("z-old")], next);
        assert!(state.files().contains_key(Path::new("unseen")));
    }
    #[test]
    fn complete_enumeration_reclaims_absence_but_never_deletes_a_budget_omission() {
        let previous = BTreeMap::from([(PathBuf::from("old"), stamp(1))]);
        let limit = 2 + cost(Path::new("old"), &stamp(1)).unwrap();
        let mut scan = Scan {
            files: BTreeMap::from([("old".into(), stamp(u64::MAX))]),
            complete: true,
            ..Scan::default()
        };
        constrain(&mut scan, &previous, limit, 1);
        assert!(scan.suppressed.contains(Path::new("old")));
        let mut state = State::baseline(previous.clone());
        let Reconciled::Changes { changes, .. } =
            state.reconcile_bounded(scan, &BTreeSet::new(), 1, 2)
        else {
            panic!("old stamp must fit")
        };
        assert!(changes.is_empty());
        assert_eq!(state.files(), &previous);
        let mut replacement = Scan {
            files: BTreeMap::from([("new".into(), stamp(1))]),
            complete: true,
            ..Scan::default()
        };
        constrain(&mut replacement, &previous, limit, 1);
        let Reconciled::Changes { changes, complete } =
            state.reconcile_bounded(replacement, &BTreeSet::new(), 1, 2)
        else {
            panic!("confirmed absence must free capacity")
        };
        assert!(complete);
        assert!(
            changes
                .iter()
                .any(|change| change.path == Path::new("old") && change.kind == Kind::Removed)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.path == Path::new("new") && change.kind == Kind::Created)
        );
    }
    #[test]
    fn older_valid_inventory_can_shrink_without_admitting_more_metadata() {
        let previous = BTreeMap::from([(PathBuf::from("old"), stamp(u64::MAX))]);
        let mut scan = Scan {
            files: BTreeMap::from([("old".into(), stamp(1)), ("new".into(), stamp(1))]),
            complete: true,
            ..Scan::default()
        };
        constrain(&mut scan, &previous, 1, 1);
        assert_eq!(scan.files, BTreeMap::from([("old".into(), stamp(1))]));
        assert!(scan.suppressed.contains(Path::new("new")));
    }
    #[test]
    fn escaped_path_batches_obey_real_delta_bytes_and_retain_every_update() {
        let changes: Vec<_> = (0..records::MAX_BATCH)
            .map(|index| Change {
                path: format!("{index:03}{}", "\u{1}".repeat(4090)).into(),
                kind: Kind::Modified,
            })
            .collect();
        let files = changes
            .iter()
            .map(|change| (name(&change.path).unwrap(), stamp(u64::MAX)))
            .collect();
        for files in [&BTreeMap::new(), &files] {
            let mut pending = changes.as_slice();
            let mut consumed = 0;
            assert!(batch_len(pending, files, 0).is_err());
            while !pending.is_empty() {
                let length = batch_len(pending, files, records::MAX_BATCH).unwrap();
                assert!(length > 0 && length < records::MAX_BATCH);
                let (batch, rest) = pending.split_at(length);
                let delta = records::Delta {
                    generation: records::new_generation(),
                    updates: updates(batch, files),
                    remaining: 0,
                    dispatch: false,
                    renewal: None,
                };
                delta.validate().unwrap();
                consumed += length;
                pending = rest;
            }
            assert_eq!(consumed, changes.len());
        }
    }
    #[test]
    fn checkpoint_header_reserve_covers_maximum_escaped_fields() {
        let empty = records::Checkpoint {
            directory: format!("/{}", "\u{1}".repeat(4095)),
            root_identity: "\u{1}".repeat(256),
            generation: records::new_generation(),
            files: BTreeMap::new(),
            remaining: records::FEEDBACK_LIMIT,
            requires_native_lease: false,
        };
        assert!(serde_json::to_vec(&empty).unwrap().len() < HEADER_RESERVE);
    }
    #[test]
    fn real_large_inventory_can_publish_a_bounded_partial_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let mut directory = temp.path().to_path_buf();
        for index in 0..6 {
            directory = directory.join(format!("{index}{}", "d".repeat(109)));
            std::fs::create_dir(&directory).unwrap();
        }
        for index in 0..4800 {
            std::fs::write(
                directory.join(format!("{index:04}{}", "f".repeat(196))),
                "x",
            )
            .unwrap();
        }
        let root =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let mut scan = super::super::scan::scan(
            &root,
            &|_| false,
            &super::super::scan::Limits {
                entries: records::MAX_FILES,
                depth: 64,
                path_bytes: 4096,
            },
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(scan.complete);
        assert_eq!(scan.files.len(), 4800);
        retain(&mut scan, &BTreeMap::new());
        assert!(!scan.suppressed.is_empty());
        assert!(!scan.files.is_empty() && scan.files.len() < 4800);
        let state = records::Checkpoint {
            directory: temp.path().canonicalize().unwrap().display().to_string(),
            root_identity: "bounded-root".into(),
            generation: records::new_generation(),
            files: scan
                .files
                .into_iter()
                .map(|(path, stamp)| (name(&path).unwrap(), stamp))
                .collect(),
            remaining: records::FEEDBACK_LIMIT,
            requires_native_lease: true,
        };
        state.validate().unwrap();
    }
    #[test]
    fn shrinking_updates_precede_growth_so_every_journal_candidate_fits() {
        let mut checkpoint = records::Checkpoint {
            directory: std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            root_identity: "legacy-large".into(),
            generation: records::new_generation(),
            files: BTreeMap::new(),
            remaining: records::FEEDBACK_LIMIT,
            requires_native_lease: true,
        };
        let old = Stamp {
            length: u64::MAX,
            modified_ns: Some(u64::MAX),
            identity: Some((u64::MAX, u64::MAX)),
        };
        let smaller = Stamp {
            length: 0,
            modified_ns: Some(0),
            identity: Some((0, 0)),
        };
        let header = serde_json::to_vec(&checkpoint).unwrap().len();
        let mut bytes = header;
        for index in 0..records::MAX_FILES {
            let path = format!("z-{index:04}{}", "p".repeat(2300));
            let length = records::entry_bytes(&path, &old).unwrap() + 1;
            if bytes + length > records::MAX_RECORD_BYTES - 32 {
                break;
            }
            bytes += length;
            checkpoint.files.insert(path, old.clone());
        }
        checkpoint.validate().unwrap();
        let new_path = format!("a-{}", "n".repeat(4094));
        let first_growth = records::Delta {
            generation: checkpoint.generation.clone(),
            updates: vec![Update {
                path: new_path.clone(),
                stamp: Some(smaller.clone()),
            }],
            remaining: checkpoint.remaining,
            dispatch: false,
            renewal: None,
        };
        assert!(
            checkpoint.apply(&first_growth).is_err(),
            "lexical growth first must exceed the real checkpoint bound"
        );
        let previous = checkpoint
            .files
            .iter()
            .map(|(path, stamp)| (PathBuf::from(path), stamp.clone()))
            .collect();
        let mut scan = Scan {
            files: checkpoint
                .files
                .keys()
                .map(|path| (PathBuf::from(path), smaller.clone()))
                .collect(),
            complete: true,
            ..Scan::default()
        };
        scan.files.insert(new_path.clone().into(), smaller.clone());
        retain(&mut scan, &previous);
        assert!(scan.files.contains_key(Path::new(&new_path)));
        let files = scan
            .files
            .iter()
            .map(|(path, stamp)| (name(path).unwrap(), stamp.clone()))
            .collect();
        let mut state = State::baseline(previous);
        let Reconciled::Changes { mut changes, .. } = state.reconcile_bounded(
            scan,
            &BTreeSet::new(),
            records::MAX_FILES,
            2 * records::MAX_FILES,
        ) else {
            panic!("bounded final state must reconcile")
        };
        order(&mut changes, &files, &checkpoint.files);
        let mut replay = records::Replay::new(checkpoint.clone()).unwrap();
        for change in &changes {
            replay
                .push(&records::Delta {
                    generation: checkpoint.generation.clone(),
                    updates: updates(std::slice::from_ref(change), &files),
                    remaining: checkpoint.remaining,
                    dispatch: false,
                    renewal: None,
                })
                .unwrap();
        }
        assert_eq!(replay.finish().unwrap().files, files);
    }
    #[test]
    fn existing_shrink_frees_space_for_a_lexically_earlier_growing_update() {
        let previous = BTreeMap::from([
            (PathBuf::from("a"), stamp(0)),
            (PathBuf::from("z"), stamp(u64::MAX)),
        ]);
        let limit = 2 + previous
            .iter()
            .map(|(path, stamp)| cost(path, stamp).unwrap())
            .sum::<usize>();
        let mut scan = Scan {
            files: BTreeMap::from([("a".into(), stamp(u64::MAX)), ("z".into(), stamp(0))]),
            complete: true,
            ..Scan::default()
        };
        constrain(&mut scan, &previous, limit, 2);
        assert!(scan.suppressed.is_empty());
        let mut state = State::baseline(previous);
        let Reconciled::Changes { changes, complete } =
            state.reconcile_bounded(scan, &BTreeSet::new(), 2, 4)
        else {
            panic!("existing updates must fit after shrink")
        };
        assert!(complete);
        assert_eq!(changes.len(), 2);
        assert_eq!(state.files()[Path::new("a")], stamp(u64::MAX));
        assert_eq!(state.files()[Path::new("z")], stamp(0));
    }
}
