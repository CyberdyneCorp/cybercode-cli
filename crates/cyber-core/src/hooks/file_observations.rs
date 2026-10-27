//! Bounded file-observation records. These describe observed metadata, never
//! permissions, file contents, or an authenticated writer. Hook effects are owned
//! separately by the hook receipt machinery and are not replayed with this state.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CHECKPOINT: &str = "file.watch.checkpoint.1";
pub const CHANGED: &str = "file.watch.changed.1";
pub const MAX_FILES: usize = 10_000;
pub const MAX_BATCH: usize = 256;
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REPLAY: usize = 128;
pub const FEEDBACK_LIMIT: u32 = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub length: u64,
    pub modified_ns: Option<u64>,
    pub identity: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub directory: String,
    pub root_identity: String,
    pub generation: String,
    pub files: BTreeMap<String, Stamp>,
    pub remaining: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub path: String,
    /// None means a previously recorded file is now absent in complete evidence.
    pub stamp: Option<Stamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Renewal {
    pub session_id: String,
    pub event_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    pub generation: String,
    pub updates: Vec<Update>,
    pub remaining: u32,
    /// Eligibility is committed before effects. Recovery never dispatches it again.
    pub dispatch: bool,
    pub renewal: Option<Renewal>,
}

pub fn relative_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 4096
        && !name.contains(['\\', ':', '\0'])
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn generation(value: &str) -> Result<(), String> {
    value
        .parse::<ulid::Ulid>()
        .map(|_| ())
        .map_err(|_| "invalid file watcher generation".into())
}

fn bounded<T: Serialize>(value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("file observation record exceeds byte bound".into());
    }
    Ok(())
}

impl Checkpoint {
    pub fn validate(&self) -> Result<(), String> {
        generation(&self.generation)?;
        if !Path::new(&self.directory).is_absolute()
            || self.directory.len() > 4096
            || self.directory.contains('\0')
            || self.root_identity.is_empty()
            || self.root_identity.len() > 256
            || self.remaining > FEEDBACK_LIMIT
            || self.files.len() > MAX_FILES
            || self.files.keys().any(|path| !relative_name(path))
        {
            return Err("invalid file watcher checkpoint scope or bound".into());
        }
        bounded(self)
    }

    /// Used by the single writer when checkpointing or claiming a fresh generation.
    /// A restart is not a user acknowledgement and cannot renew the allowance.
    pub fn follows(&self, previous: &Self) -> Result<(), String> {
        self.validate()?;
        if self.directory != previous.directory
            || self.remaining != previous.remaining
            || (self.generation == previous.generation
                && self.root_identity != previous.root_identity)
            || (self.root_identity == previous.root_identity && self.files != previous.files)
        {
            return Err(
                "file watcher checkpoint changes scope or renews feedback without authority".into(),
            );
        }
        Ok(())
    }

    fn check_delta(&self, delta: &Delta) -> Result<(), String> {
        delta.validate()?;
        if delta.generation != self.generation {
            return Err("stale file watcher generation".into());
        }
        let expected = match &delta.renewal {
            Some(_) => FEEDBACK_LIMIT,
            None if delta.dispatch => self
                .remaining
                .checked_sub(delta.updates.len() as u32)
                .ok_or("file watcher feedback allowance exhausted")?,
            None => self.remaining,
        };
        if delta.remaining != expected {
            return Err("invalid file watcher feedback accounting".into());
        }
        Ok(())
    }

    /// Prepare a candidate state without changing the acknowledged state. The
    /// owner installs the candidate only after the corresponding append succeeds.
    pub fn apply(&self, delta: &Delta) -> Result<Self, String> {
        self.check_delta(delta)?;
        let mut next = self.clone();
        for update in &delta.updates {
            match &update.stamp {
                Some(stamp) => {
                    next.files.insert(update.path.clone(), stamp.clone());
                }
                None => {
                    if next.files.remove(&update.path).is_none() {
                        return Err("file deletion lacks a recorded preimage".into());
                    }
                }
            }
        }
        next.remaining = delta.remaining;
        next.validate()?;
        Ok(next)
    }
}

/// A private replay candidate: failed folds poison it, and no partially rebuilt
/// state can escape. Entry byte accounting enforces the same JSON bound at each
/// step without cloning/serializing the entire file map for every old delta.
pub struct Replay {
    state: Option<Checkpoint>,
    entries_bytes: usize,
}

fn entry_bytes(path: &str, stamp: &Stamp) -> Result<usize, String> {
    // A two-element tuple encodes as [key,value]. Removing its brackets leaves
    // exactly the byte length of the map's key:value entry (comma vs colon).
    serde_json::to_vec(&(path, stamp))
        .map(|bytes| bytes.len() - 2)
        .map_err(|error| error.to_string())
}

impl Replay {
    pub fn new(state: Checkpoint) -> Result<Self, String> {
        state.validate()?;
        let entries_bytes = state.files.iter().try_fold(0usize, |sum, (path, stamp)| {
            entry_bytes(path, stamp).map(|bytes| sum + bytes)
        })?;
        Ok(Self {
            state: Some(state),
            entries_bytes,
        })
    }

    pub fn push(&mut self, delta: &Delta) -> Result<(), String> {
        let mut state = self
            .state
            .take()
            .ok_or("file observation replay is poisoned")?;
        state.check_delta(delta)?;
        let mut bytes = self.entries_bytes;
        for update in &delta.updates {
            if let Some(previous) = state.files.get(&update.path) {
                bytes -= entry_bytes(&update.path, previous)?;
            }
            match &update.stamp {
                Some(stamp) => {
                    bytes += entry_bytes(&update.path, stamp)?;
                    state.files.insert(update.path.clone(), stamp.clone());
                }
                None => {
                    if state.files.remove(&update.path).is_none() {
                        return Err("file deletion lacks a recorded preimage".into());
                    }
                }
            }
        }
        state.remaining = delta.remaining;
        let header = Checkpoint {
            files: BTreeMap::new(),
            directory: state.directory.clone(),
            root_identity: state.root_identity.clone(),
            generation: state.generation.clone(),
            remaining: state.remaining,
        };
        let header_bytes = serde_json::to_vec(&header)
            .map_err(|error| error.to_string())?
            .len();
        if state.files.len() > MAX_FILES
            || header_bytes + bytes + state.files.len().saturating_sub(1) > MAX_RECORD_BYTES
        {
            return Err("file observation replay exceeds state bound".into());
        }
        self.entries_bytes = bytes;
        self.state = Some(state);
        Ok(())
    }

    pub fn finish(mut self) -> Result<Checkpoint, String> {
        let state = self
            .state
            .take()
            .ok_or("file observation replay is poisoned")?;
        state.validate()?;
        Ok(state)
    }
}

impl Delta {
    pub fn validate(&self) -> Result<(), String> {
        generation(&self.generation)?;
        if self.updates.len() > MAX_BATCH || self.remaining > FEEDBACK_LIMIT {
            return Err("file observation delta exceeds bound".into());
        }
        let mut paths = BTreeSet::new();
        for update in &self.updates {
            if !relative_name(&update.path) || !paths.insert(&update.path) {
                return Err("invalid or duplicate file observation path".into());
            }
        }
        if let Some(renewal) = &self.renewal {
            if self.dispatch
                || !self.updates.is_empty()
                || !crate::ids::has_prefix(&renewal.session_id, "ses")
                || !crate::ids::has_prefix(&renewal.event_id, "evt")
            {
                return Err("invalid file watcher user acknowledgement".into());
            }
        } else if self.updates.is_empty() {
            return Err("file observation delta is empty".into());
        }
        bounded(self)
    }
}

pub fn validate_record(kind: &str, value: &serde_json::Value) -> Result<(), String> {
    match kind {
        CHECKPOINT => serde_json::from_value::<Checkpoint>(value.clone())
            .map_err(|error| error.to_string())?
            .validate(),
        CHANGED => serde_json::from_value::<Delta>(value.clone())
            .map_err(|error| error.to_string())?
            .validate(),
        _ => Err("unknown file observation record".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint() -> Checkpoint {
        Checkpoint {
            directory: std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            root_identity: "fixture-root".into(),
            generation: ulid::Ulid::new().to_string(),
            files: BTreeMap::new(),
            remaining: FEEDBACK_LIMIT,
        }
    }
    fn delta(state: &Checkpoint) -> Delta {
        Delta {
            generation: state.generation.clone(),
            updates: vec![Update {
                path: "nested/data".into(),
                stamp: Some(Stamp {
                    length: 8,
                    modified_ns: Some(1),
                    identity: Some((1, 2)),
                }),
            }],
            remaining: FEEDBACK_LIMIT - 1,
            dispatch: true,
            renewal: None,
        }
    }
    #[test]
    fn failed_candidate_does_not_advance_metadata_or_allowance() {
        let state = checkpoint();
        let next = state.apply(&delta(&state)).unwrap();
        assert_eq!(state.remaining, FEEDBACK_LIMIT);
        assert!(state.files.is_empty());
        assert_eq!(next.remaining, FEEDBACK_LIMIT - 1);
        assert!(next.files.contains_key("nested/data"));
    }
    #[test]
    fn restarted_checkpoint_cannot_reset_credit_or_admit_old_worker() {
        let old = checkpoint();
        let mut next = old.apply(&delta(&old)).unwrap();
        next.generation = ulid::Ulid::new().to_string();
        assert!(next.follows(&old.apply(&delta(&old)).unwrap()).is_ok());
        assert!(next.apply(&delta(&old)).unwrap_err().contains("stale"));
        next.remaining = FEEDBACK_LIMIT;
        assert!(next.follows(&old.apply(&delta(&old)).unwrap()).is_err());
    }
    #[test]
    fn accounting_and_ghost_deletions_are_refused() {
        let state = checkpoint();
        let mut change = delta(&state);
        change.remaining += 1;
        assert!(state.apply(&change).is_err());
        change.remaining -= 1;
        change.updates[0].stamp = None;
        assert!(state.apply(&change).unwrap_err().contains("preimage"));
    }
    #[test]
    fn exhausted_observation_is_recordable_without_dispatch_or_automatic_reset() {
        let mut state = checkpoint();
        state.remaining = 0;
        let mut change = delta(&state);
        change.remaining = 0;
        assert!(state.apply(&change).is_err());
        change.dispatch = false;
        let next = state.apply(&change).unwrap();
        assert_eq!(next.remaining, 0);
        assert!(next.files.contains_key("nested/data"));
    }
    #[test]
    fn traversal_ads_duplicates_and_empty_components_are_refused() {
        let state = checkpoint();
        for path in [
            "../private",
            "/private",
            "a/./b",
            "a//b",
            "a\\b",
            "a:stream",
            "",
        ] {
            let mut change = delta(&state);
            change.updates[0].path = path.into();
            assert!(state.apply(&change).is_err(), "{path}");
        }
        let mut change = delta(&state);
        change.updates.push(change.updates[0].clone());
        assert!(state.apply(&change).is_err());
    }
    #[test]
    fn renewal_is_separate_from_observations_and_requires_writer_verification() {
        let state = checkpoint();
        let mut change = delta(&state);
        change.renewal = Some(Renewal {
            session_id: crate::ids::new_id("ses"),
            event_id: crate::ids::new_id("evt"),
        });
        assert!(change.validate().is_err());
        change.updates.clear();
        change.dispatch = false;
        change.remaining = FEEDBACK_LIMIT;
        assert_eq!(state.apply(&change).unwrap().remaining, FEEDBACK_LIMIT);
    }
    #[test]
    fn replay_matches_acknowledged_folds_for_escaped_unicode_names() {
        let state = checkpoint();
        let mut change = delta(&state);
        change.updates[0].path = "nested/quote\"-🔧".into();
        let expected = state.apply(&change).unwrap();
        let mut replay = Replay::new(state).unwrap();
        replay.push(&change).unwrap();
        assert_eq!(replay.finish().unwrap(), expected);
    }

    #[test]
    fn replay_failure_poison_cannot_publish_a_partial_state() {
        let state = checkpoint();
        let valid = delta(&state);
        let mut invalid = valid.clone();
        invalid.generation = ulid::Ulid::new().to_string();
        let mut replay = Replay::new(state).unwrap();
        assert!(replay.push(&invalid).is_err());
        assert!(replay.push(&valid).is_err());
        assert!(replay.finish().is_err());
    }

    #[test]
    fn incremental_replay_enforces_the_full_serialized_byte_bound() {
        let mut state = checkpoint();
        let stamp = Stamp {
            length: 1,
            modified_ns: Some(1),
            identity: None,
        };
        let path = |index| format!("{}{:04}", "dir/".repeat(1000), index);
        let unit = entry_bytes(&path(0), &stamp).unwrap() + 1;
        let header = serde_json::to_vec(&state).unwrap().len();
        let count = (MAX_RECORD_BYTES - header) / unit - 1;
        for index in 0..count {
            state.files.insert(path(index), stamp.clone());
        }
        state.validate().unwrap();
        let change = Delta {
            generation: state.generation.clone(),
            updates: (count..count + 3)
                .map(|index| Update {
                    path: path(index),
                    stamp: Some(stamp.clone()),
                })
                .collect(),
            remaining: FEEDBACK_LIMIT,
            dispatch: false,
            renewal: None,
        };
        assert!(state.apply(&change).is_err());
        let mut replay = Replay::new(state).unwrap();
        assert!(replay.push(&change).is_err());
        assert!(replay.finish().is_err());
    }
}
