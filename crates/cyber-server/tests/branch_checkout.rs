//! Session branches (`session-runtime` → Session branches): a checkout with `code` awaits
//! the working-tree restore without the Session lock, so input admitted meanwhile must
//! still refuse it. A snapshot host whose restore waits at a gate opens that window.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_server::runtime::{
    Admission, BranchCheckout, CompactionConfig, Delivery, Entry, FileDiff, RestoreError,
    RevertTarget, Runtime, RuntimeError, RuntimeOptions, Snapshot, Snapshots,
};
use futures::future::BoxFuture;
use support::*;

/// Trees `t1`, `t2`, ... per track; the first restore waits until `release`.
#[derive(Default)]
struct GatedRestore {
    tracked: Mutex<u32>,
    restores: Mutex<Vec<(String, String)>>,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Snapshots for GatedRestore {
    fn track(&self, _: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        Box::pin(async move {
            let mut tracked = self.tracked.lock().unwrap();
            *tracked += 1;
            Ok(Some(Snapshot {
                tree: format!("t{tracked}"),
                skipped: Vec::new(),
            }))
        })
    }

    fn changed(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn diff(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<FileDiff>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn restore(
        &self,
        _: &str,
        target: &str,
        recorded: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        let first = {
            let mut restores = self.restores.lock().unwrap();
            restores.push((target.to_owned(), recorded.to_owned()));
            restores.len() == 1
        };
        Box::pin(async move {
            if first {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(Vec::new())
        })
    }
}

fn user_ids(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| matches!(e, Entry::User { .. }))
        .map(|e| e.id().to_owned())
        .collect()
}

/// Regression: a prompt admitted while a `code` checkout restores files starts a Drain
/// on the old path; the checkout then fails with `SessionBusyError`, records nothing,
/// leaves the active path alone and puts the restored files back.
#[tokio::test]
async fn a_prompt_admitted_during_a_code_restore_refuses_the_checkout() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("one"), text("two"), text("three")])],
        ..Setup::default()
    });
    let snapshots = Arc::new(GatedRestore::default());
    let runtime = Runtime::new(RuntimeOptions {
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
        snapshots: snapshots.clone(),
    });
    let id = runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let settle = || async {
        tokio::time::timeout(Duration::from_secs(60), runtime.wait_idle(&id))
            .await
            .expect("drain did not settle");
    };
    let first = runtime
        .admit(&id, Admission::text("first", Delivery::Queue))
        .await
        .unwrap()
        .message_id;
    settle().await;
    runtime
        .revert_stage(&id, &first, RevertTarget::Conversation)
        .await
        .unwrap();
    runtime.revert_commit(&id).await.unwrap();
    runtime
        .admit(&id, Admission::text("second", Delivery::Queue))
        .await
        .unwrap();
    settle().await;
    let before = runtime.state(&id).await.unwrap();

    let request = BranchCheckout {
        message_id: Some(first.clone()),
        code: true,
        ..BranchCheckout::default()
    };
    let race = async {
        snapshots.started.notified().await;
        let third = runtime
            .admit(&id, Admission::text("third", Delivery::Queue))
            .await
            .unwrap();
        snapshots.release.notify_one();
        third.message_id
    };
    let (checkout, third) = tokio::join!(runtime.checkout(&id, &request), race);
    assert!(
        matches!(&checkout, Err(RuntimeError::Busy(_))),
        "{checkout:?}"
    );
    settle().await;
    let after = runtime.state(&id).await.unwrap();
    let mut expected = user_ids(&before.entries);
    expected.push(third);
    assert_eq!(user_ids(&after.entries), expected);
    assert!(!user_ids(&after.entries).contains(&first));
    assert_eq!(after.revert, None, "no code revert was staged");
    let events = h.store.read_events(&id, 0, 500).unwrap().events;
    assert!(
        !events
            .iter()
            .any(|e| e.kind.starts_with("session.branch.checked_out"))
    );
    // The refusal restored the working tree from the branch's snapshot to the one before.
    let restores = snapshots.restores.lock().unwrap().clone();
    assert_eq!(restores.len(), 2, "{restores:?}");
    let (target, _) = &restores[0];
    assert_eq!(&restores[1].1, target);
    assert_ne!(&restores[1].0, target);
    runtime.shutdown().await;
}
