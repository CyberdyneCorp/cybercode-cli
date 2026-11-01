//! Real process death cuts. Only the exact ignored child test reads these fixture
//! variables; the production editor has no environment-triggered crash behavior.
use super::*;
use crate::binary::draft::{PatchBatch, Splice};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ROOT: &str = "CYBER_BINARY_TEST_DEATH_ROOT";
const CUT: &str = "CYBER_BINARY_TEST_DEATH_CUT";
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn owner(root: &Path) -> Owner {
    Owner {
        session_id: "ses_death".into(),
        directory: root.join("project").canonicalize().unwrap(),
    }
}
fn prepare(root: &Path) -> (Owner, String) {
    std::fs::create_dir(root.join("data")).unwrap();
    std::fs::create_dir(root.join("project")).unwrap();
    std::fs::create_dir(root.join("home")).unwrap();
    std::fs::write(root.join("project/bytes"), b"original").unwrap();
    let owner = owner(root);
    let mut document = Document::open(&owner.directory, "bytes").unwrap();
    let mut editor = Editor::create(
        Store::open(&root.join("data"), "bin_death").unwrap(),
        owner.clone(),
        &mut document,
        || false,
    )
    .unwrap();
    editor
        .patch(
            "patch_death",
            &PatchBatch {
                revision: editor.draft().revision(),
                splices: vec![Splice {
                    offset: Offset(0),
                    delete_length: Offset(8),
                    expected_hex: crate::binary::draft::encode_hex(b"original"),
                    insert_hex: "00ff801b".into(),
                }],
            },
        )
        .unwrap();
    let review = editor.review(0, 128, || false).unwrap();
    (owner, review.fingerprint)
}
fn kill_at(root: &Path, cut: &str) {
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "binary::editor::death_tests::death_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root.join("home"))
            .env(ROOT, root)
            .env(CUT, cut)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let barrier = root.join("ready");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !barrier.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "death child exited before {cut}"
        );
        assert!(Instant::now() < deadline, "death child never reached {cut}");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
}

#[test]
#[ignore = "run by native_process_death_retains_drafts_and_never_replays_exchange"]
fn death_child() {
    let Some(root) = std::env::var_os(ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let cut = std::env::var(CUT).unwrap();
    let mut editor = Editor::resume(
        Store::open(&root.join("data"), "bin_death").unwrap(),
        &owner(&root),
    )
    .unwrap();
    let review = editor.review(0, 128, || false).unwrap();
    let SavePreparation::Ready(ready) = editor
        .prepare_save("death_save", &review.fingerprint, || false)
        .unwrap()
    else {
        panic!("new operation")
    };
    match cut.as_str() {
        "prepared" => {}
        "intent" | "exchange" => {
            editor.record.pending = Some(ready.intent);
            persist(&mut editor.store, &editor.record).unwrap();
            if cut == "exchange" {
                let intent = editor.record.pending.as_ref().unwrap();
                native::exchange(editor.store.directory(), &intent.stage, &intent.prior).unwrap();
            }
        }
        "acknowledged" => {
            assert_eq!(
                editor.commit_save(*ready).unwrap().outcome,
                Outcome::Acknowledged
            );
        }
        _ => panic!("unknown death cut"),
    }
    let marker = File::create(root.join("ready")).unwrap();
    marker.sync_all().unwrap();
    loop {
        std::thread::park();
    }
}

fn verify_death_cut(cut: &str) {
    let root = tempfile::tempdir().unwrap();
    let (owner, review) = prepare(root.path());
    kill_at(root.path(), cut);
    let mut editor = Editor::resume(
        Store::open(&root.path().join("data"), "bin_death").unwrap(),
        &owner,
    )
    .unwrap();
    let source = std::fs::read(root.path().join("project/bytes")).unwrap();
    if cut == "prepared" {
        assert!(!editor.unresolved());
        assert!(editor.operation("death_save").is_none());
        assert_eq!(source, b"original");
        assert_eq!(editor.draft().read(0, 4).unwrap(), [0, 255, 128, 27]);
        return;
    }
    if cut == "intent" {
        assert_eq!(editor.reconcile().unwrap().outcome, Outcome::Unknown);
        assert_eq!(source, b"original");
        return;
    }
    let identity = Target::capture(&owner.directory, Path::new("bytes"))
        .unwrap()
        .file;
    if cut == "exchange" {
        assert_eq!(editor.reconcile().unwrap().outcome, Outcome::Acknowledged);
    }
    assert_eq!(source, [0, 255, 128, 27]);
    assert_eq!(
        editor.operation("death_save").unwrap().outcome,
        Outcome::Acknowledged
    );
    let SavePreparation::Existing(receipt) = editor
        .prepare_save("death_save", &review, || false)
        .unwrap()
    else {
        panic!("recovery repeated a native effect")
    };
    assert_eq!(receipt.outcome, Outcome::Acknowledged);
    assert_eq!(editor.target().file, identity);
}
#[test]
fn native_process_death_retains_drafts_and_never_replays_exchange() {
    for cut in ["prepared", "intent", "exchange", "acknowledged"] {
        verify_death_cut(cut);
    }
}
#[test]
fn native_crash_recovery_preserves_an_external_post_exchange_change_and_its_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (owner, _) = prepare(root.path());
    kill_at(root.path(), "exchange");
    std::fs::write(root.path().join("project/bytes"), b"external").unwrap();
    let mut editor = Editor::resume(
        Store::open(&root.path().join("data"), "bin_death").unwrap(),
        &owner,
    )
    .unwrap();
    assert_eq!(editor.reconcile().unwrap().outcome, Outcome::Unknown);
    assert!(editor.unresolved());
    assert_eq!(
        std::fs::read(root.path().join("project/bytes")).unwrap(),
        b"external"
    );
    let intent = editor.record.pending.as_ref().unwrap();
    assert_eq!(
        editor.store.directory().read(&intent.stage).unwrap(),
        b"original"
    );
}
