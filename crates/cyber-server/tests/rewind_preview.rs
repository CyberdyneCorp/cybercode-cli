//! Preview ownership and uncertain native settlement, with a scripted provider and
//! an instrumented snapshot port. Real Git files are covered by branches_cli.
mod support;
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support::*;

#[derive(Default)]
struct Images {
    fail: AtomicBool,
    supported: AtomicBool,
    restored: AtomicUsize,
    gate_diff: AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Snapshots for Images {
    fn track(&self, _: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        Box::pin(async {
            Ok(Some(Snapshot {
                tree: "tree_current".into(),
                skipped: vec![],
            }))
        })
    }
    fn changed(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn diff(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<FileDiff>, String>> {
        Box::pin(async move {
            if self.gate_diff.swap(false, Ordering::SeqCst) {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(vec![])
        })
    }
    fn restore(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        panic!("confirmed rewind must never fall back to unchecked legacy restore")
    }
    fn reviewed_restore_available(&self) -> bool {
        self.supported.load(Ordering::SeqCst)
    }
    fn preview_restore(
        &self,
        directory: &str,
        target: &str,
        _: &str,
        expected: &str,
    ) -> BoxFuture<'_, Result<Vec<FileDiff>, RestoreError>> {
        let diff = self.diff(directory, expected, target);
        Box::pin(async move { diff.await.map_err(RestoreError::Failed) })
    }
    fn restore_reviewed(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        authorize: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        Box::pin(async move {
            authorize().map_err(RestoreError::Failed)?;
            self.restored.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err(RestoreError::Failed(
                    "native effect lost acknowledgement".into(),
                ))
            } else {
                Ok(vec![])
            }
        })
    }
}
fn runtime_with_images(h: &Harness, images: Arc<Images>) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: h.tools.clone(),
        global_config_dir: h.repo.join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: images,
    })
}
async fn seed(h: &Harness, runtime: &Runtime) -> (String, RewindSelection) {
    let id = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let message_id = runtime
        .admit(&id, Admission::text("first", Delivery::Queue))
        .await
        .unwrap()
        .message_id;
    runtime.wait_idle(&id).await;
    (
        id,
        RewindSelection {
            action: RewindAction::Commit,
            message_id,
            target: RevertTarget::Both,
            expected_directory: h.repo.display().to_string(),
        },
    )
}
fn harness() -> Harness {
    Harness::new(Setup {
        scripts: vec![("test/main", vec![text("one"), text("two")])],
        ..Default::default()
    })
}

#[tokio::test]
async fn failed_native_ack_keeps_activity_unknown_without_moving_the_branch() {
    let h = harness();
    let images = Arc::new(Images::default());
    images.supported.store(true, Ordering::SeqCst);
    images.fail.store(true, Ordering::SeqCst);
    let runtime = runtime_with_images(&h, images.clone());
    let (id, selection) = seed(&h, &runtime).await;
    let before = runtime.state(&id).await.unwrap();
    let preview = runtime.rewind_preview(&id, &selection).await.unwrap();
    assert!(
        runtime
            .rewind_confirm(
                &id,
                &RewindConfirmation {
                    selection,
                    confirmation: preview.confirmation
                }
            )
            .await
            .is_err()
    );
    assert_eq!(images.restored.load(Ordering::SeqCst), 1);
    let after = runtime.state(&id).await.unwrap();
    assert_eq!(after.last_seq, before.last_seq);
    assert_eq!(after.entries, before.entries);
    let status: String = h.store.read(move |db| {
        Ok(db.query_row("SELECT json_extract(data,'$.status') FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 ORDER BY rowid DESC LIMIT 1", [id], |row| row.get(0))?)
    }).unwrap();
    assert_eq!(
        status, "unknown",
        "possible native effects cannot be settled as failure"
    );
}

#[tokio::test]
async fn unsupported_checked_restore_is_refused_but_conversation_and_scope_checks_work() {
    let h = harness();
    let images = Arc::new(Images::default());
    let runtime = runtime_with_images(&h, images.clone());
    let (id, mut selection) = seed(&h, &runtime).await;
    assert!(runtime.rewind_preview(&id, &selection).await.is_err());
    assert_eq!(images.restored.load(Ordering::SeqCst), 0);
    selection.target = RevertTarget::Conversation;
    let preview = runtime.rewind_preview(&id, &selection).await.unwrap();
    let mut foreign = selection.clone();
    foreign.expected_directory.push_str("/other");
    assert!(
        runtime
            .rewind_confirm(
                &id,
                &RewindConfirmation {
                    selection: foreign,
                    confirmation: preview.confirmation.clone()
                }
            )
            .await
            .is_err()
    );
    runtime
        .rewind_confirm(
            &id,
            &RewindConfirmation {
                selection,
                confirmation: preview.confirmation,
            },
        )
        .await
        .unwrap();
    assert!(runtime.state(&id).await.unwrap().entries.is_empty());
    assert_eq!(images.restored.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn input_admitted_while_a_preview_waits_invalidates_the_result() {
    let h = harness();
    let images = Arc::new(Images::default());
    images.supported.store(true, Ordering::SeqCst);
    let runtime = runtime_with_images(&h, images.clone());
    let (id, selection) = seed(&h, &runtime).await;
    images.gate_diff.store(true, Ordering::SeqCst);
    let preview = runtime.rewind_preview(&id, &selection);
    let race = async {
        images.started.notified().await;
        let receipt = runtime
            .admit(&id, Admission::text("second", Delivery::Queue))
            .await
            .unwrap();
        images.release.notify_one();
        receipt.message_id
    };
    let (preview, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(preview, race)
    })
    .await
    .unwrap();
    assert!(preview.is_err());
    runtime.wait_idle(&id).await;
    assert!(
        runtime
            .state(&id)
            .await
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.id() == second)
    );
    assert_eq!(images.restored.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reviewed_rewind_does_not_silently_finalize_a_legacy_staged_revert() {
    let h = harness();
    let images = Arc::new(Images::default());
    let runtime = runtime_with_images(&h, images);
    let (id, mut selection) = seed(&h, &runtime).await;
    selection.target = RevertTarget::Conversation;
    runtime
        .revert_stage(&id, &selection.message_id, selection.target)
        .await
        .unwrap();
    let before = runtime.state(&id).await.unwrap();
    assert!(
        matches!(runtime.rewind_preview(&id, &selection).await, Err(RuntimeError::Invalid(message)) if message.contains("already staged"))
    );
    let after = runtime.state(&id).await.unwrap();
    assert_eq!(before.entries, after.entries);
    assert_eq!(before.revert, after.revert);
    runtime.revert_clear(&id).await.unwrap();
    assert!(runtime.rewind_preview(&id, &selection).await.is_ok());
}
