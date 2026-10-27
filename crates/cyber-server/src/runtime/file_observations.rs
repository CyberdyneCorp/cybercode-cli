//! Durable, generation-fenced file observation ownership. Native watching and
//! path policy stay in the host adapter; recording cannot authorize a hook.
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Runtime, RuntimeError, WeakRuntime};
use cyber_core::hooks::file_observations::{
    self as records, Checkpoint, Delta, Renewal, Replay, Stamp,
};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

pub struct FileWatch {
    runtime: WeakRuntime,
    store: Arc<Store>,
    aggregate: String,
    sequence: i64,
    since_checkpoint: usize,
    state: Checkpoint,
}

struct Recorded {
    state: Checkpoint,
    sequence: i64,
}

fn aggregate(directory: &str) -> String {
    format!("file-watch:{:x}", Sha256::digest(directory.as_bytes()))
}
fn corrupt(reason: impl Into<String>) -> StoreError {
    StoreError::Unavailable(reason.into())
}
fn decode<T: serde::de::DeserializeOwned>(data: &str) -> Result<T, StoreError> {
    serde_json::from_str(data)
        .map_err(|error| corrupt(format!("invalid file observation history: {error}")))
}

/// Replay only metadata state after the most recent checkpoint. This does not
/// launch handlers or return pending effects to replay after a lost acknowledgement.
fn load(db: &Connection, id: &str, before: i64) -> Result<Option<Recorded>, StoreError> {
    let checkpoint: Option<(i64, String)> = db.query_row(
        "SELECT seq,data FROM event WHERE aggregate_id=?1 AND type=?2 AND seq<?3 ORDER BY seq DESC LIMIT 1",
        params![id,records::CHECKPOINT,before], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional()?;
    let Some((mut sequence, data)) = checkpoint else {
        return Ok(None);
    };
    let state: Checkpoint = decode(&data)?;
    state.validate().map_err(corrupt)?;
    if aggregate(&state.directory) != id {
        return Err(corrupt("file observation aggregate differs from Location"));
    }
    let mut query = db.prepare("SELECT seq,type,data FROM event WHERE aggregate_id=?1 AND seq>?2 AND seq<?3 ORDER BY seq LIMIT ?4")?;
    let rows = query.query_map(
        params![id, sequence, before, (records::MAX_REPLAY + 1) as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut replay = Replay::new(state).map_err(corrupt)?;
    for (index, row) in rows.enumerate() {
        let (seq, kind, data) = row?;
        if index >= records::MAX_REPLAY || seq != sequence + 1 || kind != records::CHANGED {
            return Err(corrupt(
                "file observation replay bound or sequence exceeded",
            ));
        }
        replay.push(&decode::<Delta>(&data)?).map_err(corrupt)?;
        sequence = seq;
    }
    let state = replay.finish().map_err(corrupt)?;
    Ok(Some(Recorded { state, sequence }))
}

impl Runtime {
    /// The host supplies metadata from its already pinned, policy-filtered root.
    /// Claiming a new generation fences any previous writer through exact-seq CAS.
    pub async fn claim_file_watch(
        &self,
        directory: String,
        root_identity: String,
        baseline: BTreeMap<String, Stamp>,
    ) -> Result<FileWatch, RuntimeError> {
        let _open = self.inner.open().await?;
        let id = aggregate(&directory);
        let lookup = id.clone();
        let previous = self
            .inner
            .store
            .read(move |db| load(db, &lookup, i64::MAX))?;
        let sequence = previous.as_ref().map_or(-1, |row| row.sequence);
        let files = previous
            .as_ref()
            .filter(|row| row.state.root_identity == root_identity)
            .map_or(baseline, |row| row.state.files.clone());
        let state = Checkpoint {
            directory,
            root_identity,
            generation: ulid::Ulid::new().to_string(),
            files,
            remaining: previous
                .as_ref()
                .map_or(records::FEEDBACK_LIMIT, |row| row.state.remaining),
        };
        state.validate().map_err(RuntimeError::Invalid)?;
        let stored = self.inner.store.append(
            &id,
            Expected::Seq(sequence),
            vec![NewEvent::new(
                records::CHECKPOINT,
                serde_json::to_value(&state)
                    .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
            )],
        )?;
        Ok(FileWatch {
            runtime: self.downgrade(),
            store: Arc::clone(&self.inner.store),
            aggregate: id,
            sequence: stored[0].seq,
            since_checkpoint: 0,
            state,
        })
    }
}

impl FileWatch {
    pub fn state(&self) -> &Checkpoint {
        &self.state
    }

    fn runtime(&self) -> Result<Runtime, RuntimeError> {
        self.runtime.upgrade().ok_or(RuntimeError::ShuttingDown)
    }

    /// A read-side freshness check is advisory; commit's exact-seq append is the
    /// authoritative writer fence. Consumers still verify scope before effects.
    pub fn current(&self) -> Result<bool, RuntimeError> {
        Ok(self.store.aggregate_seq(&self.aggregate)? == Some(self.sequence))
    }

    pub async fn record(&mut self, delta: Delta) -> Result<StoredEvent, RuntimeError> {
        let runtime = self.runtime()?;
        let _open = runtime.inner.open().await?;
        let next = self.state.apply(&delta).map_err(RuntimeError::Invalid)?;
        if self.since_checkpoint >= records::MAX_REPLAY {
            self.checkpoint()?;
        }
        let stored = self.store.append(
            &self.aggregate,
            Expected::Seq(self.sequence),
            vec![NewEvent::new(
                records::CHANGED,
                serde_json::to_value(&delta)
                    .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
            )],
        )?;
        self.state = next;
        self.sequence = stored[0].seq;
        self.since_checkpoint += 1;
        Ok(stored.into_iter().next().expect("one observation appended"))
    }

    fn checkpoint(&mut self) -> Result<(), RuntimeError> {
        let stored = self.store.append(
            &self.aggregate,
            Expected::Seq(self.sequence),
            vec![NewEvent::new(
                records::CHECKPOINT,
                serde_json::to_value(&self.state)
                    .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
            )],
        )?;
        self.sequence = stored[0].seq;
        self.since_checkpoint = 0;
        Ok(())
    }
}

pub(super) fn register(registry: &mut EventRegistry) {
    for kind in [records::CHECKPOINT, records::CHANGED] {
        registry
            .register_with(kind, move |value| records::validate_record(kind, value))
            .expect("valid file observation event");
    }
}

/// Validate authority and invariants under the same single-writer transaction.
/// A source labelled `hook`/`plugin` cannot renew a user allowance; a committed
/// user acknowledgement is consumable only once, including across restart.
fn verify_renewal(
    tx: &Transaction<'_>,
    event: &StoredEvent,
    state: &Checkpoint,
    renewal: &Renewal,
) -> Result<(), String> {
    let valid: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM event e JOIN session s ON s.id=e.aggregate_id WHERE e.id=?1 AND e.aggregate_id=?2 AND e.type='session.prompt.admitted.1' AND json_extract(e.data,'$.source')='user' AND s.directory=?3 AND s.archived=0
         AND (SELECT COALESCE(json_extract(location.data,'$.to.path'),json_extract(location.data,'$.info.directory'))
              FROM event location WHERE location.aggregate_id=e.aggregate_id AND location.seq<=e.seq
              AND location.type IN ('session.created.1','session.worktree.entered.1','session.worktree.exited.1','session.worktree.rebound.1')
              ORDER BY location.seq DESC LIMIT 1)=?3)",
        params![renewal.event_id,renewal.session_id,state.directory], |row|row.get(0),
    ).map_err(|error|error.to_string())?;
    let used: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND seq<?2 AND type=?3 AND json_extract(data,'$.renewal.event_id')=?4)",
        params![event.aggregate_id,event.seq,records::CHANGED,renewal.event_id],|row|row.get(0),
    ).map_err(|error|error.to_string())?;
    if !valid || used {
        return Err("file watcher renewal lacks fresh owned user input".into());
    }
    Ok(())
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if ![records::CHECKPOINT, records::CHANGED].contains(&event.kind.as_str()) {
        return Ok(());
    }
    let previous = load(tx, &event.aggregate_id, event.seq).map_err(|error| error.to_string())?;
    match event.kind.as_str() {
        records::CHECKPOINT => {
            let state: Checkpoint =
                serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
            if aggregate(&state.directory) != event.aggregate_id {
                return Err("file watcher aggregate differs from Location".into());
            }
            match previous {
                Some(previous) => state.follows(&previous.state)?,
                None if event.seq == 0 => {}
                None => return Err("file watcher checkpoint lacks prior state".into()),
            }
        }
        records::CHANGED => {
            let previous = previous.ok_or("file observation lacks checkpoint")?;
            let delta: Delta =
                serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
            if let Some(renewal) = &delta.renewal {
                verify_renewal(tx, event, &previous.state, renewal)?;
            }
            previous.state.apply(&delta)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}
