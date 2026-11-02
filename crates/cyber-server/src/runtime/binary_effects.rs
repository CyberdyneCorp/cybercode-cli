//! Durable binary-save handoff. Native observations remain owned by FileWatch;
//! these records neither mint UserInput nor replay hook callbacks after restart.
use super::{Runtime, RuntimeError};
use cyber_core::{
    binary::receipts::Version,
    hooks::file_observations::{self as observations, Stamp},
};
use cyber_store::{EventRegistry, Expected, NewEvent, StoredEvent};
use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

const CHANGED: &str = "binary.source.handoff.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub version: Version,
    pub session_id: String,
    pub directory: String,
    pub resource: String,
    pub state: State,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum State {
    Queued,
    Claimed {
        aggregate: String,
        generation: String,
        path: String,
        stamp: Stamp,
    },
    Observed {
        observation_id: String,
    },
    Suppressed {
        reason: String,
    },
}
fn aggregate(key: &str) -> String {
    format!("binary-source:{key}")
}
fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}
fn decode(data: &str) -> Result<Handoff, String> {
    serde_json::from_str(data).map_err(|error| error.to_string())
}
fn valid_key(key: &str) -> bool {
    key.len() == 68 && key.starts_with("ver_") && key[4..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && std::path::Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}
fn validate(record: &Handoff) -> Result<(), String> {
    if !valid_key(&record.version.version_id)
        || !cyber_core::ids::has_prefix(&record.session_id, "ses")
        || !valid_source_path(&record.version.path)
        || [&record.directory, &record.resource]
            .into_iter()
            .any(|path| {
                path.len() > 4096
                    || path.contains('\0')
                    || !std::path::Path::new(path).is_absolute()
            })
        || serde_json::to_vec(record).map_err(|e| e.to_string())?.len() > 32 * 1024
    {
        return Err("invalid binary source handoff".into());
    }
    match &record.state {
        State::Claimed {
            aggregate,
            generation,
            path,
            ..
        } if !aggregate.starts_with("file-watch:")
            || observations::validate_generation(generation).is_err()
            || !observations::relative_name(path) =>
        {
            Err("invalid native observation binding".into())
        }
        State::Observed { observation_id }
            if !cyber_core::ids::has_prefix(observation_id, "evt") =>
        {
            Err("invalid observation reference".into())
        }
        State::Suppressed { reason }
            if !matches!(
                reason.as_str(),
                "no_observer"
                    | "source_superseded"
                    | "origin_expired"
                    | "outside_observed_location"
                    | "handoff_interrupted"
                    | "observation_unavailable"
                    | "unobserved_path"
            ) =>
        {
            Err("invalid handoff disposition".into())
        }
        _ => Ok(()),
    }
}
fn same_source(a: &Handoff, b: &Handoff) -> bool {
    a.session_id == b.session_id
        && a.directory == b.directory
        && a.resource == b.resource
        && serde_json::to_value(&a.version).ok() == serde_json::to_value(&b.version).ok()
}
pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CHANGED, |data| {
            let record: Handoff =
                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
            validate(&record)
        })
        .expect("valid binary source handoff event");
}
pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    if event.kind != CHANGED {
        return Ok(());
    }
    let next: Handoff = serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    if event.aggregate_id != aggregate(&next.version.version_id) {
        return Err("binary handoff aggregate mismatch".into());
    }
    let prior: Option<String> = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 1",
            (&event.aggregate_id, event.seq),
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match prior {
        None if event.seq == 0 && matches!(next.state, State::Queued) => Ok(()),
        None => Err("binary handoff lacks queued source evidence".into()),
        Some(prior) => {
            let prior = decode(&prior)?;
            if !same_source(&prior, &next) {
                return Err("binary source evidence changed".into());
            }
            match (&prior.state, &next.state) {
                (State::Queued, State::Claimed { .. }) => verify_claim(tx, &next),
                (State::Queued, State::Suppressed { .. }) => Ok(()),
                (State::Claimed { .. }, State::Suppressed { .. }) => Ok(()),
                (State::Claimed { .. }, State::Observed { observation_id }) => {
                    verify_observation(tx, &prior.state, observation_id)
                }
                _ => Err("binary handoff cannot replay or change its terminal disposition".into()),
            }
        }
    }
}
fn verify_claim(tx: &Transaction<'_>, record: &Handoff) -> Result<(), String> {
    let State::Claimed {
        aggregate,
        generation,
        path,
        stamp,
    } = &record.state
    else {
        return Err("missing native claim".into());
    };
    let data: Option<String> = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1",
            (aggregate, observations::CHECKPOINT),
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let checkpoint: observations::Checkpoint =
        serde_json::from_str(&data.ok_or("native checkpoint is absent")?)
            .map_err(|error| error.to_string())?;
    if !checkpoint.requires_native_lease
        || checkpoint.directory != record.directory
        || checkpoint.generation != *generation
        || stamp.length != record.version.after_length.0
        || stamp.identity.is_none()
        || std::path::Path::new(&checkpoint.directory).join(path)
            != std::path::Path::new(&record.resource)
    {
        return Err("binary source differs from its native observation claim".into());
    }
    Ok(())
}
fn matches_observation(
    kind: &str,
    data: &str,
    generation: &str,
    path: &str,
    stamp: &Stamp,
) -> bool {
    match kind {
        observations::CHANGED => serde_json::from_str::<observations::Delta>(data)
            .ok()
            .is_some_and(|delta| {
                delta.generation == generation
                    && delta
                        .updates
                        .iter()
                        .any(|update| update.path == path && update.stamp.as_ref() == Some(stamp))
            }),
        observations::CHECKPOINT => serde_json::from_str::<observations::Checkpoint>(data)
            .ok()
            .is_some_and(|checkpoint| {
                checkpoint.generation == generation && checkpoint.files.get(path) == Some(stamp)
            }),
        _ => false,
    }
}
fn verify_observation(tx: &Transaction<'_>, claim: &State, id: &str) -> Result<(), String> {
    let State::Claimed {
        aggregate,
        generation,
        path,
        stamp,
    } = claim
    else {
        return Err("missing native claim".into());
    };
    let observation: Option<(String, String, String)> = tx
        .query_row(
            "SELECT aggregate_id,type,data FROM event WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match observation {
        Some((owner, kind, data))
            if owner == *aggregate
                && matches_observation(&kind, &data, generation, path, stamp) =>
        {
            Ok(())
        }
        _ => Err("binary handoff reference does not prove the native source observation".into()),
    }
}
impl Runtime {
    /// Invoke after the actual filesystem worker releases checkout/path owners.
    /// A repeated receipt may reconcile durable evidence, never re-enqueue an
    /// already claimed native observation or renew expired hook provenance.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub async fn publish_binary_change(
        &self,
        change: std::sync::Arc<cyber_core::binary::editor::AcknowledgedChange>,
    ) -> Result<Handoff, RuntimeError> {
        let record = self.queue_binary_change(&change)?;
        if matches!(record.state, State::Queued) {
            self.inner
                .tools
                .binary_file_changed(change.clone())
                .await
                .map_err(invalid)?;
        }
        self.reconcile_binary_handoff(change.key())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn claim_binary_observation(
        &self,
        watch: &super::file_observations::FileWatch,
        change: &cyber_core::binary::editor::AcknowledgedChange,
    ) -> Result<Handoff, RuntimeError> {
        self.queue_binary_change(change)?;
        let (sequence, mut record) = self
            .binary_handoff(change.key())?
            .ok_or_else(|| invalid("Binary handoff is absent"))?;
        if !matches!(record.state, State::Queued) {
            return Ok(record);
        }
        let (aggregate, _, state) = watch.binary_source_binding(self)?;
        if state.directory != record.directory {
            return self.suppress_binary_change(change.key(), "outside_observed_location");
        }
        let relative = std::path::Path::new(&record.resource)
            .strip_prefix(&state.directory)
            .ok()
            .map(cyber_core::vcs::slash_path);
        let Some(path) = relative else {
            return self.suppress_binary_change(change.key(), "outside_observed_location");
        };
        if !observations::relative_name(&path) {
            return self.suppress_binary_change(change.key(), "unobserved_path");
        }
        let stamp = match change.observed_stamp() {
            Ok(stamp) => stamp,
            Err(_) => return self.suppress_binary_change(change.key(), "source_superseded"),
        };
        watch.binary_source_binding(self)?;
        record.state = State::Claimed {
            aggregate: aggregate.into(),
            generation: state.generation.clone(),
            path,
            stamp,
        };
        self.append_binary_handoff(sequence, &record)?;
        Ok(record)
    }
    /// Link existing durable native evidence only. This never records another
    /// file change, renews UserInput or dispatches a hook, including on restart.
    pub fn reconcile_binary_handoff(&self, key: &str) -> Result<Handoff, RuntimeError> {
        let (sequence, mut record) = self
            .binary_handoff(key)?
            .ok_or_else(|| invalid("Binary handoff is absent"))?;
        let State::Claimed {
            aggregate,
            generation,
            path,
            stamp,
        } = &record.state
        else {
            return Ok(record);
        };
        let (aggregate, generation, path, stamp) = (
            aggregate.clone(),
            generation.clone(),
            path.clone(),
            stamp.clone(),
        );
        let retained_generation = generation.clone();
        let observed = self.inner.store.read(move |db| {
            let mut query = db.prepare(
                "SELECT id,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT ?2",
            )?;
            let rows =
                query.query_map((&aggregate, (observations::MAX_REPLAY + 2) as i64), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
            for row in rows {
                let (id, kind, data) = row?;
                if matches_observation(&kind, &data, &generation, &path, &stamp) {
                    return Ok(Some(id));
                }
            }
            Ok(None)
        })?;
        if let Some(observation_id) = observed {
            record.state = State::Observed { observation_id };
            self.append_binary_handoff(sequence, &record)?;
        } else if self
            .inner
            .file_observation_roots
            .get(&retained_generation)
            .is_none_or(|root| !root.matches(std::path::Path::new(&record.directory)))
        {
            return self.suppress_binary_change(key, "handoff_interrupted");
        }
        Ok(record)
    }
    /// Metadata only. HTTP callers must separately enforce owner/read scope.
    pub fn binary_handoff(&self, key: &str) -> Result<Option<(i64, Handoff)>, RuntimeError> {
        if !valid_key(key) {
            return Err(invalid("Invalid binary source key"));
        }
        let id = aggregate(key);
        self.inner
            .store
            .read(move |db| {
                db.query_row(
                    "SELECT seq,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                    [id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(Into::into)
            })?
            .map(|(seq, data)| {
                decode(&data)
                    .map(|record| (seq, record))
                    .map_err(RuntimeError::Corrupt)
            })
            .transpose()
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn queue_binary_change(
        &self,
        change: &cyber_core::binary::editor::AcknowledgedChange,
    ) -> Result<Handoff, RuntimeError> {
        let storage = self
            .binary_documents()
            .storage()
            .map_err(|e| invalid(e.to_string()))?;
        change
            .belongs_to(&storage)
            .map_err(|e| invalid(e.to_string()))?;
        let queued = Handoff {
            version: change.version().clone(),
            session_id: change.owner().session_id.clone(),
            directory: change
                .owner()
                .directory
                .to_str()
                .ok_or_else(|| invalid("Invalid source Location"))?
                .into(),
            resource: change
                .resource()
                .path()
                .to_str()
                .ok_or_else(|| invalid("Invalid source resource"))?
                .into(),
            state: State::Queued,
        };
        if let Some((_, prior)) = self.binary_handoff(change.key())? {
            if !same_source(&prior, &queued) {
                return Err(invalid("Binary source key differs from retained evidence"));
            }
            return Ok(prior);
        }
        self.inner.store.append(
            &aggregate(change.key()),
            Expected::Seq(-1),
            vec![NewEvent::new(
                CHANGED,
                serde_json::to_value(&queued).map_err(|e| invalid(e.to_string()))?,
            )],
        )?;
        Ok(queued)
    }
    pub fn suppress_binary_change(&self, key: &str, reason: &str) -> Result<Handoff, RuntimeError> {
        let (seq, mut record) = self
            .binary_handoff(key)?
            .ok_or_else(|| invalid("Binary handoff is absent"))?;
        if matches!(
            record.state,
            State::Observed { .. } | State::Suppressed { .. }
        ) {
            return Ok(record);
        }
        record.state = State::Suppressed {
            reason: reason.into(),
        };
        self.append_binary_handoff(seq, &record)?;
        Ok(record)
    }
    fn append_binary_handoff(&self, sequence: i64, record: &Handoff) -> Result<(), RuntimeError> {
        self.inner.store.append(
            &aggregate(&record.version.version_id),
            Expected::Seq(sequence),
            vec![NewEvent::new(
                CHANGED,
                serde_json::to_value(record).map_err(|e| invalid(e.to_string()))?,
            )],
        )?;
        Ok(())
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use cyber_core::binary::{
        Document, Offset,
        draft::{PatchBatch, Splice},
        editor::{DraftScope, Editor, Owner, SavePreparation},
        storage::StorageRoot,
    };
    use std::{collections::BTreeMap, sync::Arc};

    async fn saved() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        Runtime,
        Editor,
        super::super::file_observations::FileWatch,
    ) {
        let (project, runtime, session, _, _) =
            super::super::binary_documents_tests::fixture().await;
        let data = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageRoot::prepare(data.path()).unwrap());
        runtime.configure_binary_storage(storage.clone()).unwrap();
        let root = project.path().canonicalize().unwrap();
        let watch = runtime
            .claim_pinned_file_watch(
                root.clone(),
                std::fs::File::open(&root).unwrap(),
                BTreeMap::new(),
            )
            .await
            .unwrap();
        let owner = Owner {
            session_id: session,
            directory: root.clone(),
            root: root.clone(),
            scope: DraftScope::Manual,
        };
        let mut source = Document::open(&root, "bytes").unwrap();
        let mut editor = Editor::create(
            storage.open("bin_handoff").unwrap(),
            owner,
            &mut source,
            || false,
        )
        .unwrap();
        editor
            .patch(
                "patch_one",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "00".into(),
                        insert_hex: "01".into(),
                    }],
                },
            )
            .unwrap();
        let review = editor.review(0, 128, || false).unwrap();
        let SavePreparation::Ready(prepared) = editor
            .prepare_save("saved", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("native save");
        };
        editor.commit_save(*prepared).unwrap();
        (project, data, runtime, editor, watch)
    }
    #[tokio::test]
    async fn handoff_links_real_durable_observations_once_without_running_or_replaying_hooks() {
        let (_project, _data, runtime, mut editor, mut watch) = saved().await;
        let proof = editor.acknowledged_change("saved").unwrap().unwrap();
        let queued = runtime.queue_binary_change(&proof).unwrap();
        assert!(matches!(queued.state, State::Queued));
        assert!(matches!(
            runtime.queue_binary_change(&proof).unwrap().state,
            State::Queued
        ));
        assert!(
            runtime
                .append_binary_handoff(
                    0,
                    &Handoff {
                        state: State::Observed {
                            observation_id: "evt_fake".into()
                        },
                        ..queued
                    }
                )
                .is_err()
        );
        let claimed = runtime.claim_binary_observation(&watch, &proof).unwrap();
        assert!(matches!(claimed.state, State::Claimed { .. }));
        assert!(matches!(
            runtime.reconcile_binary_handoff(proof.key()).unwrap().state,
            State::Claimed { .. }
        ));
        let observation = watch
            .record(observations::Delta {
                generation: watch.state().generation.clone(),
                updates: vec![observations::Update {
                    path: "bytes".into(),
                    stamp: Some(proof.observed_stamp().unwrap()),
                }],
                remaining: watch.state().remaining,
                dispatch: false,
                renewal: None,
            })
            .await
            .unwrap();
        let linked = runtime.reconcile_binary_handoff(proof.key()).unwrap();
        assert!(
            matches!(linked.state,State::Observed {ref observation_id} if observation_id == &observation.id)
        );
        let before = runtime
            .inner
            .store
            .aggregate_seq(&aggregate(proof.key()))
            .unwrap();
        drop(watch); // Reconciliation survives loss of the native generation and launches no callbacks.
        assert!(matches!(
            runtime.reconcile_binary_handoff(proof.key()).unwrap().state,
            State::Observed { .. }
        ));
        assert!(matches!(
            runtime.queue_binary_change(&proof).unwrap().state,
            State::Observed { .. }
        ));
        assert_eq!(
            runtime
                .inner
                .store
                .aggregate_seq(&aggregate(proof.key()))
                .unwrap(),
            before
        );
        assert!(
            runtime
                .append_binary_handoff(
                    before.unwrap(),
                    &Handoff {
                        state: State::Queued,
                        ..linked
                    }
                )
                .is_err()
        );
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn superseded_sources_are_suppressed_and_fake_observation_references_are_rejected() {
        let (project, _data, runtime, mut editor, mut watch) = saved().await;
        let proof = editor.acknowledged_change("saved").unwrap().unwrap();
        runtime.claim_binary_observation(&watch, &proof).unwrap();
        let other = watch
            .record(observations::Delta {
                generation: watch.state().generation.clone(),
                updates: vec![observations::Update {
                    path: "other".into(),
                    stamp: Some(proof.observed_stamp().unwrap()),
                }],
                remaining: watch.state().remaining,
                dispatch: false,
                renewal: None,
            })
            .await
            .unwrap();
        let (sequence, claimed) = runtime.binary_handoff(proof.key()).unwrap().unwrap();
        assert!(
            runtime
                .append_binary_handoff(
                    sequence,
                    &Handoff {
                        state: State::Observed {
                            observation_id: other.id
                        },
                        ..claimed
                    }
                )
                .is_err()
        );
        runtime
            .suppress_binary_change(proof.key(), "handoff_interrupted")
            .unwrap();
        assert!(matches!(
            runtime.reconcile_binary_handoff(proof.key()).unwrap().state,
            State::Suppressed { .. }
        ));
        std::fs::write(project.path().join("bytes"), [9, 8]).unwrap();
        assert!(proof.observed_stamp().is_err());
        assert!(matches!(
            runtime.queue_binary_change(&proof).unwrap().state,
            State::Suppressed { .. }
        ));
        assert_eq!(std::fs::read(project.path().join("bytes")).unwrap(), [9, 8]);
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn a_source_changed_before_handoff_creates_no_successful_observation() {
        let (project, _data, runtime, mut editor, watch) = saved().await;
        let proof = editor.acknowledged_change("saved").unwrap().unwrap();
        std::fs::write(project.path().join("bytes"), [9, 8]).unwrap();
        let disposition = runtime.claim_binary_observation(&watch, &proof).unwrap();
        assert!(
            matches!(disposition.state,State::Suppressed {ref reason} if reason == "source_superseded")
        );
        let (_, sequence, _) = watch.binary_source_binding(&runtime).unwrap();
        assert_eq!(
            sequence, 0,
            "superseded handoff manufactured a native change"
        );
        assert_eq!(std::fs::read(project.path().join("bytes")).unwrap(), [9, 8]);
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn losing_the_original_watcher_cannot_replay_an_unobserved_claim() {
        let (project, _data, runtime, mut editor, watch) = saved().await;
        let change = Arc::new(editor.acknowledged_change("saved").unwrap().unwrap());
        runtime.claim_binary_observation(&watch, &change).unwrap();
        drop(watch);
        let disposition = runtime.publish_binary_change(change.clone()).await.unwrap();
        assert!(
            matches!(disposition.state,State::Suppressed {ref reason} if reason == "handoff_interrupted")
        );
        assert!(matches!(
            runtime.publish_binary_change(change).await.unwrap().state,
            State::Suppressed { .. }
        ));
        assert_eq!(
            std::fs::read(project.path().join("bytes")).unwrap(),
            [1, 255]
        );
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn native_names_outside_the_watch_path_contract_have_an_explicit_disposition() {
        let (project, _data, runtime, editor, watch) = saved().await;
        let owner = editor.owner().clone();
        std::fs::write(project.path().join("outside:watch"), [0, 255]).unwrap();
        let storage = runtime.binary_documents().storage().unwrap();
        let mut source = Document::open(&owner.root, "outside:watch").unwrap();
        let mut other = Editor::create(
            storage.open("bin_unobserved").unwrap(),
            owner,
            &mut source,
            || false,
        )
        .unwrap();
        other
            .patch(
                "patch_one",
                &PatchBatch {
                    revision: other.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "00".into(),
                        insert_hex: "01".into(),
                    }],
                },
            )
            .unwrap();
        let review = other.review(0, 128, || false).unwrap();
        let SavePreparation::Ready(prepared) = other
            .prepare_save("saved", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("save");
        };
        other.commit_save(*prepared).unwrap();
        let change = other.acknowledged_change("saved").unwrap().unwrap();
        let disposition = runtime.claim_binary_observation(&watch, &change).unwrap();
        assert!(
            matches!(disposition.state,State::Suppressed {ref reason} if reason == "unobserved_path")
        );
        assert_eq!(watch.binary_source_binding(&runtime).unwrap().1, 0);
        assert_eq!(
            std::fs::read(project.path().join("outside:watch")).unwrap(),
            [1, 255]
        );
        runtime.shutdown().await;
    }
}
