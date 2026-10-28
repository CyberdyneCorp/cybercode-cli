//! Actual store/runtime regressions for the file-observation journal. These do
//! not stand in for native watcher or complete application FileChanged acceptance.
mod support;
use cyber_core::hooks::file_observations::{self as records, Delta, Renewal, Stamp, Update};
use cyber_server::runtime::{Admission, Delivery};
use cyber_store::{Expected, NewEvent};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use support::{Harness, Setup};

fn update(generation: &str, remaining: u32) -> Delta {
    Delta {
        generation: generation.into(),
        updates: vec![Update {
            path: "nested/data".into(),
            stamp: Some(Stamp {
                length: 8,
                modified_ns: Some(1),
                identity: Some((1, 2)),
            }),
        }],
        remaining,
        dispatch: true,
        renewal: None,
    }
}
fn directory(h: &Harness) -> String {
    h.repo.display().to_string()
}
fn aggregate(h: &Harness) -> String {
    format!("file-watch:{:x}", Sha256::digest(directory(h).as_bytes()))
}

fn session_scope(
    watch: &cyber_server::runtime::file_observations::FileWatch,
    id: &str,
) -> records::SessionScope {
    records::SessionScope {
        session_id: id.into(),
        session_directory: watch.state().directory.clone(),
        directory: watch.state().directory.clone(),
        root_identity: watch.state().root_identity.clone(),
        generation: watch.state().generation.clone(),
    }
}

#[tokio::test]
async fn session_scope_is_idempotent_fenced_and_separate_from_metadata_credit() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let scope = session_scope(&watch, &id);
    watch.admit_session_scope(&id).await.unwrap();
    watch.admit_session_scope(&id).await.unwrap();
    assert_eq!(h.store.aggregate_seq(&scope.aggregate()).unwrap(), Some(0));
    assert!(
        watch.current().unwrap(),
        "scope records cannot advance metadata CAS"
    );
    assert_eq!(watch.state().remaining, records::FEEDBACK_LIMIT);
    let replacement = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    assert!(watch.admit_session_scope(&id).await.is_err());
    replacement.admit_session_scope(&id).await.unwrap();
    assert_eq!(h.store.aggregate_seq(&scope.aggregate()).unwrap(), Some(1));
    h.runtime.archive(&id, true).await.unwrap();
    assert!(replacement.admit_session_scope(&id).await.is_err());
    assert_eq!(h.store.aggregate_seq(&scope.aggregate()).unwrap(), Some(1));
}

#[tokio::test]
async fn scope_projector_rejects_wrong_owner_generation_session_and_aggregate() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let scope = session_scope(&watch, &id);
    for field in ["root", "generation", "session_directory", "aggregate"] {
        let mut forged = scope.clone();
        let mut key = scope.aggregate();
        match field {
            "root" => forged.root_identity = "other-root".into(),
            "generation" => forged.generation = records::new_generation(),
            "session_directory" => forged.session_directory = h.dir.path().display().to_string(),
            "aggregate" => key.push_str("-wrong"),
            _ => unreachable!(),
        }
        assert!(
            h.store
                .append(
                    &key,
                    Expected::Seq(-1),
                    vec![NewEvent::new(
                        records::SCOPE,
                        serde_json::to_value(forged).unwrap()
                    )]
                )
                .is_err(),
            "{field}"
        );
    }
    assert_eq!(h.store.aggregate_seq(&scope.aggregate()).unwrap(), None);
    assert!(watch.current().unwrap());
}

#[tokio::test]
async fn scope_metadata_does_not_grant_cross_directory_admission() {
    let h = Harness::new(Setup::default());
    let elsewhere = h.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let session = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: elsewhere.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let mut scope = session_scope(&watch, &session.id);
    scope.session_directory = session.directory;
    // A persisted metadata description is not native proof. Until native root
    // ownership is connected, the producer must keep its exact-directory gate.
    h.store
        .append(
            &scope.aggregate(),
            Expected::Seq(-1),
            vec![NewEvent::new(
                records::SCOPE,
                serde_json::to_value(&scope).unwrap(),
            )],
        )
        .unwrap();
    assert!(watch.admit_session_scope(&session.id).await.is_err());
    assert!(h.models.requests("test/main").is_empty());
}

#[tokio::test]
async fn restart_claim_fences_previous_worker_and_retains_metadata_and_allowance() {
    let h = Harness::new(Setup::default());
    let mut old = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    old.record(update(&old.state().generation, records::FEEDBACK_LIMIT - 1))
        .await
        .unwrap();
    let acknowledged = old.state().clone();
    let restarted = h.restart();
    let new = restarted
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    assert_ne!(new.state().generation, acknowledged.generation);
    assert_eq!(new.state().files, acknowledged.files);
    assert_eq!(new.state().remaining, acknowledged.remaining);
    assert!(!old.current().unwrap());
    assert!(
        old.record(update(
            &acknowledged.generation,
            records::FEEDBACK_LIMIT - 2
        ))
        .await
        .is_err()
    );
    assert_eq!(
        old.state(),
        &acknowledged,
        "failed stale append cannot advance acknowledged state"
    );
}

#[tokio::test]
async fn failed_append_does_not_consume_change_or_dispatch_allowance() {
    let h = Harness::new(Setup::default());
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let original = watch.state().clone();
    h.store
        .set_append_fault(Some(Arc::new(|_, event| event.kind == records::CHANGED)));
    let delta = update(&original.generation, records::FEEDBACK_LIMIT - 1);
    assert!(watch.record(delta.clone()).await.is_err());
    assert_eq!(watch.state(), &original);
    h.store.set_append_fault(None);
    let receipt = watch.record(delta).await.unwrap();
    assert_eq!(receipt.kind, records::CHANGED);
    assert_eq!(watch.state().remaining, records::FEEDBACK_LIMIT - 1);
}

#[tokio::test]
async fn periodic_checkpoint_bounds_replay_without_replaying_hook_effects() {
    let h = Harness::new(Setup::default());
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    for index in 0..(records::MAX_REPLAY + 3) {
        let mut delta = update(&watch.state().generation, records::FEEDBACK_LIMIT);
        delta.dispatch = false;
        delta.updates[0].stamp.as_mut().unwrap().length = index as u64;
        watch.record(delta).await.unwrap();
    }
    let acknowledged = watch.state().clone();
    let restarted = h.restart();
    let replacement = restarted
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(replacement.state().files, acknowledged.files);
    assert_eq!(replacement.state().remaining, acknowledged.remaining);
    assert!(
        h.models.requests("test/main").is_empty(),
        "replay must not infer or run a model"
    );
}

#[tokio::test]
async fn projector_refuses_checkpoint_scope_credit_and_metadata_rewrites() {
    let h = Harness::new(Setup::default());
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    watch
        .record(update(
            &watch.state().generation,
            records::FEEDBACK_LIMIT - 1,
        ))
        .await
        .unwrap();
    for field in ["scope", "credit", "metadata"] {
        let mut forged = watch.state().clone();
        match field {
            "scope" => forged.directory = h.dir.path().display().to_string(),
            "credit" => forged.remaining = records::FEEDBACK_LIMIT,
            "metadata" => forged.files.clear(),
            _ => unreachable!(),
        }
        let sequence = h.store.aggregate_seq(&aggregate(&h)).unwrap().unwrap();
        assert!(
            h.store
                .append(
                    &aggregate(&h),
                    Expected::Seq(sequence),
                    vec![NewEvent::new(
                        records::CHECKPOINT,
                        serde_json::to_value(forged).unwrap()
                    )]
                )
                .is_err(),
            "{field}"
        );
    }
    assert!(
        watch.current().unwrap(),
        "failed writes roll back the event sequence"
    );
}

async fn admission_event(h: &Harness, id: &str, source: &str) -> String {
    let mut input = Admission::text("acknowledge observations", Delivery::Hold);
    input.source = source.into();
    h.runtime.admit(id, input).await.unwrap();
    h.store
        .read_events(id, -1, cyber_store::MAX_PAGE_LIMIT)
        .unwrap()
        .events
        .into_iter()
        .rev()
        .find(|event| event.kind == "session.prompt.admitted.1")
        .unwrap()
        .id
}
fn renewal(generation: &str, session_id: &str, event_id: &str) -> Delta {
    Delta {
        generation: generation.into(),
        updates: vec![],
        remaining: records::FEEDBACK_LIMIT,
        dispatch: false,
        renewal: Some(Renewal {
            session_id: session_id.into(),
            event_id: event_id.into(),
        }),
    }
}

#[tokio::test]
async fn only_owned_user_input_can_renew_once_across_a_restart() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    watch
        .record(update(
            &watch.state().generation,
            records::FEEDBACK_LIMIT - 1,
        ))
        .await
        .unwrap();
    let hook = admission_event(&h, &id, "hook").await;
    assert!(
        watch
            .record(renewal(&watch.state().generation, &id, &hook))
            .await
            .is_err()
    );
    let user = admission_event(&h, &id, "user").await;
    watch
        .record(renewal(&watch.state().generation, &id, &user))
        .await
        .unwrap();
    assert_eq!(watch.state().remaining, records::FEEDBACK_LIMIT);
    let restarted = h.restart();
    let mut replacement = restarted
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    assert!(
        replacement
            .record(renewal(&replacement.state().generation, &id, &user))
            .await
            .is_err()
    );
    assert_eq!(replacement.state().remaining, records::FEEDBACK_LIMIT);
}

#[tokio::test]
async fn current_session_projection_cannot_rebind_an_earlier_user_acknowledgement() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let user = admission_event(&h, &id, "user").await;
    let elsewhere = h.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let target = elsewhere.display().to_string();
    // Exercise the projector's historical binding independently of a current
    // Session row: changing that row cannot change where the input was admitted.
    let session = id.clone();
    let location = target.clone();
    h.store
        .transaction(move |tx| {
            tx.execute(
                "UPDATE session SET directory=?2 WHERE id=?1",
                rusqlite::params![session, location],
            )?;
            Ok(())
        })
        .unwrap();
    let mut watch = h
        .runtime
        .claim_file_watch(target, "other-root".into(), BTreeMap::new())
        .await
        .unwrap();
    assert!(
        watch
            .record(renewal(&watch.state().generation, &id, &user))
            .await
            .is_err()
    );
}

fn file_definition(h: &Harness) -> cyber_core::hooks::HookDefinition {
    use cyber_core::config::{self, LoadRequest};
    use cyber_core::hooks::HookCatalog;
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        h.dir.path().join("file-hook-config").display().to_string(),
    )]);
    let paths = cyber_core::paths::Paths::resolve(&env, h.dir.path());
    paths.ensure().unwrap();
    std::fs::write(paths.config.join("cyber.jsonc"),serde_json::json!({"hooks":{"FileChanged":[{"hooks":[{"type":"command","command":"printf reviewed","id":"file-guard"}]}]}}).to_string()).unwrap();
    let resolved = config::load(&LoadRequest {
        location: &h.repo,
        paths: &paths,
        env: &env,
        home: h.dir.path(),
        profile: None,
        overrides: &[],
        flags: serde_json::json!({}),
    })
    .unwrap();
    HookCatalog::from_config(&resolved)
        .unwrap()
        .definitions
        .remove(0)
}
async fn file_event(
    h: &Harness,
    id: &str,
    observation: &cyber_store::StoredEvent,
) -> cyber_core::hooks::HookEvent {
    let info = h.runtime.state(id).await.unwrap().info;
    let identity = h.runtime.session_envelope(id).await.unwrap().identity(
        id,
        &info.directory,
        &info.agent,
        &info.mode,
    );
    cyber_core::hooks::HookEvent::new("FileChanged",identity,1,serde_json::json!({
        "observation_id":observation.id,"watch_generation":observation.data["generation"],"file_path":"nested/data"
    }).as_object().unwrap().clone()).unwrap()
}

#[tokio::test]
async fn unknown_file_handler_delivery_remains_consumed_without_reexecution() {
    use cyber_server::runtime::HookExecutionStatus;
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let source = watch
        .record(update(
            &watch.state().generation,
            records::FEEDBACK_LIMIT - 1,
        ))
        .await
        .unwrap();
    let event = file_event(&h, &id, &source).await;
    let definition = file_definition(&h);
    let owner = h
        .runtime
        .try_start_hook_execution(&event, &definition, false)
        .await
        .unwrap()
        .unwrap();
    let receipt_id = owner.record().id.clone();
    drop(owner);
    assert_eq!(
        h.runtime.hook_executions(&id, 10).unwrap()[0].status,
        HookExecutionStatus::Unknown
    );
    assert!(
        h.runtime
            .try_start_hook_execution(&event, &definition, false)
            .await
            .unwrap()
            .is_none()
    );
    let restarted = h.restart();
    assert!(
        restarted
            .try_start_hook_execution(&event, &definition, false)
            .await
            .unwrap()
            .is_none()
    );
    let receipts = restarted.hook_executions(&id, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].id, receipt_id);
    assert_eq!(receipts[0].status, HookExecutionStatus::Unknown);
}

#[tokio::test]
async fn stale_observation_generation_cannot_admit_a_new_handler() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut old = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let source = old
        .record(update(&old.state().generation, records::FEEDBACK_LIMIT - 1))
        .await
        .unwrap();
    let event = file_event(&h, &id, &source).await;
    let _new = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    assert!(
        h.runtime
            .try_start_hook_execution(&event, &file_definition(&h), false)
            .await
            .is_err()
    );
    assert!(h.runtime.hook_executions(&id, 10).unwrap().is_empty());
}

#[tokio::test]
async fn archived_session_cannot_admit_a_prepared_file_observation() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut watch = h
        .runtime
        .claim_file_watch(directory(&h), "root-one".into(), BTreeMap::new())
        .await
        .unwrap();
    let source = watch
        .record(update(
            &watch.state().generation,
            records::FEEDBACK_LIMIT - 1,
        ))
        .await
        .unwrap();
    let event = file_event(&h, &id, &source).await;
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    h.store
        .append(
            &id,
            Expected::Seq(seq),
            vec![NewEvent::new(
                "session.archived.1",
                serde_json::json!({"archived":true}),
            )],
        )
        .unwrap();
    assert!(
        h.runtime
            .try_start_hook_execution(&event, &file_definition(&h), false)
            .await
            .is_err()
    );
    assert!(h.runtime.hook_executions(&id, 10).unwrap().is_empty());
}
