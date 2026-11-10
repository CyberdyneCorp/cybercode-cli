//! Writer protocol only: events do not perform or prove native metadata deletion.
use super::*;
use cyber_server::runtime::snapshot_reclamation::metadata::{self, File, Plan, Role};
use cyber_store::{Expected, NewEvent};
use std::os::unix::fs::MetadataExt;

fn plan(f: &Fixture) -> Plan {
    let lifetime = &f.receipt.checkout.as_ref().unwrap().lifetime;
    let workers = f
        .journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workers");
    let files = [
        (Role::Intent, f.journal.join("intent.json")),
        (Role::Spare, f.journal.join("intent.next")),
        (Role::Lifetime, workers.join(&lifetime.owner.activity_id)),
        (
            Role::SourcePhase,
            workers.join(lifetime.source_phase.as_ref().unwrap()),
        ),
    ]
    .into_iter()
    .map(|(role, path)| {
        let metadata = std::fs::metadata(&path).unwrap();
        let birth = metadata
            .created()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let generation = serde_json::to_vec(&(
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.uid(),
            metadata.gid(),
            metadata.nlink(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ))
        .unwrap();
        File {
            owner: lifetime.owner.activity_id.clone(),
            role,
            identity: format!(
                "{}:{}:{}:{}",
                metadata.dev(),
                metadata.ino(),
                birth.as_secs(),
                birth.subsec_nanos()
            ),
            generation: blake3::hash(&generation).to_hex().to_string(),
            hash: blake3::hash(&std::fs::read(path).unwrap())
                .to_hex()
                .to_string(),
            bytes: metadata.len(),
        }
    })
    .collect();
    Plan {
        version: 1,
        payload_plan_id: records::Plan::id(&f.receipt).unwrap(),
        files,
    }
}
fn append(
    f: &Fixture,
    sequence: i64,
    kind: &str,
    value: Value,
) -> Result<(), cyber_store::StoreError> {
    f.app
        .store
        .append(
            &records::Plan::id(&f.receipt).unwrap(),
            Expected::Seq(sequence),
            vec![NewEvent::new(kind, value)],
        )
        .map(|_| ())
}
fn completed(plan: &Plan) -> Value {
    json!(records::Completion {
        plan_id: plan.payload_plan_id.clone(),
        removed_bytes: plan.total_bytes().unwrap()
    })
}
async fn payload_complete(f: &Fixture) -> Plan {
    f.age();
    assert!(f.reclaim().await.unwrap() > 0);
    let plan = plan(f);
    plan.validate(&f.record().unwrap().plan).unwrap();
    plan
}

#[tokio::test]
async fn metadata_history_distinguishes_phases_and_reopens_without_native_effects() {
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    let intent = std::fs::read(f.journal.join("intent.json")).unwrap();
    let requests = f.model.requests().len();
    append(&f, 1, metadata::PLANNED, json!(plan)).unwrap();
    let record = f.record().unwrap();
    assert!(record.completed && !record.retired);
    assert_eq!(record.metadata, Some(plan.clone()));
    // The trusted writer is explicitly supplying a completion in this Store
    // protocol test. Native retirement must separately validate its deletion
    // callback; accepting these events is not native deletion acceptance.
    append(&f, 2, metadata::COMPLETED, completed(&plan)).unwrap();
    assert!(f.record().unwrap().retired);
    let receipt = f.receipt.clone();
    f.app.runtime.shutdown().await;
    let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
        DatabaseLocation::File(f.paths.data.join("cyber.db")),
        cyber_server::runtime::Runtime::registry(),
    ))
    .unwrap();
    let record = reopened
        .read(move |db| records::lookup(db, &receipt))
        .unwrap()
        .unwrap();
    assert!(record.completed && record.retired);
    assert_eq!(record.metadata, Some(plan));
    assert_eq!(
        std::fs::read(f.journal.join("intent.json")).unwrap(),
        intent
    );
    assert_eq!(f.model.requests().len(), requests);
}

#[tokio::test]
async fn metadata_retirement_cannot_precede_payload_completion() {
    let f = Fixture::new().await;
    f.age();
    planned_without_deletion(&f).await;
    let plan = plan(&f);
    let notifications = f.app.store.subscribe();
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    assert!(
        policy
            .retire(
                f.record().unwrap().plan,
                plan.clone(),
                Box::new(|| panic!("incomplete payload invoked retirement"))
            )
            .is_err()
    );
    assert!(append(&f, 0, metadata::PLANNED, json!(plan)).is_err());
    assert!(!f.record().unwrap().completed);
    assert!(f.record().unwrap().metadata.is_none());
    assert!(f.journal.join("stage_0").exists());
    assert!(notifications.try_recv().is_err());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn metadata_plan_rejects_foreign_missing_duplicate_and_unbounded_evidence() {
    let f = Fixture::new().await;
    let valid = payload_complete(&f).await;
    let notifications = f.app.store.subscribe();
    for mode in [
        "version",
        "plan",
        "owner",
        "owner_size",
        "missing",
        "duplicate",
        "array_size",
        "identity",
        "alias",
        "cancellation",
        "hash",
        "size",
        "path",
    ] {
        let mut value = json!(valid);
        corrupt(&mut value, mode);
        assert!(append(&f, 1, metadata::PLANNED, value).is_err(), "{mode}");
        assert!(f.record().unwrap().metadata.is_none(), "{mode}");
    }
    assert!(notifications.try_recv().is_err());
    assert!(f.journal.join("intent.json").exists() && f.journal.join("intent.next").exists());
    f.app.runtime.shutdown().await;
}
fn corrupt(plan: &mut Value, mode: &str) {
    match mode {
        "version" => plan["version"] = json!(2),
        "plan" => plan["payload_plan_id"] = json!(format!("snapshot_reclaim_{}", "f".repeat(64))),
        "owner" => plan["files"][0]["owner"] = json!("op_foreign"),
        "owner_size" => plan["files"][0]["owner"] = json!("x".repeat(128 * 1024)),
        "missing" => {
            plan["files"].as_array_mut().unwrap().remove(2);
        }
        "duplicate" => {
            let file = plan["files"][0].clone();
            plan["files"].as_array_mut().unwrap().push(file);
        }
        "array_size" => plan["files"] = json!(vec![plan["files"][0].clone(); 11]),
        "identity" => plan["files"][2]["identity"] = json!("1:2:3:4"),
        "alias" => plan["files"][1]["identity"] = plan["files"][0]["identity"].clone(),
        "cancellation" => plan["files"][1]["role"] = json!("cancellation"),
        "hash" => plan["files"][0]["hash"] = json!("not a digest"),
        "size" => plan["files"][0]["bytes"] = json!(u64::MAX),
        "path" => plan["files"][0]["path"] = json!("../../owner"),
        _ => panic!("unknown corruption"),
    }
}

#[tokio::test]
async fn metadata_plan_is_immutable_and_completion_rechecks_later_native_fences() {
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    append(&f, 1, metadata::PLANNED, json!(plan)).unwrap();
    let payload = f.record().unwrap().plan;
    let mut rewritten = payload.clone();
    rewritten.files[0].identity = "1:2:3:4".into();
    let same_receipt = plan.clone();
    assert!(
        f.app
            .store
            .read(move |db| same_receipt.authorize(db, &rewritten))
            .is_err()
    );
    let mut changed = plan.clone();
    changed.files[0].generation = "0".repeat(64);
    assert!(
        f.app
            .store
            .read(move |db| changed.authorize(db, &payload))
            .is_err()
    );
    let mut completion = completed(&plan);
    completion["removed_bytes"] = json!(plan.total_bytes().unwrap() + 1);
    assert!(append(&f, 2, metadata::COMPLETED, completion).is_err());
    f.app
        .store
        .append(
            "op_later_unknown",
            Expected::Seq(-1),
            vec![NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":f.session,"status":"unknown","phase":"legacy_process"}),
            )],
        )
        .unwrap();
    assert!(append(&f, 2, metadata::COMPLETED, completed(&plan)).is_err());
    assert!(!f.record().unwrap().retired);
    assert_eq!(
        f.app
            .store
            .reproject(
                &[],
                "snapshot.reclamation.",
                Arc::new(|db, event| records::project(db, event))
            )
            .unwrap(),
        3
    );
    assert!(f.record().unwrap().metadata.is_some());
    assert!(f.journal.join("intent.json").exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn completed_metadata_history_reprojects_after_session_deletion() {
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    append(&f, 1, metadata::PLANNED, json!(plan)).unwrap();
    append(&f, 2, metadata::COMPLETED, completed(&plan)).unwrap();
    f.app.runtime.delete(&f.session).await.unwrap();
    assert_eq!(
        f.app
            .store
            .reproject(
                &[],
                "snapshot.reclamation.",
                Arc::new(|db, event| records::project(db, event))
            )
            .unwrap(),
        4
    );
    assert!(f.record().unwrap().retired);
    assert!(append(&f, 3, metadata::COMPLETED, completed(&plan)).is_err());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn metadata_writer_retains_the_plan_and_never_replays_a_completed_callback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    let payload = f.record().unwrap().plan;
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    assert!(
        policy
            .retire(
                payload.clone(),
                plan.clone(),
                Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err("interrupted native callback".into())
                })
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.record().unwrap().metadata, Some(plan.clone()));
    assert!(!f.record().unwrap().retired);
    let mut changed = plan.clone();
    changed.files[0].generation = "0".repeat(64);
    assert!(
        policy
            .retire(
                payload.clone(),
                changed,
                Box::new(|| panic!("changed plan invoked native work"))
            )
            .is_err()
    );
    let total = plan.total_bytes().unwrap();
    let count = calls.clone();
    assert!(
        policy
            .retire(
                payload.clone(),
                plan.clone(),
                Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(total + 1)
                })
            )
            .is_err()
    );
    assert!(!f.record().unwrap().retired);
    let count = calls.clone();
    assert_eq!(
        policy
            .retire(
                payload.clone(),
                plan.clone(),
                Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(total)
                })
            )
            .unwrap(),
        total
    );
    assert!(f.record().unwrap().retired);
    assert_eq!(
        policy
            .retire(
                payload,
                plan,
                Box::new(|| panic!("retired callback replayed"))
            )
            .unwrap(),
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(f.journal.join("intent.json").exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn metadata_callback_excludes_the_reference_writer() {
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    let payload = f.record().unwrap().plan;
    let total = plan.total_bytes().unwrap();
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let (started, entered) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let retiring = std::thread::spawn(move || {
        policy.retire(
            payload,
            plan,
            Box::new(move || {
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(total)
            }),
        )
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let wrote = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (store, flag) = (f.app.store.clone(), wrote.clone());
    let writer = std::thread::spawn(move || {
        store.transaction(move |_| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!wrote.load(std::sync::atomic::Ordering::SeqCst));
    release.send(()).unwrap();
    assert_eq!(retiring.join().unwrap().unwrap(), total);
    writer.join().unwrap().unwrap();
    assert!(f.record().unwrap().retired);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn legacy_maintenance_policy_refuses_metadata_work_without_invoking_the_callback() {
    let f = Fixture::new().await;
    let plan = payload_complete(&f).await;
    let unavailable = Cut {
        inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
        after_delete: false,
        pause: None,
        tamper: None,
    };
    let error = unavailable
        .retire(
            f.record().unwrap().plan,
            plan,
            Box::new(|| panic!("unsupported retirement invoked native work")),
        )
        .unwrap_err();
    assert!(error.contains("unavailable"));
    assert!(f.record().unwrap().metadata.is_none());
    assert!(f.journal.join("intent.json").exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn retirement_discovery_pages_stably_and_filters_location_without_granting_native_authority()
{
    let f = Fixture::new().await;
    payload_complete(&f).await;
    let prototype = f.record().unwrap().plan;
    // Pagination exercises only stored index discovery. These deliberately
    // uncommitted receipt variants cannot authorize any native retirement.
    let mut expected =
        std::collections::BTreeSet::from([records::Plan::id(&prototype.receipt).unwrap()]);
    for index in 1..=32u32 {
        let mut payload = prototype.clone();
        let records::Receipt::Restore(receipt) = &mut payload.receipt else {
            unreachable!()
        };
        receipt.events_digest = format!("{index:064x}");
        let id = records::Plan::id(&payload.receipt).unwrap();
        expected.insert(id.clone());
        let bytes = payload.total_bytes().unwrap();
        f.app.store.transaction(move |db| {
            db.execute("INSERT INTO event(id,aggregate_id,seq,type,data,time) VALUES (?1,?2,0,?3,?4,1)",
                rusqlite::params![format!("evt_meta_plan_{index}"),id,records::PLANNED,json!(payload).to_string()])?;
            db.execute("INSERT INTO event(id,aggregate_id,seq,type,data,time) VALUES (?1,?2,1,?3,?4,1)",
                rusqlite::params![format!("evt_meta_done_{index}"),id,records::COMPLETED,json!(records::Completion {plan_id:id.clone(),removed_bytes:bytes}).to_string()])?;
            Ok(())
        }).unwrap();
    }
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let first = policy
        .retirement_batch(Some(f.project.to_str().unwrap()), None)
        .unwrap();
    assert_eq!(first.receipts.len(), 32);
    let next = policy
        .retirement_batch(Some(f.project.to_str().unwrap()), first.after.as_deref())
        .unwrap();
    assert_eq!(next.receipts.len(), 1);
    assert!(next.after.is_none());
    let actual = first
        .receipts
        .into_iter()
        .chain(next.receipts)
        .map(|receipt| records::Plan::id(&receipt).unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert!(
        policy
            .retirement_batch(Some("/another-location"), None)
            .unwrap()
            .receipts
            .is_empty()
    );
    assert!(f.journal.join("intent.json").exists());
    f.app.runtime.shutdown().await;
}
