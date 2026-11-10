//! Multiple real Locations share one App and its serialized Store writer.
use super::*;

#[derive(Clone, Copy)]
enum Route {
    Missing,
    Replacement,
    PrivateLink,
}
struct Displaced {
    retained: PathBuf,
    owner: PathBuf,
    route: Route,
}
impl Displaced {
    fn paths(f: &Fixture, route: Route) -> Self {
        let retained = f.project.with_file_name("retained-original");
        let owner = f.paths.tmp.join("later-location-owner");
        std::fs::create_dir(&owner).unwrap();
        std::fs::write(owner.join("owner-source"), b"later Location bytes").unwrap();
        Self {
            retained,
            owner,
            route,
        }
    }
    fn new(f: &Fixture, route: Route) -> Self {
        let displaced = Self::paths(f, route);
        std::fs::rename(&f.project, &displaced.retained).unwrap();
        match route {
            Route::Missing => {}
            Route::Replacement => {
                std::fs::create_dir(&f.project).unwrap();
                std::fs::write(
                    f.project.join("owner-source"),
                    b"replacement Location bytes",
                )
                .unwrap();
            }
            Route::PrivateLink => std::os::unix::fs::symlink(&displaced.owner, &f.project).unwrap(),
        }
        // Later source ownership must survive both refusal and eventual cleanup.
        std::fs::write(displaced.retained.join("file_0"), b"later source owner").unwrap();
        displaced
    }
    fn restore(self, f: &Fixture) {
        assert_eq!(
            std::fs::read(self.owner.join("owner-source")).unwrap(),
            b"later Location bytes"
        );
        match self.route {
            Route::Missing => assert!(f.project.symlink_metadata().is_err()),
            Route::Replacement => {
                assert_eq!(
                    std::fs::read(f.project.join("owner-source")).unwrap(),
                    b"replacement Location bytes"
                );
                std::fs::remove_file(f.project.join("owner-source")).unwrap();
                std::fs::remove_dir(&f.project).unwrap();
            }
            Route::PrivateLink => {
                assert_eq!(std::fs::read_link(&f.project).unwrap(), self.owner);
                std::fs::remove_file(&f.project).unwrap();
            }
        }
        std::fs::rename(self.retained, &f.project).unwrap();
    }
}

#[tokio::test]
async fn unavailable_old_location_defers_payload_but_not_other_location_retirement() {
    exercise(false).await;
}
#[tokio::test]
async fn unavailable_old_location_preserves_pending_metadata_and_other_locations_progress() {
    exercise(true).await;
}

async fn exercise(metadata_pending: bool) {
    for route in [Route::Missing, Route::Replacement, Route::PrivateLink] {
        exercise_route(metadata_pending, route).await;
    }
}
async fn exercise_route(metadata_pending: bool, route: Route) {
    let f = Fixture::new().await;
    let other = f.project.with_file_name("other-location");
    std::fs::create_dir(&other).unwrap();
    let restored = Restore::new(&f.app, &f.paths, &other, Some("after!\n".repeat(4096))).await;
    f.age();
    age(&f.app, &restored.receipt);
    if metadata_pending {
        assert!(f.reclaim().await.unwrap() > 0);
        native_retirement::plan_before_deletion(&f).await;
    }
    let witness = Witness::new(&f);
    let displaced = Displaced::new(&f, route);
    assert!(f.reclaim().await.is_err());
    if metadata_pending {
        assert!(
            f.snapshots
                .retire(f.project.to_str().unwrap(), f.receipt.clone().into())
                .await
                .is_err()
        );
    }
    f.snapshots.gc().await;
    witness.unchanged(&f);
    assert_other_retired(&f, &restored, &other).await;
    resume(&f, displaced, &witness).await;
}
async fn resume(f: &Fixture, displaced: Displaced, witness: &Witness) {
    displaced.restore(f);
    f.snapshots.gc().await;
    assert!(f.record().unwrap().retired);
    assert!(!f.journal.exists());
    assert_eq!(
        std::fs::read(f.project.join("file_0")).unwrap(),
        b"later source owner"
    );
    let reply = f
        .client
        .post(
            &format!(
                "/sessions/{}/rewind/restores/{}/acknowledge",
                f.session, f.receipt.origin.activity_id
            ),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_value::<SnapshotRestoreReceipt>(reply["data"].clone()).unwrap(),
        f.receipt
    );
    assert_eq!(sequence(&f.app, &f.session), witness.session_sequence);
    assert_eq!(
        sequence(&f.app, &f.receipt.origin.activity_id),
        witness.owner_sequence
    );
    assert_eq!(f.model.requests().len(), witness.requests);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn location_disappears_after_a_native_metadata_unlink_and_resumes_the_same_plan() {
    let f = Fixture::new().await;
    f.age();
    assert!(f.reclaim().await.unwrap() > 0);
    let witness = Witness::new(&f);
    let (snapshots, displaced) = relocate_after_intent(&f);
    assert!(
        snapshots
            .retire(f.project.to_str().unwrap(), f.receipt.clone().into())
            .await
            .is_err()
    );
    assert!(!f.project.exists());
    assert!(!f.journal.join("intent.json").exists());
    assert!(f.journal.join("intent.next").exists());
    let pending = f.record().unwrap();
    assert!(pending.completed && !pending.retired);
    let plan = pending.metadata.unwrap();
    assert_eq!(sequence(&f.app, &records::Plan::id(&f.receipt).unwrap()), 2);
    resume(&f, displaced, &witness).await;
    assert_eq!(f.record().unwrap().metadata, Some(plan));
}

fn relocate_after_intent(f: &Fixture) -> (Arc<GitSnapshots>, Displaced) {
    let displaced = Displaced::paths(f, Route::Missing);
    let retained = displaced.retained.clone();
    let source = f.project.clone();
    let intent = f.journal.join("intent.json");
    let snapshots = GitSnapshots::with_reclamation(
        f.paths.data.clone(),
        Arc::new(move |_| {
            if !intent.exists() && source.exists() {
                std::fs::rename(&source, &retained).unwrap();
                std::fs::write(retained.join("file_0"), b"later source owner").unwrap();
            }
            json!({"snapshots":{"retention_days":7}})
        }),
        vec![
            f.paths.config.clone(),
            f.paths.state.clone(),
            f.paths.cache.clone(),
            f.paths.tmp.clone(),
        ],
        None,
        Some(cyber_app::snapshot_reclamation::policy(f.app.store.clone())),
    );
    (snapshots, displaced)
}

struct Witness {
    journal: std::collections::BTreeMap<String, Vec<u8>>,
    workers: std::collections::BTreeMap<String, Vec<u8>>,
    metadata: Option<records::metadata::Plan>,
    reclamation_sequence: i64,
    session_sequence: i64,
    owner_sequence: i64,
    requests: usize,
}
impl Witness {
    fn new(f: &Fixture) -> Self {
        Self {
            journal: image_bytes(&f.journal),
            workers: image_bytes(&workers(f)),
            metadata: f.record().and_then(|record| record.metadata),
            reclamation_sequence: sequence(&f.app, &records::Plan::id(&f.receipt).unwrap()),
            session_sequence: sequence(&f.app, &f.session),
            owner_sequence: sequence(&f.app, &f.receipt.origin.activity_id),
            requests: f.model.requests().len(),
        }
    }
    fn unchanged(&self, f: &Fixture) {
        assert_eq!(image_bytes(&f.journal), self.journal);
        assert_eq!(image_bytes(&workers(f)), self.workers);
        assert_eq!(f.record().and_then(|record| record.metadata), self.metadata);
        assert_eq!(
            sequence(&f.app, &records::Plan::id(&f.receipt).unwrap()),
            self.reclamation_sequence
        );
        assert_eq!(sequence(&f.app, &f.session), self.session_sequence);
        assert_eq!(
            sequence(&f.app, &f.receipt.origin.activity_id),
            self.owner_sequence
        );
        assert_eq!(f.model.requests().len(), self.requests);
    }
}
fn workers(f: &Fixture) -> PathBuf {
    f.journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workers")
}
fn sequence(app: &App, aggregate: &str) -> i64 {
    let aggregate = aggregate.to_owned();
    app.store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT COALESCE(MAX(seq),-1) FROM event WHERE aggregate_id=?1",
                [aggregate],
                |row| row.get(0),
            )?)
        })
        .unwrap()
}
fn age(app: &App, receipt: &SnapshotRestoreReceipt) {
    let owner = receipt.origin.activity_id.clone();
    app.store
        .transaction(move |db| {
            db.execute(
                "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                rusqlite::params![8i64 * 86400000, owner],
            )?;
            Ok(())
        })
        .unwrap();
}
async fn assert_other_retired(f: &Fixture, restored: &Restore, other: &std::path::Path) {
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let record = policy
        .record(&restored.receipt.clone().into())
        .unwrap()
        .unwrap();
    assert!(record.completed && record.retired);
    assert!(!restored.journal.exists());
    assert_eq!(
        std::fs::read(other.join("file_0")).unwrap(),
        "before\n".repeat(4096).as_bytes()
    );
    let reply = restored
        .client
        .post(
            &format!(
                "/sessions/{}/rewind/restores/{}/acknowledge",
                restored.session, restored.receipt.origin.activity_id
            ),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_value::<SnapshotRestoreReceipt>(reply["data"].clone()).unwrap(),
        restored.receipt
    );
}
