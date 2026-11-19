//! Epoch-pinned Context Sources (`system-context` → Epoch-pinned Context Sources): observed
//! only when a baseline is rendered, never reconciled, and never blocking a Turn.
mod support;

use std::time::{Duration, Instant};

use cyber_server::runtime::{Admission, Delivery};
use support::*;

fn admit(text: &str) -> Admission {
    Admission::text(text, Delivery::Queue)
}

fn pin(h: &Harness, text: &str, delay: Duration) {
    *h.tools.pinned.lock().unwrap() = Some((text.to_owned(), delay));
}

#[tokio::test]
async fn pinned_source_changes_mid_epoch() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b")])],
        ..Setup::default()
    });
    pin(&h, "<repository_map>first</repository_map>", Duration::ZERO);
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    pin(
        &h,
        "<repository_map>second</repository_map>",
        Duration::ZERO,
    );
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.settle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].system[1].contains("<repository_map>first"));
    assert_eq!(
        requests[0].system, requests[1].system,
        "the system prefix is byte-identical"
    );
    let state = h.state(&id).await;
    let epoch = state.epoch.as_ref().unwrap();
    assert_eq!(epoch.number, 1);
    assert_eq!(
        epoch.snapshot["core/repo-map"],
        "<repository_map>first</repository_map>"
    );
    assert!(epoch.sources["core/repo-map"].pinned);
    let messages = format!("{:?}", requests[1].messages);
    assert!(!messages.contains("second"), "no mid-conversation update");
    assert_eq!(
        h.tools
            .pinned_observed
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "observed only when the baseline was rendered"
    );
}

#[tokio::test]
async fn slow_pinned_source_at_the_first_turn() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("a"), text("b"), text("c")]),
            ("test/summary", vec![text("summary")]),
        ],
        compaction: cyber_server::runtime::CompactionConfig {
            auto: true,
            buffer: 100,
            keep_tokens: 4,
            keep_turns: None,
            model: None,
        },
        ..Setup::default()
    });
    pin(
        &h,
        "<repository_map>slow</repository_map>",
        Duration::from_secs(5),
    );
    let id = h.session().await;
    let started = Instant::now();
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(1900), "{waited:?}");
    assert!(waited < Duration::from_millis(4500), "{waited:?}");
    let requests = h.models.requests("test/main");
    assert!(!requests[0].system[1].contains("repository_map"));
    assert!(
        !h.state(&id)
            .await
            .epoch
            .unwrap()
            .snapshot
            .contains_key("core/repo-map")
    );
    // The source first appears in the baseline of the Session's next Context Epoch.
    pin(&h, "<repository_map>ready</repository_map>", Duration::ZERO);
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.settle(&id).await;
    h.runtime.compact(&id, None).await.unwrap();
    assert!(h.state(&id).await.compacted.is_some());
    h.runtime.admit(&id, admit("three")).await.unwrap();
    h.settle(&id).await;
    let last = h.models.requests("test/main").pop().unwrap();
    assert!(
        last.system[1].contains("<repository_map>ready"),
        "{:?}",
        last.system
    );
}

#[tokio::test]
async fn restart_replays_pinned_bytes() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b")])],
        ..Setup::default()
    });
    pin(
        &h,
        "<repository_map>stored</repository_map>",
        Duration::ZERO,
    );
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    pin(
        &h,
        "<repository_map>changed</repository_map>",
        Duration::ZERO,
    );
    let restarted = h.restart();
    restarted.admit(&id, admit("two")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests[0].system, requests[1].system);
    assert!(requests[1].system[1].contains("<repository_map>stored"));
}
