//! `system-context` (Context Epochs) and `compaction`, end to end.

mod support;

use cyber_llm::{Content, ErrorKind};
use cyber_server::runtime::{Admission, CompactionConfig, Delivery, InputStatus, RuntimeError};
use support::*;

fn admit(text: &str) -> Admission {
    Admission::text(text, Delivery::Steer)
}

fn small_tail() -> CompactionConfig {
    CompactionConfig {
        auto: true,
        buffer: 100,
        keep_tokens: 4,
        keep_turns: None,
        model: None,
    }
}

#[tokio::test]
async fn historical_epoch_without_a_system_prefix_keeps_legacy_request_bytes() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("reply")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    h.store.append(&id, cyber_store::Expected::Seq(seq), vec![cyber_store::NewEvent::new(
        "session.context.epoch_started.1", serde_json::json!({
            "epoch":1, "baseline":"Historical baseline bytes.", "snapshot":{}, "provider":"test"
        }),
    )]).unwrap();
    let restarted = h.restart();
    restarted.admit(&id, admit("continue")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(
        requests[0].system,
        [
            cyber_server::runtime::base_prompt("test"),
            "Historical baseline bytes.".into()
        ]
    );
    assert!(
        restarted
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .system_prefix
            .is_none()
    );
}

#[tokio::test]
async fn baseline_is_byte_stable_within_an_epoch() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b")])],
        ..Setup::default()
    });
    std::fs::write(h.repo.join("AGENTS.md"), "Use pnpm.").unwrap();
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    let restarted = h.restart();
    restarted.admit(&id, admit("two")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(
        requests[0].system, requests[1].system,
        "same bytes across Turns and restarts"
    );
    let baseline = &requests[0].system[1];
    assert!(baseline.contains("Today's date: 2026-10-03"));
    assert!(baseline.contains("Instructions from:") && baseline.contains("Use pnpm."));
    assert!(baseline.contains(&format!("Working directory: {}", h.repo.display())));
}

#[tokio::test]
async fn edited_instructions_arrive_as_one_system_message() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b")])],
        ..Setup::default()
    });
    std::fs::write(h.repo.join("AGENTS.md"), "Use pnpm.").unwrap();
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    std::fs::write(h.repo.join("AGENTS.md"), "Use bun.").unwrap();
    assert_eq!(
        h.models.requests("test/main").len(),
        1,
        "a changed source never wakes an idle Session"
    );
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.settle(&id).await;
    let second = &h.models.requests("test/main")[1];
    assert_eq!(
        second.system,
        h.models.requests("test/main")[0].system,
        "the cached prefix is not rewritten"
    );
    let reminders = transcript(second).matches("<system-reminder>").count();
    assert_eq!(reminders, 1);
    assert!(transcript(second).contains("Use bun."));
    let user_two = transcript(second).find("\"two\"").unwrap();
    assert!(
        transcript(second).find("Use bun.").unwrap() > user_two,
        "the update follows newly promoted input"
    );
}

#[tokio::test]
async fn unreadable_instructions_block_the_first_turn_and_keep_the_prompt() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("ok")])],
        ..Setup::default()
    });
    std::fs::create_dir(h.repo.join("AGENTS.md")).unwrap(); // reading a directory fails with an I/O error
    let mut live = h.runtime.subscribe();
    let id = h.session().await;
    h.runtime.admit(&id, admit("hi")).await.unwrap();
    h.settle(&id).await;
    let (kind, message) = next_error(&mut live).await;
    assert_eq!(kind, "context_initialization_blocked");
    assert!(message.contains("core/instructions"));
    assert_eq!(
        h.state(&id).await.inbox[0].status,
        InputStatus::Pending,
        "the prompt stays retryable"
    );

    std::fs::remove_dir(h.repo.join("AGENTS.md")).unwrap();
    h.runtime.wake(&id).await.unwrap();
    h.settle(&id).await;
    assert_eq!(h.state(&id).await.inbox[0].status, InputStatus::Promoted);
}

#[tokio::test]
async fn provider_family_switch_starts_a_new_epoch() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("a")]),
            ("other/main", vec![text("b")]),
        ],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    assert_eq!(h.state(&id).await.epoch.unwrap().number, 1);
    h.runtime.switch_model(&id, "other/main").await.unwrap();
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.settle(&id).await;
    let epoch = h.state(&id).await.epoch.unwrap();
    assert_eq!((epoch.number, epoch.provider.as_str()), (2, "other"));
}

#[tokio::test]
async fn repair_context_starts_a_fresh_epoch() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a")])],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    h.runtime.repair_context(&id).await.unwrap();
    assert_eq!(h.state(&id).await.epoch.unwrap().number, 2);
}

#[tokio::test]
async fn manual_compaction_keeps_a_verbatim_tail_and_the_task_state() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("a1"), text("a2"), text("a3")]),
            ("test/summary", vec![text("## Objective\nship the parser")]),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime
        .admit(&id, admit("do not deploy; refactor the parser"))
        .await
        .unwrap();
    h.settle(&id).await;
    h.runtime.admit(&id, admit("keep going")).await.unwrap();
    h.settle(&id).await;
    let constraint_id = h.state(&id).await.entries[0].id().to_string();

    h.runtime
        .compact(&id, Some("focus on the parser".into()))
        .await
        .unwrap();
    let state = h.state(&id).await;
    let compacted = state.compacted.clone().expect("compacted");
    assert!(compacted.tail_start > 0 && compacted.tail_start < state.entries.len());
    assert!(state.epoch_stale, "the next Turn starts a new epoch");
    let summary_request = &h.models.requests("test/summary")[0];
    assert!(transcript(summary_request).contains("focus on the parser"));
    assert!(summary_request.tools.is_empty());

    h.runtime.admit(&id, admit("next")).await.unwrap();
    h.settle(&id).await;
    let next = &h.models.requests("test/main")[2];
    let first = serde_json::to_string(&next.messages[0]).unwrap();
    assert!(first.contains("ship the parser"));
    assert!(
        first.contains(&format!(
            "[{constraint_id}] do not deploy; refactor the parser"
        )),
        "task state keeps the source ID"
    );
    assert!(
        !transcript(next).contains("auto-compacted")
            && !transcript(next).contains("compacted automatically")
    );
    assert_eq!(h.state(&id).await.epoch.unwrap().number, 2);
}

#[tokio::test]
async fn repeated_compaction_never_drops_a_user_constraint() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                (0..5).map(|i| text(&format!("reply {i}"))).collect(),
            ),
            (
                "test/summary",
                (0..3)
                    .map(|i| text(&format!("summary {i} without the constraint")))
                    .collect(),
            ),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("do not deploy")).await.unwrap();
    h.settle(&id).await;
    let source = h.state(&id).await.entries[0].id().to_string();
    for i in 0..3 {
        h.runtime
            .admit(&id, admit(&format!("step {i}")))
            .await
            .unwrap();
        h.settle(&id).await;
        h.runtime.compact(&id, None).await.unwrap();
    }
    h.runtime.admit(&id, admit("final")).await.unwrap();
    h.settle(&id).await;
    let last = h.models.requests("test/main").pop().unwrap();
    let summary = serde_json::to_string(&last.messages[0]).unwrap();
    assert!(
        summary.contains("summary 2"),
        "the latest summary supersedes earlier ones"
    );
    assert!(summary.contains(&format!("[{source}] do not deploy")));
    let merged = transcript(&h.models.requests("test/summary")[2]);
    assert!(
        merged.contains("summary 1"),
        "the previous summary is merged into the next"
    );
}

#[tokio::test]
async fn automatic_compaction_runs_before_an_oversized_turn_and_continues() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![text(&"x".repeat(4000)), text("after compaction")],
            ),
            ("test/summary", vec![text("summary of the long answer")]),
        ],
        context_limit: 1_500,
        compaction: CompactionConfig {
            auto: true,
            buffer: 100,
            keep_tokens: 50,
            keep_turns: None,
            model: None,
        },
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("write a lot")).await.unwrap();
    h.settle(&id).await;
    h.runtime.admit(&id, admit("now continue")).await.unwrap();
    h.settle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 2);
    let second = transcript(&requests[1]);
    assert!(second.contains("summary of the long answer"));
    assert!(
        !second.contains(&"x".repeat(4000)),
        "the long answer was summarized"
    );
    assert!(
        second.contains("compacted automatically"),
        "automatic compaction continues the work"
    );
}

#[tokio::test]
async fn overflow_compacts_once_and_retries_with_media_stripped() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![
                    text("first"),
                    error(ErrorKind::ContextOverflow, "prompt is too long"),
                    text("recovered"),
                ],
            ),
            ("test/summary", vec![text("summary")]),
        ],
        compaction: CompactionConfig {
            auto: true,
            buffer: 100,
            keep_tokens: 10_000,
            keep_turns: Some(1),
            model: None,
        },
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("hello")).await.unwrap();
    h.settle(&id).await;
    let mut with_image = admit("look at this screenshot");
    with_image.parts.push(Content::Image {
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    });
    h.runtime.admit(&id, with_image).await.unwrap();
    h.settle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 3);
    let retried = transcript(&requests[2]);
    assert!(retried.contains("[Attached image/png]") && !retried.contains("iVBORw0KGgo="));
    assert!(
        matches!(&h.state(&id).await.entries.last().unwrap(), cyber_server::runtime::Entry::Assistant(a) if a.text == "recovered")
    );
}

#[tokio::test]
async fn overflow_without_auto_compaction_stops_with_the_error() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![error(ErrorKind::ContextOverflow, "prompt is too long")],
        )],
        compaction: CompactionConfig {
            auto: false,
            ..CompactionConfig::default()
        },
        ..Setup::default()
    });
    let mut live = h.runtime.subscribe();
    let id = h.session().await;
    h.runtime.admit(&id, admit("hi")).await.unwrap();
    h.settle(&id).await;
    let (_, message) = next_error(&mut live).await;
    assert!(message.contains("ContextOverflow"));
    assert!(h.state(&id).await.compacted.is_none());
}

#[tokio::test]
async fn failed_summary_stops_with_a_clear_message() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("a"), text("b")]),
            (
                "test/summary",
                vec![error(ErrorKind::ContextOverflow, "summary too large")],
            ),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.settle(&id).await;
    match h.runtime.compact(&id, None).await {
        Err(RuntimeError::Compaction(m)) => assert!(m.starts_with("Session too large to compact")),
        other => panic!("expected a compaction failure, got {other:?}"),
    }
    assert!(h.state(&id).await.compacted.is_none());
}

#[tokio::test]
async fn compaction_requested_during_a_turn_waits_for_the_safe_boundary() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![text("a"), tools(&[("c1", "write", "{}")]), text("b")],
            ),
            ("test/summary", vec![text("summary")]),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    h.runtime.admit(&id, admit("two")).await.unwrap();
    h.tools.started.notified().await;
    h.runtime.compact(&id, None).await.unwrap();
    assert!(
        h.state(&id).await.compacted.is_none(),
        "not while tools run"
    );
    h.tools.release.notify_one();
    h.settle(&id).await;
    assert!(h.state(&id).await.compacted.is_some());
    assert!(transcript(&h.models.requests("test/main")[2]).contains("summary"));
}

/// Regression: when the newest entry alone exceeds the tail budget, the tail is empty and
/// compaction must summarize everything instead of indexing past the end.
#[tokio::test]
async fn oversized_newest_entry_compacts_with_an_empty_tail() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text(&"y".repeat(400)), text("ok")]),
            ("test/summary", vec![text("all summarized")]),
        ],
        compaction: CompactionConfig {
            auto: true,
            buffer: 100,
            keep_tokens: 4,
            keep_turns: None,
            model: None,
        },
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime
        .admit(&id, admit("long answer please"))
        .await
        .unwrap();
    h.settle(&id).await;
    h.runtime.compact(&id, None).await.unwrap();
    let state = h.state(&id).await;
    assert_eq!(
        state.compacted.as_ref().unwrap().tail_start,
        state.entries.len()
    );
    h.runtime.admit(&id, admit("next")).await.unwrap();
    h.settle(&id).await;
    let request = h.models.requests("test/main").pop().unwrap();
    assert_eq!(request.messages.len(), 2, "summary plus the new prompt");
    assert!(!transcript(&request).contains(&"y".repeat(400)));
}

#[tokio::test]
async fn loaded_schemas_survive_compaction_and_restart() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("one"), text("two")]),
            ("test/summary", vec![text("preserved objective")]),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime
        .load_tool_schemas(
            &id,
            vec!["mcp__jira__read".into()],
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    for prompt in ["first", "second"] {
        h.runtime.admit(&id, admit(prompt)).await.unwrap();
        h.settle(&id).await;
    }
    h.runtime.compact(&id, None).await.unwrap();
    assert!(h.state(&id).await.compacted.is_some());
    assert!(
        h.state(&id)
            .await
            .loaded_tool_names()
            .contains("mcp__jira__read")
    );
    assert!(
        h.restart()
            .state(&id)
            .await
            .unwrap()
            .loaded_tool_names()
            .contains("mcp__jira__read")
    );
}
