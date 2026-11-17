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
    resolves: AtomicUsize,
    reviews: AtomicUsize,
    lease_acquires: AtomicUsize,
    health_override: Mutex<Option<Health>>,
    partial: AtomicBool,
    sink: Mutex<Option<cyber_computer::backend::RevocationSink>>,
    /// Each end_scope records the identity and what the probe observed at that moment.
    end_scopes: Mutex<Vec<(String, bool)>>,
    probe: Mutex<Option<Box<dyn Fn() -> bool + Send>>>,
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
                target_id: String::new(),
                source: None,
                topology_epoch: 0,
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
            resolves: AtomicUsize::new(0),
            reviews: AtomicUsize::new(0),
            lease_acquires: AtomicUsize::new(0),
            health_override: Mutex::new(None),
            partial: AtomicBool::new(false),
            sink: Mutex::new(None),
            end_scopes: Mutex::new(Vec::new()),
            probe: Mutex::new(None),
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
        self.lease_acquires.fetch_add(1, Ordering::SeqCst);
        if self.host_lease.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy);
        }
        Ok(Arc::new(FakeLease(Arc::clone(&self.host_lease))))
    }

    fn health(&self, _prompt: bool) -> std::result::Result<Health, Error> {
        if let Some(health) = self.health_override.lock().unwrap().clone() {
            return Ok(health);
        }
        Ok(Health {
            supported: true,
            platform: "macos".into(),
            minimum_version: "fixture".into(),
            screen_capture: self.screen_consent.load(Ordering::SeqCst),
            accessibility: self.input_consent.load(Ordering::SeqCst),
            binary: "fixture".into(),
            ..Health::default()
        })
    }
    fn resolve(&self, _: &Selector) -> std::result::Result<NativeTarget, Error> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
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
        if self.partial.load(Ordering::SeqCst) {
            return Err(Error::PartialInput {
                requested: 2,
                posted: 1,
            });
        }
        if self.fail.load(Ordering::SeqCst) {
            Err(Error::Native("fixture failure"))
        } else {
            Ok(())
        }
    }
    fn review(
        &self,
        selector: &Selector,
        _: &(dyn Fn() -> bool + Sync),
    ) -> std::result::Result<NativeTarget, Error> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        self.resolve(selector)
    }
    fn end_scope(&self, target: &NativeTarget) {
        let observed = self
            .probe
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|probe| probe());
        self.end_scopes
            .lock()
            .unwrap()
            .push((target.identity.clone(), observed));
    }
    fn set_revocation_sink(&self, sink: cyber_computer::backend::RevocationSink) {
        *self.sink.lock().unwrap() = Some(sink);
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
                    ..Selector::default()
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
    std::fs::remove_dir(&f.computer.artifacts).unwrap();
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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
                ..Selector::default()
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

#[test]
fn private_capture_storage_is_prepared_and_refuses_non_directory_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("computer");
    prepare_artifact_root(&root).unwrap();
    assert!(root.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert!(prepare_artifact_root(&alias).is_err());
    }
    let file = temporary.path().join("file");
    std::fs::write(&file, "owner fixture").unwrap();
    assert!(prepare_artifact_root(&file).is_err());
}

// ---- Platform-neutral foundation (add-computer-use-platform-foundation) ----

fn key(modifiers: &[cyber_computer::Modifier], key: &str) -> Action {
    Action::Key {
        chord: cyber_computer::KeyChord {
            modifiers: modifiers.to_vec(),
            key: key.into(),
        },
    }
}
fn journal_rows(store: &Store, operation: &str) -> i64 {
    let operation = operation.to_owned();
    store
        .read(move |db| {
            db.query_row(
                "SELECT COUNT(*) FROM computer_operation WHERE operation_key=?1",
                [operation],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .unwrap()
}
fn later_platform_health(evidence: Evidence, selectors: Vec<SelectorKind>) -> Health {
    use cyber_computer::capability::{Assurance, Capability, InputCapability};
    Health {
        supported: true,
        platform: "linux".into(),
        minimum_version: "fixture desktop".into(),
        binary: "fixture".into(),
        schema_version: SCHEMA_VERSION,
        backend: "fixture-portal".into(),
        capture: Capability {
            available: true,
            reasons: Vec::new(),
        },
        input: InputCapability {
            available: true,
            reasons: Vec::new(),
            assurance: Assurance::Verified,
        },
        selectors,
        evidence,
        ..Health::default()
    }
}
use cyber_computer::capability::SelectorKind;
use cyber_computer::{
    Modifier,
    backend::{PickKind, SourceKind, TargetSource},
};

#[test]
fn blocked_chord_is_refused_before_intent_with_the_grant_left_armed() {
    // F8c: before this change a chord the macOS adapter refuses was journaled,
    // failed natively, settled unknown and stopped the grant.
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    let (resolves, captures) = (
        f.backend.resolves.load(Ordering::SeqCst),
        f.backend.captures.load(Ordering::SeqCst),
    );
    for (operation, chord) in [
        ("copy", key(&[Modifier::Command], "c")),
        ("paste", key(&[Modifier::Command], "v")),
        ("switch", key(&[Modifier::Control], "a")),
        ("quit", key(&[Modifier::Command], "q")),
    ] {
        let error = f
            .computer
            .input("owner", operation, &frame.id, chord, true)
            .expect_err("refused");
        assert_eq!(
            error.reason(),
            Some(ReasonCode::KeyChordRefused),
            "{operation}"
        );
        assert_eq!(
            journal_rows(&f.store, operation),
            0,
            "no intent for {operation}"
        );
    }
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 0);
    assert_eq!(f.backend.resolves.load(Ordering::SeqCst), resolves);
    assert_eq!(f.backend.captures.load(Ordering::SeqCst), captures);
    assert!(f.computer.status("owner").unwrap().grant.is_some());
    assert!(f.computer.journal.unknown("owner").unwrap().is_empty());
    let undo = f
        .computer
        .input(
            "owner",
            "undo",
            &frame.id,
            key(&[Modifier::Command], "z"),
            true,
        )
        .unwrap();
    assert_eq!(undo.outcome, "posted");
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 1);
}

#[test]
fn display_grants_keep_the_unrestricted_macos_chord_rules() {
    let f = Fixture::new();
    {
        let mut target = f.backend.target.lock().unwrap();
        target.app = None;
        target.identity = "display:1".into();
    }
    let preview = f.computer.preview("owner", Selector::display(1)).unwrap();
    f.computer.allow("owner", &preview.id, None, false).unwrap();
    let frame = f.frame();
    let receipt = f
        .computer
        .input(
            "owner",
            "tab",
            &frame.id,
            key(&[Modifier::Command], "tab"),
            true,
        )
        .unwrap();
    assert_eq!(receipt.outcome, "posted");
}

#[test]
fn input_on_a_capture_only_host_is_refused_by_code_without_journal_or_native_calls() {
    let f = Fixture::new();
    *f.backend.health_override.lock().unwrap() = Some(later_platform_health(
        Evidence::FixtureTested,
        vec![SelectorKind::App],
    ));
    let health = f.computer.health(false).unwrap().health;
    assert_eq!(health.mode, cyber_computer::capability::Mode::CaptureOnly);
    assert_eq!(health.input.reasons, vec![ReasonCode::EvidencePending]);
    f.arm();
    let frame = f.frame();
    let resolves = f.backend.resolves.load(Ordering::SeqCst);
    let error = f
        .computer
        .input("owner", "click", &frame.id, motion(), true)
        .err()
        .unwrap();
    assert_eq!(error.reason(), Some(ReasonCode::EvidencePending));
    assert_eq!(journal_rows(&f.store, "click"), 0);
    assert_eq!(f.backend.resolves.load(Ordering::SeqCst), resolves);
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 0);
    assert!(
        f.computer.status("owner").unwrap().grant.is_some(),
        "capture grant stays armed"
    );
}

#[test]
fn topology_change_stales_frames_without_revoking_and_binding_loss_fences_before_teardown() {
    let f = Fixture::new();
    let backend = Arc::clone(&f.backend);
    let store = Arc::clone(&f.store);
    let computer = Arc::new(f.computer);
    computer.install_revocation_sink();
    let weak = Arc::downgrade(&computer);
    *backend.probe.lock().unwrap() = Some(Box::new(move || {
        weak.upgrade()
            .is_some_and(|computer| computer.controller.has_authority(computer.now()))
    }));
    let preview = computer
        .preview("owner", Selector::app("test.fixture"))
        .unwrap();
    computer.allow("owner", &preview.id, None, false).unwrap();
    let old = computer
        .screenshot("owner", "before")
        .unwrap()
        .frame
        .unwrap();
    let sink = backend.sink.lock().unwrap().clone().unwrap();
    sink(Revocation {
        class: EventClass::TopologyChange,
        reason: ReasonCode::TopologyChanged,
        identity: None,
    });
    let error = computer
        .input("owner", "stale", &old.id, motion(), true)
        .err()
        .unwrap();
    assert_eq!(error.reason(), Some(ReasonCode::FrameStale));
    assert_eq!(journal_rows(&store, "stale"), 0);
    assert!(computer.status("owner").unwrap().grant.is_some());
    let fresh = computer
        .screenshot("owner", "after")
        .unwrap()
        .frame
        .unwrap();
    assert_eq!(fresh.target.topology_epoch, 1);
    assert_eq!(
        computer
            .input("owner", "fresh", &fresh.id, motion(), true)
            .unwrap()
            .outcome,
        "posted"
    );
    sink(Revocation {
        class: EventClass::BindingLoss,
        reason: ReasonCode::TargetGone,
        identity: Some("app:another".into()),
    });
    assert!(
        computer.status("owner").unwrap().grant.is_some(),
        "an unrelated binding does not revoke"
    );
    sink(Revocation::unclassified(ReasonCode::SessionLocked));
    assert!(computer.status("owner").unwrap().grant.is_none());
    let scopes = backend.end_scopes.lock().unwrap().clone();
    assert_eq!(
        scopes,
        vec![(preview.target.identity.clone(), false)],
        "teardown runs once, after the grant is fenced"
    );
    assert!(
        computer
            .input("owner", "after-lock", &fresh.id, motion(), true)
            .is_err()
    );
}

#[test]
fn health_and_status_never_acquire_the_host_lease() {
    let f = Fixture::new();
    f.backend.host_lease.store(true, Ordering::SeqCst); // another home holds it
    for _ in 0..3 {
        f.computer.health(false).unwrap();
        f.computer.status("owner").unwrap();
    }
    assert_eq!(f.backend.lease_acquires.load(Ordering::SeqCst), 0);
    let health = f.computer.health(false).unwrap().health;
    assert_eq!(health.schema_version, 2);
    assert_eq!(health.mode, cyber_computer::capability::Mode::InputEnabled);
    assert!(health.screen_capture && health.accessibility && health.supported);
    f.enabled.store(false, Ordering::SeqCst);
    let disabled = f.computer.health(false).unwrap().health;
    assert_eq!(disabled.capture.reasons, vec![ReasonCode::FeatureDisabled]);
    assert!(
        disabled.screen_capture,
        "legacy field keeps its macOS value"
    );
}

#[test]
fn unsupported_selectors_are_refused_before_any_native_call() {
    let f = Fixture::new();
    for selector in [
        Selector {
            pick: Some(PickKind::Window),
            ..Selector::default()
        },
        Selector {
            window: Some("cuc_candidate".into()),
            ..Selector::default()
        },
    ] {
        let error = f.computer.preview("owner", selector).err().unwrap();
        assert_eq!(
            error.reason(),
            Some(ReasonCode::SelectorUnsupportedOnPlatform)
        );
    }
    assert_eq!(f.backend.resolves.load(Ordering::SeqCst), 0);
    assert_eq!(f.backend.reviews.load(Ordering::SeqCst), 0);
}

#[test]
fn pick_reviews_arm_only_by_review_identifier_and_tear_down_unconfirmed_scopes() {
    let f = Fixture::new();
    *f.backend.health_override.lock().unwrap() = Some(later_platform_health(
        Evidence::FixtureTested,
        vec![SelectorKind::Pick],
    ));
    let pick = Selector {
        pick: Some(PickKind::Output),
        ..Selector::default()
    };
    let preview = f.computer.preview("owner", pick.clone()).unwrap();
    assert!(!preview.re_resolvable);
    assert_eq!(preview.evidence, Evidence::FixtureTested);
    assert_eq!(f.backend.reviews.load(Ordering::SeqCst), 1);
    assert!(f.computer.allow("other", &preview.id, None, false).is_err());
    assert!(
        f.computer.allow("owner", &preview.id, None, false).is_err(),
        "a consumed review cannot be confirmed again"
    );
    let preview = f.computer.preview("owner", pick.clone()).unwrap();
    assert!(
        f.computer
            .allow("owner", "cup_missing", None, false)
            .is_err()
    );
    f.computer.allow("owner", &preview.id, None, false).unwrap();
    assert_eq!(
        f.backend.reviews.load(Ordering::SeqCst),
        2,
        "allow never starts a review or opens a picker"
    );
    let superseded = f.computer.preview("owner", pick).unwrap();
    f.computer.stop("owner");
    assert!(
        f.computer
            .allow("owner", &superseded.id, None, false)
            .is_err()
    );
    let ended = f.backend.end_scopes.lock().unwrap().len();
    assert!(
        ended >= 3,
        "rejected, superseded and stopped scopes end: {ended}"
    );
}

#[test]
fn receipts_carry_version_two_references_but_no_owner_display_detail() {
    let f = Fixture::new();
    f.backend.target.lock().unwrap().source = Some(TargetSource {
        platform: "macos".into(),
        kind: SourceKind::App,
        backend: "macos-screencapturekit".into(),
        detail: std::collections::BTreeMap::from([(
            "title".into(),
            "Quarterly Secrets.numbers".into(),
        )]),
    });
    let preview = f
        .computer
        .preview("owner", Selector::app("test.fixture"))
        .unwrap();
    assert!(preview.target.target_id.starts_with("cut_"));
    assert_eq!(preview.schema_version, 2);
    f.computer.allow("owner", &preview.id, None, false).unwrap();
    let shot = f.computer.screenshot("owner", "shot").unwrap();
    let frame = shot.frame.clone().unwrap();
    assert_eq!(shot.schema_version, 2);
    assert_eq!(shot.platform, "macos");
    assert_eq!(shot.target_id, preview.target.target_id);
    assert_eq!(shot.frame_id.as_deref(), Some(frame.id.as_str()));
    assert_eq!(frame.schema_version, 2);
    let receipt = f
        .computer
        .input(
            "owner",
            "typed",
            &frame.id,
            Action::Type {
                text: "sentinel-secret".into(),
            },
            true,
        )
        .unwrap();
    assert_eq!(receipt.grant_id, frame.grant_id);
    assert_eq!(receipt.cleanup, Some(Cleanup::NotNeeded));
    assert!(receipt.started_ms > 0 && receipt.settled_ms >= receipt.started_ms);
    let rows = f
        .store
        .read(|db| {
            let mut query = db.prepare(
                "SELECT audit||COALESCE(receipt,'') FROM computer_operation UNION ALL SELECT metadata||native_target FROM computer_frame",
            )?;
            let rows = query
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows.join("\n"))
        })
        .unwrap();
    assert!(!rows.contains("Quarterly Secrets"));
    assert!(!rows.contains("sentinel-secret"));
    assert!(rows.contains(&preview.target.target_id));
}

#[test]
fn partial_delivery_settles_unknown_with_counts_and_uncertain_cleanup() {
    let f = Fixture::new();
    f.arm();
    let frame = f.frame();
    f.backend.partial.store(true, Ordering::SeqCst);
    let receipt = f
        .computer
        .input("owner", "partial", &frame.id, motion(), true)
        .unwrap();
    assert_eq!(receipt.outcome, "unknown");
    assert_eq!(
        receipt.events,
        Some(EventCounts {
            requested: 2,
            posted: 1
        })
    );
    assert_eq!(receipt.cleanup, Some(Cleanup::Uncertain));
    assert_eq!(receipt.reason, Some(ReasonCode::InputUnavailable));
    let replayed = f
        .computer
        .input("owner", "partial", &frame.id, motion(), true)
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(f.backend.inputs.load(Ordering::SeqCst), 1, "never repeated");
}

#[test]
fn version_one_journal_rows_replay_as_written() {
    let f = Fixture::new();
    let written =
        r#"{"operation_id":"v1-op","action":"click","outcome":"posted","replayed":false}"#;
    let shot = r#"{"operation_id":"v1-shot","action":"screenshot","outcome":"captured","replayed":false,"frame":{"id":"cuf_01J00000000000000000000000","session_id":"owner","grant_id":"cug_1","captured_at":5,"target":{"identity":"app:test.fixture:42:100:7","app":"test.fixture","pid":42,"window":7,"display":1,"launch_ms":100,"x":0.0,"y":0.0,"width":10.0,"height":10.0,"image_width":10,"image_height":10,"rotation":0},"sha256":"00","bytes":24,"created_ms":1}}"#;
    for (key, receipt) in [("v1-op", written), ("v1-shot", shot)] {
        let (key, receipt) = (key.to_owned(), receipt.to_owned());
        f.store
            .transaction(move |tx| {
                tx.execute("INSERT INTO computer_operation(operation_key,session_id,action,fingerprint,audit,status,started_ms,settled_ms,receipt) VALUES(?1,'owner','click','digest','{}','posted',1,2,?2)",rusqlite::params![key,receipt])?;
                Ok(())
            })
            .unwrap();
    }
    let replayed = f
        .computer
        .journal
        .replay("owner", "v1-op", "digest")
        .unwrap()
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.schema_version, 1);
    let mut expected: serde_json::Value = serde_json::from_str(written).unwrap();
    expected["replayed"] = json!(true);
    expected["schema_version"] = json!(1); // the version it was written with
    assert_eq!(serde_json::to_value(&replayed).unwrap(), expected);
    let shot = f
        .computer
        .journal
        .replay("owner", "v1-shot", "digest")
        .unwrap()
        .unwrap();
    let frame = shot.frame.unwrap();
    assert_eq!(frame.schema_version, 1);
    assert!(frame.target.target_id.is_empty());
    let stored = f
        .store
        .read(|db| {
            db.query_row(
                "SELECT receipt FROM computer_operation WHERE operation_key='v1-op'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(stored, written, "replay never rewrites a version 1 row");
}
