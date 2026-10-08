//! Hook admission, atomic terminal projections, privacy and interrupted owners.
mod support;
use cyber_core::config::{self, LoadRequest};
use cyber_core::hooks::{
    HookAction, HookCatalog, HookDecision, HookDefinition, HookEvent, HookIdentity, HookLocation,
    HookOutcome,
};
use cyber_core::paths::Paths;
use cyber_server::runtime::{HookExecutionIo, HookExecutionResult, HookExecutionStatus};
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};

fn definition(h: &Harness) -> HookDefinition {
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        h.dir.path().join("config").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, h.dir.path());
    paths.ensure().unwrap();
    std::fs::write(paths.config.join("cyber.jsonc"),json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"printf reviewed","id":"guard"}]}]}}).to_string()).unwrap();
    let resolved = config::load(&LoadRequest {
        location: &h.repo,
        paths: &paths,
        env: &env,
        home: h.dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap();
    HookCatalog::from_config(&resolved)
        .unwrap()
        .definitions
        .remove(0)
}
async fn event(h: &Harness, id: &str) -> HookEvent {
    let info = h.state(id).await.info;
    HookEvent::new(
        "PreToolUse",
        HookIdentity {
            session_id: id.into(),
            location: HookLocation {
                directory: info.directory.into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: info.agent,
            mode: info.mode,
        },
        1,
        json!({"tool_input":{"command":"private stdin"}})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap()
}
fn result() -> HookExecutionResult {
    HookExecutionResult {
        outcome: HookOutcome::Blocked,
        decision: HookDecision {
            decision: Some(HookAction::Deny),
            reason: Some("policy".into()),
            ..Default::default()
        },
        acknowledged: true,
        must_stop: false,
        io: Some(HookExecutionIo {
            stdin: "private stdin".into(),
            stdout: "private stdout".into(),
            stderr: "private stderr".into(),
            truncated: false,
        }),
    }
}

#[tokio::test]
async fn committed_start_and_terminal_receipts_survive_runtime_restart_without_raw_io() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), false)
        .await
        .unwrap();
    owner.verify(&h.runtime).unwrap();
    let id = owner.record().id.clone();
    let started = h.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].status, HookExecutionStatus::Running);
    assert_eq!(
        h.store.read_events(&id, -1, 10).unwrap().events[0].kind,
        "hook.started.1"
    );
    let settled = owner.finish(result()).unwrap();
    assert_eq!(settled.status, HookExecutionStatus::Completed);
    assert!(settled.io.is_none());
    assert_eq!(settled.outcome, Some(HookOutcome::Blocked));
    let replay = h.restart().hook_executions(&session, 10).unwrap();
    assert_eq!(
        replay[0].decision.as_ref().unwrap().reason.as_deref(),
        Some("policy")
    );
    let events = h.store.read_events(&id, -1, 10).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, "hook.executed.1");
    let encoded = serde_json::to_string(&events).unwrap();
    for text in ["private stdin", "private stdout", "private stderr"] {
        assert!(!encoded.contains(text));
    }
    // Independent execution aggregates must not advance the cached Session sequence.
    h.runtime.rename(&session, "after hook").await.unwrap();
}

#[tokio::test]
async fn admitted_io_policy_allows_bounded_capture_and_cannot_change_at_settlement() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), true)
        .await
        .unwrap();
    let id = owner.record().id.clone();
    let mut forged = serde_json::to_value(owner.record()).unwrap();
    forged["status"] = json!("completed");
    forged["duration_ms"] = json!(1);
    forged["outcome"] = json!("ok");
    forged["decision"] = json!({});
    forged["acknowledged"] = json!(true);
    forged["log_io"] = json!(false);
    assert!(
        h.store
            .append(
                &id,
                Expected::Seq(0),
                vec![NewEvent::new("hook.executed.1", forged)]
            )
            .is_err()
    );
    assert_eq!(h.store.read_events(&id, -1, 10).unwrap().events.len(), 1);
    let receipt = owner.finish(result()).unwrap();
    assert_eq!(receipt.io.unwrap().stdout, "private stdout");
    let duplicate = h.store.read_events(&id, -1, 10).unwrap().events[1]
        .data
        .clone();
    assert!(
        h.store
            .append(
                &id,
                Expected::Any,
                vec![NewEvent::new("hook.executed.1", duplicate)]
            )
            .is_err()
    );
    assert_eq!(h.store.read_events(&id, -1, 10).unwrap().events.len(), 2);
}

#[tokio::test]
async fn disposed_or_invalid_terminal_owner_records_unknown_without_success() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), true)
        .await
        .unwrap();
    drop(owner);
    let record = h.runtime.hook_executions(&session, 10).unwrap().remove(0);
    assert_eq!(record.status, HookExecutionStatus::Unknown);
    assert_eq!(record.outcome, Some(HookOutcome::Error));
    assert_eq!(record.acknowledged, Some(false));
    assert!(record.must_stop);
    assert!(record.io.is_none());
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), true)
        .await
        .unwrap();
    let id = owner.record().id.clone();
    let mut invalid = result();
    invalid.acknowledged = false;
    invalid.outcome = HookOutcome::Ok;
    assert!(owner.finish(invalid).is_err());
    let events = h.store.read_events(&id, -1, 10).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data["status"], "unknown");
    assert_eq!(events[1].data["outcome"], "error");
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), false)
        .await
        .unwrap();
    let id = owner.record().id.clone();
    let mut unacknowledged_allow = result();
    unacknowledged_allow.acknowledged = false;
    unacknowledged_allow.outcome = HookOutcome::Error;
    unacknowledged_allow.decision.decision = Some(HookAction::Allow);
    assert!(owner.finish(unacknowledged_allow).is_err());
    let events = h.store.read_events(&id, -1, 10).unwrap().events;
    assert_eq!(events[1].data["status"], "unknown");
    assert!(events[1].data["decision"]["decision"].is_null());
}

#[tokio::test]
async fn closed_scope_refuses_new_hook_admission_but_allows_terminal_receipts() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let event = event(&h, &session).await;
    let definition = definition(&h);
    let owner = h
        .runtime
        .start_hook_execution(&event, &definition, false)
        .await
        .unwrap();
    let original = h
        .store
        .read_events(&owner.record().id, -1, 10)
        .unwrap()
        .events[0]
        .data
        .clone();
    h.runtime.stop_subtree(&session).await.unwrap();
    assert!(owner.verify(&h.runtime).is_err());
    assert!(
        h.runtime
            .start_hook_execution(&event, &definition, false)
            .await
            .is_err()
    );
    // Verify the single writer independently refuses replay of old captured authority.
    let mut raced = original;
    raced["id"] = json!("hke_racing");
    assert!(
        h.store
            .append(
                "hke_racing",
                Expected::Seq(-1),
                vec![NewEvent::new("hook.started.1", raced)]
            )
            .is_err()
    );
    assert!(
        h.store
            .read_events("hke_racing", -1, 10)
            .unwrap()
            .events
            .is_empty()
    );
    assert!(owner.finish(result()).is_ok());
}

#[tokio::test]
async fn foreign_location_or_changed_definition_cannot_admit_a_hook() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let mut definition = definition(&h);
    definition.handler.command = Some("changed".into());
    let event = event(&h, &session).await;
    assert!(
        h.runtime
            .start_hook_execution(&event, &definition, false)
            .await
            .is_err()
    );
    let foreign = HookEvent::new(
        "PreToolUse",
        HookIdentity {
            location: HookLocation {
                directory: h.dir.path().into(),
                workspace: None,
            },
            ..event.identity().clone()
        },
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    assert!(
        h.runtime
            .start_hook_execution(&foreign, &self::definition(&h), false)
            .await
            .is_err()
    );
    assert!(h.runtime.hook_executions(&session, 10).unwrap().is_empty());
    assert!(h.runtime.hook_executions(&session, 0).is_err());
}

#[tokio::test]
async fn oversized_opt_in_io_rolls_back_terminal_projection_and_preserves_unknown_evidence() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition(&h), true)
        .await
        .unwrap();
    let id = owner.record().id.clone();
    let mut oversized = result();
    oversized.io.as_mut().unwrap().stdout = "x".repeat(1024 * 1024 + 1);
    assert!(owner.finish(oversized).is_err());
    let events = h.store.read_events(&id, -1, 10).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data["status"], "unknown");
    assert!(events[1].data.get("io").is_none());
}

#[tokio::test]
async fn once_admission_is_exclusive_across_runtimes_and_survives_unknown_restart() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let event = event(&h, &session).await;
    let mut definition = definition(&h);
    definition.handler.once = true;
    definition.digest = definition.handler.digest().unwrap();
    let peer = h.restart();
    let (left, right) = tokio::join!(
        h.runtime
            .try_start_hook_execution(&event, &definition, false),
        peer.try_start_hook_execution(&event, &definition, false)
    );
    let mut owners = [left.unwrap(), right.unwrap()];
    assert_eq!(owners.iter().filter(|owner| owner.is_some()).count(), 1);
    let mut owner = owners.iter_mut().find_map(Option::take).unwrap();
    owner.mark_launch(&h.runtime).unwrap();
    drop(owner);
    let replay = h.restart();
    assert!(
        replay
            .try_start_hook_execution(&event, &definition, false)
            .await
            .unwrap()
            .is_none()
    );
    let receipts = replay.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].once);
    assert_eq!(receipts[0].status, HookExecutionStatus::Unknown);
    // The same inspected handler remains eligible in a different Session.
    let other = h.session().await;
    let other_event = self::event(&h, &other).await;
    let owner = replay
        .try_start_hook_execution(&other_event, &definition, false)
        .await
        .unwrap()
        .unwrap();
    owner.finish(result()).unwrap();
}

#[tokio::test]
async fn once_receipt_cannot_change_its_claim_and_changed_digest_is_a_new_handler() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let event = event(&h, &session).await;
    let mut definition = definition(&h);
    definition.handler.once = true;
    definition.digest = definition.handler.digest().unwrap();
    let owner = h
        .runtime
        .start_hook_execution(&event, &definition, false)
        .await
        .unwrap();
    let mut terminal = serde_json::to_value(owner.record()).unwrap();
    terminal["once"] = json!(false);
    terminal["status"] = json!("completed");
    terminal["duration_ms"] = json!(1);
    terminal["outcome"] = json!("ok");
    terminal["decision"] = json!({});
    terminal["acknowledged"] = json!(true);
    assert!(
        h.store
            .append(
                &owner.record().id,
                Expected::Seq(0),
                vec![NewEvent::new("hook.executed.1", terminal)]
            )
            .is_err()
    );
    owner.finish(result()).unwrap();
    assert!(
        h.runtime
            .try_start_hook_execution(&event, &definition, false)
            .await
            .unwrap()
            .is_none()
    );
    definition.handler.command = Some("printf changed".into());
    definition.digest = definition.handler.digest().unwrap();
    h.runtime
        .start_hook_execution(&event, &definition, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    assert_eq!(h.runtime.hook_executions(&session, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn last_run_matches_checkout_digest_event_scope_and_keeps_unknown_without_io() {
    use cyber_core::hooks::HookRunStatus;
    use cyber_server::runtime::{CreateSession, hook_last_run};
    let h = Harness::new(Setup::default());
    let definition = definition(&h);
    let session = h.session().await;
    assert!(
        hook_last_run(&h.store, &h.repo, &definition)
            .unwrap()
            .is_none()
    );
    let completed = h
        .runtime
        .start_hook_execution(&event(&h, &session).await, &definition, true)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let summary = hook_last_run(&h.store, &h.repo, &definition)
        .unwrap()
        .unwrap();
    assert_eq!(summary.id, completed.id);
    assert!(matches!(summary.status, HookRunStatus::Completed));
    for secret in ["private stdin", "private stdout", "private stderr"] {
        assert!(!serde_json::to_string(&summary).unwrap().contains(secret));
    }
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let nested = h.repo.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: nested.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let owner = h
        .runtime
        .start_hook_execution(&event(&h, &child).await, &definition, true)
        .await
        .unwrap();
    let unknown = owner.record().id.clone();
    drop(owner);
    let other = h.dir.path().join("other-checkout");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let foreign = h
        .runtime
        .create_session(CreateSession {
            directory: other.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .start_hook_execution(&event(&h, &foreign).await, &definition, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let mut changed = definition.clone();
    changed.event = "PostToolUse".into();
    let post = HookEvent::new(
        "PostToolUse",
        event(&h, &session).await.identity().clone(),
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    h.runtime
        .start_hook_execution(&post, &changed, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let mut scoped = definition.clone();
    scoped.scope = cyber_core::hooks::HookScope::Local;
    h.runtime
        .start_hook_execution(&event(&h, &session).await, &scoped, false)
        .await
        .unwrap()
        .finish(result())
        .unwrap();
    let before: i64 = h
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    let last = hook_last_run(&h.store, &nested, &definition)
        .unwrap()
        .unwrap();
    assert_eq!(last.id, unknown);
    assert!(matches!(last.status, HookRunStatus::Unknown));
    assert!(last.must_stop);
    for secret in ["private stdin", "private stdout", "private stderr"] {
        assert!(!serde_json::to_string(&last).unwrap().contains(secret));
    }
    let mut new_digest = definition.clone();
    new_digest.digest = format!("sha256:{}", "b".repeat(64));
    assert!(
        hook_last_run(&h.store, &h.repo, &new_digest)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        hook_last_run(&h.store, &other, &definition)
            .unwrap()
            .unwrap()
            .session_id,
        foreign
    );
    let after: i64 = h
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(before, after);
}
