//! Actual native metadata retirement; retained receipts never replay source work.
use super::*;

async fn payload_complete(f: &Fixture) {
    f.age();
    assert!(f.reclaim().await.unwrap() > 0);
    assert!(f.record().unwrap().completed);
}
async fn retire(f: &Fixture) -> Result<u64, String> {
    f.snapshots
        .retire(f.project.to_str().unwrap(), f.receipt.clone().into())
        .await
}
fn worker_routes(f: &Fixture) -> [PathBuf; 2] {
    let lifetime = &f.receipt.checkout.as_ref().unwrap().lifetime;
    let directory = f
        .journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workers");
    [
        directory.join(&lifetime.owner.activity_id),
        directory.join(lifetime.source_phase.as_ref().unwrap()),
    ]
}

#[tokio::test]
async fn native_retirement_removes_exact_metadata_and_acknowledges_from_the_retained_store_receipt()
{
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    payload_complete(&f).await;
    let routes = worker_routes(&f);
    let inference = f.model.requests().len();
    let messages = f
        .client
        .get(&format!("/sessions/{}/messages?order=asc", f.session))
        .await
        .unwrap();
    let owner = f.project.parent().unwrap().join("later-owner");
    std::fs::write(&owner, b"later owner source bytes").unwrap();
    std::fs::remove_file(f.project.join("file_0")).unwrap();
    std::os::unix::fs::symlink(&owner, f.project.join("file_0")).unwrap();
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o0)).unwrap();
    assert!(retire(&f).await.unwrap() > 0);
    assert!(f.record().unwrap().retired);
    assert!(!f.journal.exists());
    for path in routes {
        assert!(!path.exists(), "{}", path.display());
    }
    assert_eq!(retire(&f).await.unwrap(), 0);
    let receipt = f
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
        serde_json::from_value::<SnapshotRestoreReceipt>(receipt["data"].clone()).unwrap(),
        f.receipt
    );
    assert_eq!(std::fs::read_link(f.project.join("file_0")).unwrap(), owner);
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(owner).unwrap(), b"later owner source bytes");
    assert_eq!(
        f.client
            .get(&format!("/sessions/{}/messages?order=asc", f.session))
            .await
            .unwrap(),
        messages
    );
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}

struct Cut {
    inner: Arc<dyn Policy>,
    after: bool,
    tamper: Option<PathBuf>,
}
impl Policy for Cut {
    fn receipt(
        &self,
        origin: &cyber_server::runtime::SnapshotOrigin,
    ) -> Result<Option<Settlement>, String> {
        self.inner.receipt(origin)
    }
    fn record(&self, receipt: &records::Receipt) -> Result<Option<records::Record>, String> {
        self.inner.record(receipt)
    }
    fn reclaim(&self, plan: records::Plan, delete: Delete) -> Result<u64, String> {
        self.inner.reclaim(plan, delete)
    }
    fn retire(
        &self,
        payload: records::Plan,
        plan: records::metadata::Plan,
        delete: Delete,
    ) -> Result<u64, String> {
        let after = self.after;
        let tamper = self.tamper.clone();
        self.inner.retire(
            payload,
            plan,
            Box::new(move || {
                if let Some(path) = tamper {
                    std::fs::write(path, b"later owner metadata").unwrap();
                    return delete();
                }
                if after {
                    delete()?;
                }
                Err("injected metadata interruption".into())
            }),
        )
    }
    fn collect(&self, collect: Collect) -> Result<(), String> {
        self.inner.collect(collect)
    }
    fn refused(&self, directory: &str, error: &str) {
        self.inner.refused(directory, error);
    }
}

#[tokio::test]
async fn native_retirement_resumes_before_and_after_deletion_without_rewriting_acknowledgement() {
    for after in [false, true] {
        let f = Fixture::new().await;
        payload_complete(&f).await;
        let cut = maintenance(
            &f.paths,
            f.config.clone(),
            Arc::new(Cut {
                inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
                after,
                tamper: None,
            }),
        );
        assert!(
            cut.retire(f.project.to_str().unwrap(), f.receipt.clone().into())
                .await
                .is_err()
        );
        assert_eq!(f.journal.exists(), !after);
        let pending = f.record().unwrap();
        assert!(pending.metadata.is_some() && !pending.retired);
        let blocked = f
            .app
            .runtime
            .snapshot_restore_acknowledge(&f.session, &f.receipt.origin.activity_id)
            .await
            .unwrap_err();
        assert!(
            blocked.to_string().contains("retirement is pending"),
            "{blocked}"
        );
        assert!(retire(&f).await.unwrap() > 0);
        assert!(f.record().unwrap().retired && !f.journal.exists());
        assert_eq!(retire(&f).await.unwrap(), 0);
        f.app.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn native_retirement_preserves_changed_or_unknown_metadata_after_planning() {
    for leaf in ["intent.next", "owner-kept"] {
        let f = Fixture::new().await;
        payload_complete(&f).await;
        let active = std::fs::read(f.journal.join("intent.json")).unwrap();
        let target = f.journal.join(leaf);
        let cut = maintenance(
            &f.paths,
            f.config.clone(),
            Arc::new(Cut {
                inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
                after: false,
                tamper: Some(target.clone()),
            }),
        );
        assert!(
            cut.retire(f.project.to_str().unwrap(), f.receipt.clone().into())
                .await
                .is_err()
        );
        assert!(f.record().unwrap().metadata.is_some() && !f.record().unwrap().retired);
        assert_eq!(
            std::fs::read(f.journal.join("intent.json")).unwrap(),
            active
        );
        assert_eq!(std::fs::read(target).unwrap(), b"later owner metadata");
        assert!(worker_routes(&f).iter().all(|path| path.exists()));
        f.app.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn native_retirement_policy_loss_after_one_unlink_preserves_the_remaining_metadata() {
    let f = Fixture::new().await;
    payload_complete(&f).await;
    let active = f.journal.join("intent.json");
    let cut = GitSnapshots::with_reclamation(
        f.paths.data.clone(),
        Arc::new(move |_| {
            if active.exists() {
                json!({})
            } else {
                Value::Null
            }
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
    assert!(
        cut.retire(f.project.to_str().unwrap(), f.receipt.clone().into())
            .await
            .unwrap_err()
            .contains("retention")
    );
    assert!(!f.journal.join("intent.json").exists() && f.journal.join("intent.next").exists());
    assert!(worker_routes(&f).iter().all(|path| path.exists()));
    assert!(!f.record().unwrap().retired);
    assert!(retire(&f).await.unwrap() > 0);
    assert!(f.record().unwrap().retired && !f.journal.exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn native_metadata_retirement_frees_cap_space_without_source_or_inference_replay() {
    let f = Fixture::new().await;
    payload_complete(&f).await;
    let namespace = f.journal.parent().unwrap().parent().unwrap();
    let cap = private_bytes(namespace) + 4096;
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":7,"fallback_max_bytes":cap}});
    let inference = f.model.requests().len();
    std::fs::write(f.project.join("zz_new"), vec![83u8; 8192]).unwrap();
    let captured = f
        .snapshots
        .track(f.project.to_str().unwrap())
        .await
        .unwrap();
    assert!(captured.is_some());
    assert!(f.record().unwrap().retired && !f.journal.exists());
    assert_eq!(
        std::fs::read(f.project.join("file_0")).unwrap(),
        "before\n".repeat(4096).as_bytes()
    );
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn native_retirement_refuses_unknown_replaced_aliased_and_live_metadata_before_planning() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["unknown", "replacement", "symlink", "hardlink", "live"] {
        let f = Fixture::new().await;
        payload_complete(&f).await;
        let spare = f.journal.join("intent.next");
        let retained = f.paths.tmp.join("owner-metadata");
        let mut held = None;
        match mode {
            "unknown" => std::fs::write(f.journal.join("owner-kept"), b"owner bytes").unwrap(),
            "replacement" => {
                let bytes = std::fs::read(&spare).unwrap();
                std::fs::rename(&spare, &retained).unwrap();
                std::fs::write(&spare, bytes).unwrap();
                std::fs::set_permissions(&spare, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            "symlink" => {
                std::fs::rename(&spare, &retained).unwrap();
                std::os::unix::fs::symlink(&retained, &spare).unwrap();
            }
            "hardlink" => std::fs::hard_link(&spare, &retained).unwrap(),
            "live" => {
                let file = std::fs::File::open(&worker_routes(&f)[0]).unwrap();
                file.try_lock().unwrap();
                held = Some(file);
            }
            _ => unreachable!(),
        }
        let intent = std::fs::read(f.journal.join("intent.json")).unwrap();
        assert!(retire(&f).await.is_err(), "{mode}");
        assert!(f.record().unwrap().metadata.is_none(), "{mode}");
        assert_eq!(
            std::fs::read(f.journal.join("intent.json")).unwrap(),
            intent
        );
        assert!(worker_routes(&f).iter().all(|path| path.exists()));
        drop(held);
        f.app.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn native_retirement_respects_retention_and_reader_leases_before_planning() {
    let f = Fixture::new().await;
    payload_complete(&f).await;
    for settings in [
        Value::Null,
        json!({"snapshots":false}),
        json!({"snapshots":{"retention_days":0}}),
    ] {
        *f.config.lock().unwrap() = settings;
        assert_eq!(retire(&f).await.unwrap(), 0);
        assert!(f.record().unwrap().metadata.is_none() && f.journal.exists());
    }
    *f.config.lock().unwrap() = json!({});
    let reader = cyber_server::workspace::mutation_lock(&f.project)
        .unwrap()
        .lock_owned()
        .await;
    assert!(retire(&f).await.unwrap_err().contains("reader or writer"));
    assert!(f.record().unwrap().metadata.is_none());
    drop(reader);
    assert!(retire(&f).await.unwrap() > 0);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn native_metadata_retirement_child() {
    let Ok(control) = std::env::var("CYBER_NATIVE_METADATA_RETIREMENT_CHILD") else {
        return;
    };
    let control = PathBuf::from(control);
    let f = Fixture::in_directory(tempfile::tempdir_in(&control).unwrap()).await;
    payload_complete(&f).await;
    std::fs::write(control.join("fixture.json"), json!({"root":f.paths.data.parent().unwrap(),"project":f.project,"journal":f.journal,"receipt":f.receipt}).to_string()).unwrap();
    let active = f.journal.join("intent.json");
    let marker = control.join("ready");
    let snapshots = GitSnapshots::with_reclamation(
        f.paths.data.clone(),
        Arc::new(move |_| {
            if !active.exists() {
                std::fs::write(
                    &marker,
                    b"active intent removed; remaining metadata retained",
                )
                .unwrap();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            json!({})
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
    snapshots
        .retire(f.project.to_str().unwrap(), f.receipt.clone().into())
        .await
        .unwrap();
    panic!("worker passed the metadata cut");
}

#[tokio::test]
async fn native_retirement_after_real_process_death_reopens_the_exact_plan_without_source_replay() {
    use std::os::unix::process::ExitStatusExt;
    let mut control = tempfile::tempdir().unwrap();
    let log = std::fs::File::create(control.path().join("worker.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_retirement::native_metadata_retirement_child",
            "--nocapture",
        ])
        .env("CYBER_NATIVE_METADATA_RETIREMENT_CHILD", control.path())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let ready = loop {
        if control.path().join("ready").exists() {
            break true;
        }
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let _ = child.kill();
    let status = child.wait().unwrap();
    if !ready {
        control.disable_cleanup(true);
    }
    assert!(
        ready,
        "worker missed native metadata cut; fixture retained at {}: {}",
        control.path().display(),
        std::fs::read_to_string(control.path().join("worker.log")).unwrap()
    );
    assert_eq!(status.signal(), Some(9));
    resume_child_fixture(control.path()).await;
}
async fn resume_child_fixture(control: &std::path::Path) {
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(control.join("fixture.json")).unwrap()).unwrap();
    let root = PathBuf::from(fixture["root"].as_str().unwrap());
    let project = PathBuf::from(fixture["project"].as_str().unwrap());
    let journal = PathBuf::from(fixture["journal"].as_str().unwrap());
    let receipt: SnapshotRestoreReceipt =
        serde_json::from_value(fixture["receipt"].clone()).unwrap();
    assert!(!journal.join("intent.json").exists() && journal.join("intent.next").exists());
    let target = root.join("later-owner");
    std::fs::write(&target, b"owner after process death").unwrap();
    std::fs::remove_file(project.join("file_0")).unwrap();
    std::os::unix::fs::symlink(&target, project.join("file_0")).unwrap();
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    let store = Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            DatabaseLocation::File(paths.data.join("cyber.db")),
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    );
    let policy = cyber_app::snapshot_reclamation::policy(store);
    let retained: records::Receipt = receipt.into();
    let before = policy.record(&retained).unwrap().unwrap();
    assert!(before.metadata.is_some() && !before.retired);
    let snapshots = maintenance(&paths, Arc::new(Mutex::new(json!({}))), policy.clone());
    // The missing active intent cannot be discovered from journal JSON. The
    // production GC resumes it from the Store's bounded retirement page.
    snapshots.gc().await;
    assert!(policy.record(&retained).unwrap().unwrap().retired && !journal.exists());
    assert_eq!(
        snapshots
            .retire(project.to_str().unwrap(), retained)
            .await
            .unwrap(),
        0
    );
    assert_eq!(std::fs::read_link(project.join("file_0")).unwrap(), target);
    assert_eq!(std::fs::read(target).unwrap(), b"owner after process death");
}
