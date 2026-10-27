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
