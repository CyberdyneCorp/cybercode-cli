use super::*;
use cyber_computer::{Error, ImagePoint, backend::Capture, control::InputPermit};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Fake {
    target: Mutex<NativeTarget>,
    inputs: AtomicUsize,
    captures: AtomicUsize,
    fail: AtomicBool,
    wrong_capture: AtomicBool,
    screen_consent: AtomicBool,
    input_consent: AtomicBool,
    host_lease: Arc<AtomicBool>,
    block_input: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
    block_resolve: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            target: Mutex::new(NativeTarget {
                identity: "app:test.fixture:42:100:7".into(),
                app: Some("test.fixture".into()),
                pid: 42,
                window: 7,
                display: 1,
                launch_ms: 100,
                x: -100.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
                image_width: 100,
                image_height: 100,
                rotation: 0,
            }),
            inputs: AtomicUsize::new(0),
            captures: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            wrong_capture: AtomicBool::new(false),
            screen_consent: AtomicBool::new(true),
            input_consent: AtomicBool::new(true),
            host_lease: Arc::new(AtomicBool::new(false)),
            block_input: Mutex::new(None),
            block_resolve: Mutex::new(None),
        }
    }
}
struct FakeLease(Arc<AtomicBool>);
impl Drop for FakeLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl Backend for Fake {
    fn acquire_lease(&self) -> std::result::Result<cyber_computer::backend::HostLease, Error> {
        if self.host_lease.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy);
        }
        Ok(Arc::new(FakeLease(Arc::clone(&self.host_lease))))
    }

    fn health(&self, _prompt: bool) -> std::result::Result<Health, Error> {
        Ok(Health {
            supported: true,
            platform: "fixture".into(),
            minimum_version: "fixture".into(),
            screen_capture: self.screen_consent.load(Ordering::SeqCst),
            accessibility: self.input_consent.load(Ordering::SeqCst),
            binary: "fixture".into(),
        })
    }
    fn resolve(&self, _: &Selector) -> std::result::Result<NativeTarget, Error> {
        let block = self.block_resolve.lock().unwrap().take();
        if let Some((ready, release)) = block {
            ready.wait();
            release.wait();
        }
        Ok(self.target.lock().unwrap().clone())
    }
    fn capture(&self, target: &NativeTarget) -> std::result::Result<Capture, Error> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        let mut target = target.clone();
        if self.wrong_capture.load(Ordering::SeqCst) {
            target.window += 1;
        }
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(100u32.to_be_bytes());
        png.extend(100u32.to_be_bytes());
        Ok(Capture { target, png })
    }
    fn input(&self, _: &NativeTarget, permit: &InputPermit) -> std::result::Result<(), Error> {
        assert!(!permit.cancelled());
        self.inputs.fetch_add(1, Ordering::SeqCst);
        let block = self.block_input.lock().unwrap().take();
        if let Some((ready, release)) = block {
            ready.wait();
            release.wait();
        }
        if permit.cancelled() {
            return Err(Error::NotArmed);
        }
        if self.fail.load(Ordering::SeqCst) {
            Err(Error::Native("fixture failure"))
        } else {
            Ok(())
        }
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    computer: Computer,
    backend: Arc<Fake>,
    store: Arc<Store>,
    enabled: Arc<AtomicBool>,
    present: Arc<AtomicBool>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(
            Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::Memory,
                crate::runtime::Runtime::registry(),
            ))
            .unwrap(),
        );
        let backend = Arc::new(Fake::default());
        let enabled = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&enabled);
        let present = Arc::new(AtomicBool::new(true));
        let attendance = Arc::clone(&present);
        let computer = Computer::new(
            Arc::clone(&store),
            root.path().join("computer"),
            Arc::new(move || ComputerPolicy {
                enabled: flag.load(Ordering::SeqCst),
                retention_days: 1,
            }),
            Arc::new(move |_| attendance.load(Ordering::SeqCst)),
            backend.clone(),
        )
        .unwrap();
        Self {
            _root: root,
            computer,
            backend,
            store,
            enabled,
            present,
        }
    }
    fn arm(&self) {
        let preview = self
            .computer
            .preview(
                "owner",
                Selector {
                    app: Some("test.fixture".into()),
                    display: None,
                },
            )
            .unwrap();
        self.computer
            .allow("owner", &preview.id, None, false)
            .unwrap();
    }
    fn frame(&self) -> FrameInfo {
        self.computer
            .screenshot("owner", "capture-1")
            .unwrap()
            .frame
            .unwrap()
    }
}
fn motion() -> Action {
    Action::Move {
        point: ImagePoint { x: 20.0, y: 30.0 },
    }
}

#[test]
fn capture_and_input_are_idempotent_and_session_bound() {
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    assert!(f.computer.frame_bytes("other", &frame.id).is_err());
    assert_eq!(
        f.computer.frame_bytes("owner", &frame.id).unwrap().len(),
        24
    );
    let repeated = f.computer.screenshot("owner", "capture-1").unwrap();
    assert!(repeated.replayed);
    assert_eq!(f.backend.captures.load(Ordering::SeqCst), 1);
    let receipt = f
        .computer
        .input("owner", "input-1", &frame.id, motion(), true)
        .unwrap();
    assert_eq!(receipt.outcome, "posted");
    assert!(
        f.computer
            .input("owner", "input-1", &frame.id, motion(), true)
            .unwrap()
            .replayed
    );
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 1);
    assert!(
        f.computer
            .input("other", "input-1", &frame.id, motion(), true)
            .is_err()
    );
    assert!(
        f.computer
            .input(
                "owner",
                "input-1",
                &frame.id,
                Action::Wait { milliseconds: 1 },
                true
            )
            .is_err()
    );
}
#[test]
fn plan_permission_feature_presence_and_stale_target_deny_before_native_input() {
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    assert!(
        f.computer
            .input("owner", "plan", &frame.id, motion(), false)
            .is_err()
    );
    f.present.store(false, Ordering::SeqCst);
    assert!(
        f.computer
            .input("owner", "absent", &frame.id, motion(), true)
            .is_err()
    );
    f.present.store(true, Ordering::SeqCst);
    f.enabled.store(false, Ordering::SeqCst);
    assert!(
        f.computer
            .input("owner", "disabled", &frame.id, motion(), true)
            .is_err()
    );
    f.enabled.store(true, Ordering::SeqCst);
    f.arm();
    let frame = f
        .computer
        .screenshot("owner", "capture-2")
        .unwrap()
        .frame
        .unwrap();
    f.backend.target.lock().unwrap().window += 1;
    assert!(
        f.computer
            .input("owner", "changed", &frame.id, motion(), true)
            .is_err()
    );
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 0);
}
#[test]
fn native_failure_stops_and_requires_explicit_review_before_rearming() {
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    f.backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        f.computer
            .input("owner", "failed", &frame.id, motion(), true)
            .unwrap()
            .outcome,
        "unknown"
    );
    assert!(f.computer.status("owner").unwrap().grant.is_none());
    let preview = f
        .computer
        .preview("owner", frame.target.selector())
        .unwrap();
    assert_eq!(preview.unknown_operations.len(), 1);
    assert!(f.computer.allow("owner", &preview.id, None, false).is_err());
    let preview = f
        .computer
        .preview("owner", frame.target.selector())
        .unwrap();
    f.computer.allow("owner", &preview.id, None, true).unwrap();
    assert!(
        f.computer
            .input("owner", "failed", &frame.id, motion(), true)
            .unwrap()
            .replayed
    );
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 1);
}
#[test]
fn recovery_does_not_replay_intents_or_restore_grants() {
    let f = Fixture::new();
    f.arm();
    f.computer
        .journal
        .begin(
            "owner",
            "crashed",
            "type",
            "digest",
            json!({"text_bytes":4}),
            0,
        )
        .unwrap();
    f.computer.journal.recover().unwrap();
    let receipt = f
        .computer
        .journal
        .replay("owner", "crashed", "digest")
        .unwrap()
        .unwrap();
    assert_eq!(receipt.outcome, "unknown");
    assert!(receipt.replayed);
    assert_eq!(f.computer.journal.unknown("owner").unwrap().len(), 1);
    let unknown = f.computer.journal.unknown_all().unwrap();
    f.computer.journal.acknowledge(&unknown).unwrap();
    assert_eq!(
        f.computer
            .journal
            .replay("owner", "crashed", "digest")
            .unwrap()
            .unwrap()
            .outcome,
        "unknown",
        "acknowledgement is not evidence of a native outcome"
    );
    let restarted = Computer::new(
        Arc::clone(&f.store),
        f._root.path().join("computer"),
        Arc::new(|| ComputerPolicy {
            enabled: true,
            retention_days: 1,
        }),
        Arc::new(|_| true),
        f.backend.clone(),
    )
    .unwrap();
    assert!(restarted.status("owner").unwrap().grant.is_none());
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 0);
}
#[test]
fn typed_text_never_enters_native_journal() {
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    f.computer
        .input(
            "owner",
            "private-text",
            &frame.id,
            Action::Type {
                text: "sentinel-private-content".into(),
            },
            true,
        )
        .unwrap();
    let rows=f.store.read(|db| db.query_row("SELECT audit||COALESCE(receipt,'') FROM computer_operation WHERE operation_key='private-text'",[],|r|r.get::<_,String>(0)).map_err(Into::into)).unwrap();
    assert!(!rows.contains("sentinel-private-content"));
    assert!(rows.contains("text_sha256"));
    assert!(rows.contains("text_bytes"));
    assert!(rows.contains(&frame.grant_id));
    assert!(
        rows.contains(&frame.target.identity),
        "native evidence retains its target binding after private frame expiry"
    );
}
#[test]
fn wrong_native_capture_and_artifact_failure_are_settled_without_a_frame() {
    let f = Fixture::new();
    f.arm();
    f.backend.wrong_capture.store(true, Ordering::SeqCst);
    let receipt = f.computer.screenshot("owner", "wrong").unwrap();
    assert_eq!(receipt.outcome, "unknown");
    assert!(receipt.frame.is_none());
    assert!(f.computer.screenshot("owner", "wrong").unwrap().replayed);
    f.backend.wrong_capture.store(false, Ordering::SeqCst);
    std::fs::write(&f.computer.artifacts, b"not a directory").unwrap();
    let receipt = f.computer.screenshot("owner", "disk").unwrap();
    assert_eq!(receipt.outcome, "unknown");
    assert!(receipt.frame.is_none());
    assert!(f.computer.screenshot("owner", "disk").unwrap().replayed);
}
#[test]
fn stopping_invalidates_pending_review_and_private_frames_expire() {
    let f = Fixture::new();
    let preview = f
        .computer
        .preview(
            "owner",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    f.computer.stop("owner");
    assert!(f.computer.allow("owner", &preview.id, None, false).is_err());
    f.arm();
    let frame = f.frame();
    let id = frame.id.clone();
    f.store
        .transaction(move |tx| {
            tx.execute("UPDATE computer_frame SET created_ms=0 WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        f.computer.frame_bytes("owner", &frame.id),
        Err(ComputerError::FrameExpired)
    ));
    f.computer.prune().unwrap();
    assert!(
        !f.computer
            .artifact_path("owner", &frame.id)
            .unwrap()
            .exists()
    );
}

#[test]
fn stop_fences_an_owner_allow_already_resolving_its_target() {
    let fixture = Fixture::new();
    let preview = fixture
        .computer
        .preview(
            "owner",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    let computer = Arc::new(fixture.computer);
    let ready = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    *fixture.backend.block_resolve.lock().unwrap() = Some((ready.clone(), release.clone()));
    let worker = Arc::clone(&computer);
    let task = std::thread::spawn(move || worker.allow("owner", &preview.id, None, false));
    ready.wait();
    computer.stop_all();
    release.wait();
    assert!(task.join().unwrap().is_err());
    assert!(computer.status("owner").unwrap().grant.is_none());
}

#[test]
fn presence_loss_fences_an_owner_allow_already_resolving_its_target() {
    let mut fixture = Fixture::new();
    let epoch = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let observed = epoch.clone();
    fixture
        .computer
        .set_presence_epoch(Arc::new(move |_| observed.load(Ordering::Acquire)));
    let preview = fixture
        .computer
        .preview(
            "owner",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    let computer = Arc::new(fixture.computer);
    let ready = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    *fixture.backend.block_resolve.lock().unwrap() = Some((ready.clone(), release.clone()));
    let worker = Arc::clone(&computer);
    let task = std::thread::spawn(move || worker.allow("owner", &preview.id, None, false));
    ready.wait();
    epoch.fetch_add(1, Ordering::AcqRel); // drop last attachment then reconnect while resolve is blocked
    release.wait();
    assert!(task.join().unwrap().is_err());
    assert!(computer.status("owner").unwrap().grant.is_none());
}

#[test]
fn native_host_lease_excludes_a_second_runtime_until_explicit_stop() {
    let fixture = Fixture::new();
    fixture.arm();
    let other = Computer::new(
        Arc::clone(&fixture.store),
        fixture._root.path().join("second"),
        Arc::new(|| ComputerPolicy {
            enabled: true,
            retention_days: 1,
        }),
        Arc::new(|_| true),
        fixture.backend.clone(),
    )
    .unwrap();
    let preview = other
        .preview(
            "other",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    assert!(other.allow("other", &preview.id, None, false).is_err());
    fixture.computer.stop_all();
    let preview = other
        .preview(
            "other",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    other.allow("other", &preview.id, None, false).unwrap();
    assert!(other.status("other").unwrap().grant.is_some());
}
#[test]
fn incomplete_capture_artifacts_never_become_readable_frame_authority() {
    let fixture = Fixture::new();
    fixture.arm();
    let frame = fixture.frame();
    assert_eq!(
        fixture
            .computer
            .status("owner")
            .unwrap()
            .latest_frame
            .unwrap()
            .id,
        frame.id
    );
    let id = frame.id.clone();
    fixture
        .store
        .transaction(move |tx| {
            tx.execute("UPDATE computer_frame SET ready=0 WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        fixture.computer.frame_bytes("owner", &frame.id),
        Err(ComputerError::FrameExpired)
    ));
    assert!(
        fixture
            .computer
            .status("owner")
            .unwrap()
            .latest_frame
            .is_none()
    );
    assert!(
        fixture
            .computer
            .input("owner", "incomplete", &frame.id, motion(), true)
            .is_err()
    );
    assert_eq!(fixture.backend.inputs.load(Ordering::SeqCst), 0);
}

#[test]
fn stopped_workers_keep_the_host_lease_until_native_exit_and_cannot_hide_unknowns_by_switching_session()
 {
    let fixture = Fixture::new();
    fixture.arm();
    let frame = fixture.frame();
    let computer = Arc::new(fixture.computer);
    let ready = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    *fixture.backend.block_input.lock().unwrap() = Some((ready.clone(), release.clone()));
    let native = Arc::clone(&computer);
    let worker =
        std::thread::spawn(move || native.input("owner", "busy-native", &frame.id, motion(), true));
    ready.wait();
    computer.stop_all();
    let other = Computer::new(
        Arc::clone(&fixture.store),
        fixture._root.path().join("other"),
        Arc::new(|| ComputerPolicy {
            enabled: true,
            retention_days: 1,
        }),
        Arc::new(|_| true),
        fixture.backend.clone(),
    )
    .unwrap();
    let preview = other
        .preview(
            "other",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    assert!(
        other.allow("other", &preview.id, None, true).is_err(),
        "Stop cannot transfer input while a native worker lives"
    );
    release.wait();
    assert!(
        worker.join().unwrap().is_err(),
        "a new runtime recovered the in-flight intent conservatively"
    );
    let preview = other
        .preview(
            "other",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    assert_eq!(preview.unknown_operations[0].session_id, "owner");
    assert!(other.allow("other", &preview.id, None, false).is_err());
    let preview = other
        .preview(
            "other",
            Selector {
                app: Some("test.fixture".into()),
                display: None,
            },
        )
        .unwrap();
    other.allow("other", &preview.id, None, true).unwrap();
}

#[test]
fn failed_private_cleanup_keeps_bytes_reserved_until_deletion_succeeds() {
    let fixture = Fixture::new();
    fixture.arm();
    let receipt = fixture
        .computer
        .screenshot("owner", "cleanup-frame")
        .unwrap();
    let frame = receipt.frame.unwrap();
    let path = fixture.computer.artifact_path("owner", &frame.id).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap(); // deterministic remove_file failure
    let id = frame.id.clone();
    fixture
        .store
        .transaction(move |tx| {
            tx.execute(
                "UPDATE computer_frame SET expired=1,bytes=268435456 WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    fixture.computer.prune().unwrap();
    assert_eq!(fixture.computer.journal.stored_bytes().unwrap(), 268435456);
    assert!(fixture.computer.screenshot("owner", "over-quota").is_err());
    std::fs::remove_dir(&path).unwrap();
    fixture.computer.prune().unwrap();
    assert_eq!(fixture.computer.journal.stored_bytes().unwrap(), 0);
}

#[test]
fn observed_os_consent_revocation_cannot_rearm_after_consent_is_restored() {
    for screen in [false, true] {
        let fixture = Fixture::new();
        fixture.arm();
        assert!(fixture.computer.health(false).unwrap().grant.is_some());
        let consent = if screen {
            &fixture.backend.screen_consent
        } else {
            &fixture.backend.input_consent
        };
        consent.store(false, Ordering::SeqCst);
        assert!(fixture.computer.health(false).unwrap().grant.is_none());
        consent.store(true, Ordering::SeqCst);
        assert!(fixture.computer.status("owner").unwrap().grant.is_none());
        fixture.arm();
        assert!(fixture.computer.status("owner").unwrap().grant.is_some());
    }
}

#[test]
fn undeletable_cleanup_batch_does_not_starve_later_expired_files() {
    let fixture = Fixture::new();
    let directory = fixture.computer.artifacts.join(digest(b"owner"));
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..=500 {
        let path = directory.join(format!("cuf_{index:026}.png"));
        if index < 500 {
            std::fs::create_dir(path).unwrap();
        } else {
            std::fs::write(path, b"fixture").unwrap();
        }
    }
    fixture.store.transaction(|tx|{for index in 0..=500 {tx.execute("INSERT INTO computer_frame (id,session_id,sha256,metadata,native_target,bytes,created_ms,expired) VALUES (?1,'owner','fixture','{}','{}',7,0,1)",[format!("cuf_{index:026}")])?;}Ok(())}).unwrap();
    fixture.computer.prune().unwrap();
    assert!(!directory.join(format!("cuf_{:026}.png", 500)).exists());
    assert_eq!(fixture.computer.journal.stored_bytes().unwrap(), 3500);
}

#[test]
fn deleting_capture_owner_expires_pixels_even_without_retention_and_keeps_unknown_safety_evidence()
{
    let mut fixture = Fixture::new();
    fixture.computer.policy = Arc::new(|| ComputerPolicy {
        enabled: true,
        retention_days: 0,
    });
    fixture.arm();
    let frame = fixture
        .computer
        .screenshot("owner", "delete-frame")
        .unwrap()
        .frame
        .unwrap();
    let path = fixture.computer.artifact_path("owner", &frame.id).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    fixture
        .computer
        .journal
        .begin(
            "owner",
            "unsettled-delete",
            "type",
            "hash",
            json!({"action":"type","sha256":"hash"}),
            wall_ms(),
        )
        .unwrap();
    fixture
        .store
        .transaction(|tx| {
            mark_session_deleted(tx, "owner")?;
            Ok(())
        })
        .unwrap();
    assert!(fixture.computer.frame_bytes("owner", &frame.id).is_err());
    fixture.computer.prune().unwrap();
    assert!(
        fixture.computer.journal.stored_bytes().unwrap() > 0,
        "failed cleanup stays accounted"
    );
    let mut late = frame.clone();
    late.id = cyber_core::ids::new_id("cuf");
    assert!(
        fixture
            .computer
            .journal
            .frame(&late, &late.target, None)
            .is_err(),
        "a late writer cannot reserve an artifact for a deleted Session"
    );
    assert!(
        fixture.computer.journal.ready_frame(&frame.id).is_err(),
        "late readiness cannot resurrect expired pixels"
    );
    fixture
        .store
        .transaction(|tx| {
            tx.execute(
                "INSERT INTO computer_deleted_session(session_id,deleted_ms) VALUES('unrelated',0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    std::fs::remove_dir(&path).unwrap();
    fixture.computer.prune().unwrap();
    assert_eq!(fixture.computer.journal.stored_bytes().unwrap(), 0);
    let preserved=fixture.store.read(|db|db.query_row("SELECT COUNT(*) FROM computer_operation WHERE operation_key='unsettled-delete' AND status='intent'",[],|row|row.get::<_,i64>(0)).map_err(Into::into)).unwrap();
    assert_eq!(preserved, 1);
}
