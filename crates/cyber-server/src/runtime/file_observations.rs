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
    pinned_root: Option<Arc<super::file_observation_roots::PinnedRoot>>,
}

impl Drop for FileWatch {
    fn drop(&mut self) {
        if self.pinned_root.is_some()
            && let Some(runtime) = self.runtime.upgrade()
        {
            runtime
                .inner
                .file_observation_roots
                .remove(&self.state.generation);
        }
    }
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
    pub(super) async fn capture_observation_input(
        &self,
        handle: &super::Handle,
        user: bool,
    ) -> Option<super::file_observation_roots::CapturedInput> {
        if !user {
            return None;
        }
        let directory = handle.state.lock().await.info.directory.clone();
        tokio::task::spawn_blocking(move || {
            super::file_observation_roots::CapturedInput::new(&directory).ok()
        })
        .await
        .ok()
        .flatten()
    }
    /// Recheck every new real handler against the retained native generation.
    /// Only metadata is read under the database connection; OS identity probes
    /// happen after that read and after the native registry lock is released.
    pub(super) fn validate_native_file_hook(
        &self,
        event: &cyber_core::hooks::HookEvent,
    ) -> Result<Option<super::file_observation_roots::Lease>, RuntimeError> {
        if event.is_synthetic() || event.event() != "FileChanged" {
            return Ok(None);
        }
        let field = |name: &str| {
            event.as_json()[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| RuntimeError::Invalid(format!("FileChanged lacks {name}")))
        };
        let observation = field("observation_id")?;
        let generation = field("watch_generation")?;
        let raw = &event.identity().location.directory;
        let lease = self.native_file_scope(&observation, &generation, raw)?;
        let expected = lease
            .as_ref()
            .map_or(raw.as_path(), |lease| lease.root.path.as_path());
        if event.execution_directory() != expected {
            return Err(RuntimeError::Invalid(
                "file observation execution directory differs from its native owner".into(),
            ));
        }
        Ok(lease)
    }

    /// The host uses this checked location for configuration and native command
    /// cwd while preserving the raw Session envelope. It is not effect authority;
    /// each actual handler must obtain and retain its own admission lease.
    pub fn file_observation_directory(
        &self,
        observation: &str,
        generation: &str,
        directory: &std::path::Path,
    ) -> Result<Option<std::path::PathBuf>, RuntimeError> {
        Ok(self
            .native_file_scope(observation, generation, directory)?
            .map(|lease| lease.root.path.clone()))
    }

    fn native_file_scope(
        &self,
        observation: &str,
        generation: &str,
        raw: &std::path::Path,
    ) -> Result<Option<super::file_observation_roots::Lease>, RuntimeError> {
        let observation = observation.to_owned();
        let generation = generation.to_owned();
        let expected_generation = generation.clone();
        let owner: Option<(String, String, String, bool)> = self.inner.store.read(move |db| {
            Ok(db.query_row(
                "SELECT json_extract(c.data,'$.directory'),json_extract(c.data,'$.root_identity'),json_extract(c.data,'$.generation'),COALESCE(json_extract(c.data,'$.requires_native_lease'),0)
                 FROM event source JOIN event c ON c.aggregate_id=source.aggregate_id
                 WHERE source.id=?1 AND source.type=?2 AND json_extract(source.data,'$.generation')=?3 AND c.type=?4
                 ORDER BY c.seq DESC LIMIT 1",
                params![observation,records::CHANGED,expected_generation,records::CHECKPOINT],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).optional()?)
        })?;
        let (directory, identity, current_generation, requires_native_lease) =
            owner.ok_or_else(|| {
                RuntimeError::Invalid("file observation source is unavailable".into())
            })?;
        if current_generation != generation {
            return Err(RuntimeError::Invalid(
                "file observation native generation is stale".into(),
            ));
        }
        if let Some(root) = self.inner.file_observation_roots.get(&generation) {
            if root.path != std::path::Path::new(&directory)
                || root.identity != identity
                || !root.matches(raw)
            {
                return Err(RuntimeError::Invalid(
                    "file observation no longer belongs to the pinned native root".into(),
                ));
            }
            return Ok(Some(super::file_observation_roots::Lease {
                generation,
                root,
            }));
        } else if requires_native_lease || raw != std::path::Path::new(&directory) {
            return Err(RuntimeError::Invalid(
                "file observation lacks its required live native lease".into(),
            ));
        }
        Ok(None)
    }
    /// Claim from an owned, already opened native directory. Its canonical path
    /// is the journal key; the descriptor supplies physical identity and remains
    /// alive until this FileWatch and its in-flight callbacks have settled.
    pub async fn claim_pinned_file_watch(
        &self,
        directory: std::path::PathBuf,
        file: std::fs::File,
        baseline: BTreeMap<String, Stamp>,
    ) -> Result<FileWatch, RuntimeError> {
        let root = Arc::new(
            super::file_observation_roots::PinnedRoot::new(&directory, file)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        );
        let mut watch = self
            .claim_file_watch_mode(
                root.path.display().to_string(),
                root.identity.clone(),
                baseline,
                true,
            )
            .await?;
        self.inner
            .file_observation_roots
            .register(watch.state.generation.clone(), &root)
            .map_err(RuntimeError::Invalid)?;
        watch.pinned_root = Some(root);
        Ok(watch)
    }
    /// The host supplies metadata from its already pinned, policy-filtered root.
    /// Claiming a new generation fences any previous writer through exact-seq CAS.
    pub async fn claim_file_watch(
        &self,
        directory: String,
        root_identity: String,
        baseline: BTreeMap<String, Stamp>,
    ) -> Result<FileWatch, RuntimeError> {
        self.claim_file_watch_mode(directory, root_identity, baseline, false)
            .await
    }

    async fn claim_file_watch_mode(
        &self,
        directory: String,
        root_identity: String,
        baseline: BTreeMap<String, Stamp>,
        requires_native_lease: bool,
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
        let replaced = previous
            .as_ref()
            .is_none_or(|row| row.state.root_identity != root_identity);
        let state = Checkpoint {
            directory,
            root_identity,
            generation: records::new_generation(),
            files,
            remaining: previous
                .as_ref()
                .map_or(records::FEEDBACK_LIMIT, |row| row.state.remaining),
            requires_native_lease,
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
        self.inner
            .file_observation_roots
            .revoke(std::path::Path::new(&state.directory));
        if replaced {
            self.invalidate_observed_location(&state.directory).await;
        }
        Ok(FileWatch {
            runtime: self.downgrade(),
            store: Arc::clone(&self.inner.store),
            aggregate: id,
            sequence: stored[0].seq,
            since_checkpoint: 0,
            state,
            pinned_root: None,
        })
    }

    async fn invalidate_observed_location(&self, directory: &str) {
        let handles: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for handle in handles {
            if handle.state.lock().await.info.directory != directory {
                continue;
            }
            handle
                .envelope_epoch
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            *handle
                .envelope
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }
}

impl FileWatch {
    pub(super) fn validate_native_scope(&self, directory: &str) -> Result<(), RuntimeError> {
        if let Some(root) = &self.pinned_root {
            if !root.matches(std::path::Path::new(directory)) {
                return Err(RuntimeError::Invalid(
                    "observing Session no longer names the pinned root".into(),
                ));
            }
        } else if self.state.requires_native_lease || directory != self.state.directory {
            return Err(RuntimeError::Invalid(
                "alias admission requires live pinned native proof".into(),
            ));
        }
        Ok(())
    }
    pub fn state(&self) -> &Checkpoint {
        &self.state
    }

    /// A scope record is provenance; native alias admission additionally needs
    /// a live pinned root. This port currently retains exact-directory scope.
    pub async fn admit_session_scope(&self, session_id: &str) -> Result<(), RuntimeError> {
        super::file_observation_scopes::admit(self, self.runtime()?, &self.store, session_id).await
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
        // Prepare native provenance before taking the lifecycle admission read
        // lease: nested acquisition could deadlock behind a shutdown writer.
        if let Some(renewal) = &delta.renewal {
            self.admit_session_scope(&renewal.session_id).await?;
        }
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
    for kind in [records::CHECKPOINT, records::CHANGED, records::SCOPE] {
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
    let location: Option<(String, Option<String>, Option<String>)> = tx.query_row(
        "SELECT s.directory, (SELECT COALESCE(json_extract(location.data,'$.to.path'),json_extract(location.data,'$.info.directory'))
              FROM event location WHERE location.aggregate_id=e.aggregate_id AND location.seq<=e.seq
              AND location.type IN ('session.created.1','session.worktree.entered.1','session.worktree.exited.1','session.worktree.rebound.1')
              ORDER BY location.seq DESC LIMIT 1), json_extract(e.data,'$.observation_scope')
         FROM event e JOIN session s ON s.id=e.aggregate_id WHERE e.id=?1 AND e.aggregate_id=?2 AND e.type='session.prompt.admitted.1' AND json_extract(e.data,'$.source')='user' AND s.archived=0",
        params![renewal.event_id,renewal.session_id], |row|Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|error|error.to_string())?;
    let valid = match location {
        Some((directory, Some(history), snapshot)) if history == directory => {
            let physical = if state.requires_native_lease {
                snapshot
                    .and_then(|snapshot| {
                        serde_json::from_str::<records::InputScope>(&snapshot).ok()
                    })
                    .is_some_and(|scope| {
                        scope.session_directory == directory
                            && scope.directory == state.directory
                            && scope.root_identity == state.root_identity
                    })
            } else {
                true
            };
            physical
                && super::file_observation_scopes::matches(
                    tx,
                    state,
                    &renewal.session_id,
                    &directory,
                )?
        }
        _ => false,
    };
    let used: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM event WHERE id!=?1 AND type='file.watch.changed.1' AND json_type(data,'$.renewal.event_id')='text' AND json_extract(data,'$.renewal.event_id')=?2)",
        params![event.id,renewal.event_id],|row|row.get(0),
    ).map_err(|error|error.to_string())?;
    if !valid || used {
        return Err("file watcher renewal lacks fresh owned user input".into());
    }
    Ok(())
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if event.kind == super::events::ADMITTED {
        return super::file_observation_scopes::project_input(tx, event);
    }
    if event.kind == records::SCOPE {
        return super::file_observation_scopes::project(tx, event);
    }
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

pub(super) fn verify_scope_generation(
    db: &Connection,
    scope: &records::SessionScope,
) -> Result<(), String> {
    let current = load(db, &aggregate(&scope.directory), i64::MAX)
        .map_err(|error| error.to_string())?
        .ok_or("file observation scope lacks its watch owner")?;
    if current.state.directory != scope.directory
        || current.state.root_identity != scope.root_identity
        || current.state.generation != scope.generation
    {
        return Err("file observation scope belongs to a stale or different root".into());
    }
    Ok(())
}

/// Writer-side freshness, eligibility and path proof for real hook admission.
pub(super) fn verify_hook_source(
    db: &Connection,
    directory: &str,
    session_id: &str,
    observation_id: &str,
    generation: &str,
    file_path: &str,
) -> Result<(), String> {
    let source: Option<(String, String, String)> = db
        .query_row(
            "SELECT aggregate_id,type,data FROM event WHERE id=?1",
            [observation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let (id, kind, data) = source.ok_or("file observation does not exist")?;
    if kind != records::CHANGED {
        return Err("hook source is not a file observation".into());
    }
    let delta: Delta = decode(&data).map_err(|error| error.to_string())?;
    delta.validate()?;
    let current = load(db, &id, i64::MAX)
        .map_err(|error| error.to_string())?
        .ok_or("file watch is unavailable")?;
    if !super::file_observation_scopes::matches(db, &current.state, session_id, directory)?
        || current.state.generation != generation
        || delta.generation != generation
        || !delta.dispatch
        || delta.renewal.is_some()
        || !delta.updates.iter().any(|update| update.path == file_path)
    {
        return Err("file observation scope, generation or delivery eligibility changed".into());
    }
    Ok(())
}

impl FileWatch {
    /// Dispatch a committed observation to one real Session, outside lifecycle or
    /// Session locks. Hook decisions do not alter the observed filesystem result.
    /// Receipt admission rechecks provenance atomically; duplicate/unknown receipts
    /// stay consumed, including after restart.
    pub async fn dispatch_to(
        &self,
        observation: &StoredEvent,
        session_id: &str,
    ) -> Result<(), RuntimeError> {
        self.dispatch_to_cancellable(
            observation,
            session_id,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// Stop starting new handlers on owner cancellation; settle every launched
    /// handler through the existing cooperative receipt machinery.
    pub async fn dispatch_to_cancellable(
        &self,
        observation: &StoredEvent,
        session_id: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), RuntimeError> {
        let runtime = self.runtime()?;
        if observation.aggregate_id != self.aggregate || observation.kind != records::CHANGED {
            return Err(RuntimeError::Invalid(
                "observation belongs to another owner".into(),
            ));
        }
        let delta: Delta = serde_json::from_value(observation.data.clone())
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        delta.validate().map_err(RuntimeError::Invalid)?;
        if !delta.dispatch || delta.generation != self.state.generation || delta.renewal.is_some() {
            return Err(RuntimeError::Invalid(
                "observation is not eligible for dispatch".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        self.admit_session_scope(session_id).await?;
        let handle = runtime.inner.handle(session_id).await?;
        let (directory, hooks) = {
            let state = handle.state.lock().await;
            if state.info.archived {
                return Err(RuntimeError::Invalid(
                    "observing Session changed Location or is archived".into(),
                ));
            }
            (state.info.directory.clone(), delta.updates.iter().map(|update|super::lifecycle::lifecycle(&state,"FileChanged",update.path.clone(),
                serde_json::json!({"file_path":update.path,"change":if update.stamp.is_some(){"changed"}else{"removed"},
                    "observation_id":observation.id,"watch_generation":delta.generation}))).collect::<Vec<_>>())
        };
        self.validate_native_scope(&directory)?;
        let mut failure = None;
        for hook in hooks {
            if cancel.is_cancelled() {
                break;
            }
            if let Err(error) = runtime
                .inner
                .owned_lifecycle_hooks_cancelled(hook, Some(cancel.clone()))
                .await
            {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(RuntimeError::Hook(error)))
    }
}
