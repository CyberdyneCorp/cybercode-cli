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
async fn inspection_preserves_source_provenance_when_a_later_observation_is_unavailable() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("first"), text("second")])],
        ..Setup::default()
    });
    std::fs::write(
        h.repo.join("CYBER.md"),
        "Private retained instruction body.",
    )
    .unwrap();
    let id = h.session().await;
    h.runtime.admit(&id, admit("begin")).await.unwrap();
    h.settle(&id).await;
    let before = h.runtime.state(&id).await.unwrap();
    let first = serde_json::to_value(h.runtime.inspect_context(&before)).unwrap();
    let source = |inspection: &serde_json::Value| {
        inspection["effective_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == "core/instructions")
            .unwrap()
            .clone()
    };
    let active = source(&first);
    assert_eq!(active["status"], "active");
    assert_eq!(active["origin"], "instruction_files");
    assert!(
        active["file_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| { path.as_str().unwrap().ends_with("CYBER.md") })
    );
    *h.tools.instruction_limits.lock().unwrap() = Err("private diagnostic sentinel".into());
    h.runtime.admit(&id, admit("continue")).await.unwrap();
    h.settle(&id).await;
    let after = h.runtime.state(&id).await.unwrap();
    let system_entries = |state: &cyber_server::runtime::SessionState| {
        state
            .entries
            .iter()
            .filter(|entry| matches!(entry, cyber_server::runtime::Entry::System { .. }))
            .count()
    };
    assert_eq!(system_entries(&before), system_entries(&after));
    let inspection = serde_json::to_value(h.runtime.inspect_context(&after)).unwrap();
    let retained = source(&inspection);
    assert_eq!(retained["status"], "retained_unavailable");
    assert_eq!(retained["file_paths"], active["file_paths"]);
    assert_eq!(retained["estimated_tokens"], active["estimated_tokens"]);
    assert_eq!(
        before.epoch.unwrap().baseline,
        after.epoch.unwrap().baseline
    );
    assert!(
        !inspection
            .to_string()
            .contains("private diagnostic sentinel")
    );
    assert!(
        !inspection
            .to_string()
            .contains("Private retained instruction body.")
    );
    let restarted = h.restart();
    let loaded = restarted.state(&id).await.unwrap();
    assert_eq!(
        serde_json::to_value(restarted.inspect_context(&loaded)).unwrap(),
        inspection
    );
    assert_eq!(h.models.requests("test/main").len(), 2);
}

/// `system-context` → Scope-aware rule inspection and loading: an edited or removed
/// instruction file shows as changed on disk until the next request boundary reads it,
/// and inspection neither opens it nor records anything.
#[tokio::test]
async fn inspection_reports_instruction_files_changed_since_they_were_read() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("first"), text("second")])],
        ..Setup::default()
    });
    let file = h.repo.join("CYBER.md");
    std::fs::write(&file, "Original instruction.").unwrap();
    let id = h.session().await;
    h.runtime.admit(&id, admit("begin")).await.unwrap();
    h.settle(&id).await;
    let inspect = |runtime: &cyber_server::runtime::Runtime,
                   state: &cyber_server::runtime::SessionState| {
        let inspection = serde_json::to_value(runtime.inspect_context(state)).unwrap();
        let source = inspection["effective_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == "core/instructions")
            .unwrap()
            .clone();
        (inspection, source)
    };
    let (inspection, source) = inspect(&h.runtime, &h.runtime.state(&id).await.unwrap());
    assert_eq!(source["changed_on_disk"], false, "{inspection:#}");
    assert_eq!(inspection["changed_sources"], 0);
    let path = file.to_str().unwrap();
    assert!(source["file_stamps"][path]["bytes"].as_u64().unwrap() > 0);

    std::fs::write(&file, "Edited instruction, now longer than before.").unwrap();
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    let state = h.runtime.state(&id).await.unwrap();
    let (inspection, source) = inspect(&h.runtime, &state);
    assert_eq!(source["changed_on_disk"], true, "{inspection:#}");
    assert_eq!(inspection["changed_sources"], 1);
    assert!(!inspection.to_string().contains("Edited instruction"));
    assert_eq!(h.store.aggregate_seq(&id).unwrap().unwrap(), seq);
    assert_eq!(h.models.requests("test/main").len(), 1);
    let restarted = h.restart();
    let (_, source) = inspect(&restarted, &restarted.state(&id).await.unwrap());
    assert_eq!(source["changed_on_disk"], true);

    restarted.admit(&id, admit("continue")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert!(format!("{:?}", requests[1]).contains("Edited instruction"));
    let (inspection, source) = inspect(&restarted, &restarted.state(&id).await.unwrap());
    assert_eq!(source["changed_on_disk"], false, "{inspection:#}");

    std::fs::remove_file(&file).unwrap();
    let (inspection, source) = inspect(&restarted, &restarted.state(&id).await.unwrap());
    assert_eq!(source["changed_on_disk"], true, "{inspection:#}");
    assert_eq!(inspection["changed_sources"], 1);
}

#[tokio::test]
async fn historical_source_provenance_stays_unknown_during_read_only_inspection() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    h.store
        .append(
            &id,
            cyber_store::Expected::Seq(seq),
            vec![cyber_store::NewEvent::new(
                "session.context.epoch_started.1",
                serde_json::json!({
                    "epoch": 1, "baseline": "Private old bytes.",
                    "snapshot": {"legacy/source": "Private old bytes."}, "provider": "test"
                }),
            )],
        )
        .unwrap();
    let restarted = h.restart();
    let state = restarted.state(&id).await.unwrap();
    let inspection = serde_json::to_value(restarted.inspect_context(&state)).unwrap();
    let source = inspection["effective_sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["id"] == "legacy/source")
        .unwrap();
    assert_eq!(source["status"], "unknown");
    assert_eq!(source["origin"], "unknown");
    assert!(source["estimated_tokens"].as_u64().unwrap() > 0);
    assert!(!inspection.to_string().contains("Private old bytes."));
    assert!(h.models.requests("test/main").is_empty());
    assert_eq!(h.store.aggregate_seq(&id).unwrap().unwrap(), seq + 1);
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
            cyber_server::runtime::base_prompt(cyber_server::runtime::ModelFamily::Default),
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

/// Regression: an epoch from before model families has no `family`; comparing it with the
/// resolved family would rebuild every resumed Session's prefix.
#[tokio::test]
async fn epoch_without_a_family_keeps_its_prefix_bytes() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("reply")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    h.store.append(&id, cyber_store::Expected::Seq(seq), vec![cyber_store::NewEvent::new(
        "session.context.epoch_started.1", serde_json::json!({
            "epoch":1, "baseline":"Historical baseline bytes.", "snapshot":{}, "provider":"test",
            "system_prefix":"Historical base prompt."
        }),
    )]).unwrap();
    let restarted = h.restart();
    restarted.admit(&id, admit("continue")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(
        requests[0].system,
        ["Historical base prompt.", "Historical baseline bytes."]
    );
    let epoch = restarted.state(&id).await.unwrap().epoch.unwrap();
    assert_eq!((epoch.number, epoch.family), (1, None));
}

/// `system-context` → Base prompt coverage, "Prompt version change".
#[tokio::test]
async fn prompt_version_change_starts_a_new_epoch() {
    epoch_built_with_version_is_replaced(1).await;
}

/// `system-context` → Untrusted content guidance, "Version bump starts a new epoch": an
/// epoch built with the prompt before the untrusted-content topic (version 2) is replaced
/// at the next Turn.
#[tokio::test]
async fn untrusted_content_topic_version_bump_starts_a_new_epoch() {
    assert!(cyber_core::prompts::base_prompt_version() >= 3);
    epoch_built_with_version_is_replaced(2).await;
}

async fn epoch_built_with_version_is_replaced(old: u32) {
    use cyber_server::runtime::{ModelFamily, base_prompt};
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b")])],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime.admit(&id, admit("one")).await.unwrap();
    h.settle(&id).await;
    let built = h.state(&id).await.epoch.unwrap();
    let version = cyber_core::prompts::base_prompt_version();
    assert_eq!((built.number, built.prompt_version), (1, Some(version)));
    let family = ModelFamily::ALL
        .into_iter()
        .find(|f| Some(f.as_str()) == built.family.as_deref())
        .unwrap();

    // The same family, but built with an older prompt.
    let seq = h.store.aggregate_seq(&id).unwrap().unwrap();
    h.store.append(&id, cyber_store::Expected::Seq(seq), vec![cyber_store::NewEvent::new(
        "session.context.epoch_started.1", serde_json::json!({
            "epoch":2, "baseline":built.baseline, "snapshot":built.snapshot, "provider":"test",
            "system_prefix":format!("Version {old} base prompt."), "family":family.as_str(),
            "prompt_version":old
        }),
    )]).unwrap();
    let restarted = h.restart();
    restarted.admit(&id, admit("two")).await.unwrap();
    restarted.wait_idle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests.last().unwrap().system[0], base_prompt(family));
    let epoch = restarted.state(&id).await.unwrap().epoch.unwrap();
    assert_eq!((epoch.number, epoch.prompt_version), (3, Some(version)));
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
async fn unavailable_instruction_limits_keep_the_first_input_retryable_without_inference() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("ok")])],
        ..Setup::default()
    });
    std::fs::write(h.repo.join("AGENTS.md"), "configured instructions").unwrap();
    *h.tools.instruction_limits.lock().unwrap() = Err("instruction_limits unavailable".into());
    let mut live = h.runtime.subscribe();
    let id = h.session().await;
    h.runtime.admit(&id, admit("hi")).await.unwrap();
    h.settle(&id).await;
    let (kind, _) = next_error(&mut live).await;
    assert_eq!(kind, "context_initialization_blocked");
    assert!(h.models.requests("test/main").is_empty());
    assert!(h.state(&id).await.epoch.is_none());
    assert_eq!(h.state(&id).await.inbox[0].status, InputStatus::Pending);
    *h.tools.instruction_limits.lock().unwrap() = Ok(Default::default());
    h.runtime.wake(&id).await.unwrap();
    h.settle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
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
    // Regression: only Turn requests carried the prompt cache key.
    assert_eq!(summary_request.cache_key.as_deref(), Some(id.as_str()));
    assert_eq!(
        summary_request
            .routing
            .as_ref()
            .unwrap()
            .subagent
            .as_deref(),
        Some("compact")
    );

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
    // `provider-catalog` → GitHub Copilot subscription provider: compaction is the agent's.
    let summary = h.models.requests("test/summary");
    assert_eq!(summary[0].initiator, Some(cyber_llm::Initiator::Agent));
    assert_eq!(requests[1].initiator, Some(cyber_llm::Initiator::User));
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

/// `session-runtime` → Reasoning replay policy: a compaction summary carries no opaque
/// state, and the reasoning of the kept tail still replays natively.
#[tokio::test]
async fn compaction_summary_carries_no_opaque_state_and_the_tail_replays() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![
                    reasoning_then_text("old thought", "a1"),
                    reasoning_then_text("new thought", "a2"),
                    text("a3"),
                ],
            ),
            ("test/summary", vec![text("## Objective\nship it")]),
        ],
        compaction: small_tail(),
        ..Setup::default()
    });
    let id = h.session().await;
    for prompt in ["one", "two"] {
        h.runtime.admit(&id, admit(prompt)).await.unwrap();
        h.settle(&id).await;
    }
    h.runtime.compact(&id, None).await.unwrap();
    let summary_request = &h.models.requests("test/summary")[0];
    assert!(
        summary_request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .all(|part| part.opaque().is_none())
    );
    h.runtime.admit(&id, admit("three")).await.unwrap();
    h.settle(&id).await;
    let next = &h.models.requests("test/main")[2];
    assert!(
        next.messages[0]
            .content
            .iter()
            .all(|p| p.opaque().is_none())
    );
    assert!(!transcript(next).contains("old thought"));
    let tail: Vec<_> = next.messages[1..]
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|part| match part {
            Content::Reasoning { text, opaque } => Some((text.as_str(), opaque.is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(tail, [("new thought", true)]);
}

/// A successful operation does not authorize instruction loading with callbacks
/// captured before a configuration change while the operation was in flight.
#[tokio::test]
async fn nested_instructions_resolve_settings_after_the_successful_operation() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("read_one", "read", r#"{"path":"web/app.ts"}"#)]),
                text("done"),
                tools(&[("read_two", "read", r#"{"path":"web/app.ts"}"#)]),
                text("done"),
            ],
        )],
        ..Setup::default()
    });
    std::fs::create_dir(h.repo.join("web")).unwrap();
    std::fs::write(h.repo.join("web/CYBER.md"), "fresh operation instructions").unwrap();
    std::fs::write(h.repo.join("web/app.ts"), "contents").unwrap();
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Gated);
    let id = h.session().await;
    h.runtime.admit(&id, admit("first read")).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        h.tools.started.notified(),
    )
    .await
    .unwrap();
    *h.tools.instruction_limits.lock().unwrap() = Err("configuration changed during read".into());
    h.tools.release.notify_one();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert_eq!(state.calls["read_one"].output.as_deref(), Some("done"));
    assert!(state.epoch.unwrap().reminded_instructions.is_empty());
    *h.tools.instruction_limits.lock().unwrap() = Ok(Default::default());
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Return("contents".into()));
    h.runtime.admit(&id, admit("second read")).await.unwrap();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert!(
        state.calls["read_two"]
            .output
            .as_deref()
            .unwrap()
            .contains("fresh operation instructions")
    );
    assert_eq!(state.epoch.unwrap().reminded_instructions.len(), 1);
}

/// Same-path replacement must not load the new checkout's instruction files for
/// an operation captured against the old physical Location.
#[tokio::test]
async fn a_replaced_location_cannot_supply_instructions_for_an_inflight_read() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("read_old", "read", r#"{"path":"web/app.ts"}"#)]),
                text("done"),
                tools(&[("read_new", "read", r#"{"path":"web/app.ts"}"#)]),
                text("done"),
            ],
        )],
        ..Setup::default()
    });
    std::fs::create_dir(h.repo.join("web")).unwrap();
    std::fs::write(h.repo.join("web/app.ts"), "original file").unwrap();
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Gated);
    let id = h.session().await;
    h.runtime
        .admit(&id, admit("read old checkout"))
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        h.tools.started.notified(),
    )
    .await
    .unwrap();
    std::fs::rename(&h.repo, h.dir.path().join("old-checkout")).unwrap();
    std::fs::create_dir_all(h.repo.join("web")).unwrap();
    std::fs::create_dir(h.repo.join(".git")).unwrap();
    std::fs::write(h.repo.join("web/app.ts"), "replacement file").unwrap();
    std::fs::write(h.repo.join("web/CYBER.md"), "replacement checkout guidance").unwrap();
    h.tools.release.notify_one();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert_eq!(state.calls["read_old"].output.as_deref(), Some("done"));
    assert!(state.epoch.unwrap().reminded_instructions.is_empty());
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Return("replacement file".into()));
    h.runtime
        .admit(&id, admit("read new checkout"))
        .await
        .unwrap();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert!(
        state.calls["read_new"]
            .output
            .as_deref()
            .unwrap()
            .contains("replacement checkout guidance")
    );
    assert_eq!(state.epoch.unwrap().reminded_instructions.len(), 1);
}
