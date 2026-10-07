//! Exercise durable subtree boundaries through real provider dispatch.
mod support;
use cyber_core::budget::{Budget, Enforcement};
use cyber_server::runtime::{Admission, CreateSession, Delivery};
use serde_json::{Value, json};
use support::{Harness, Setup, text};

async fn create(h: &Harness, parent: Option<&str>, budget: Option<Budget>) -> String {
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: parent.map(str::to_owned),
            budget,
            title: Some("Budget fixture".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}
async fn prompt(h: &Harness, id: &str) {
    h.runtime
        .admit(id, Admission::text("continue", Delivery::Steer))
        .await
        .unwrap();
    h.settle(id).await;
}
fn signals(h: &Harness, id: &str, kind: &str) -> Vec<Value> {
    h.store
        .read_events(id, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.data)
        .collect()
}
fn setup() -> Setup {
    Setup {
        scripts: vec![(
            "test/main",
            vec![text("first"), text("second"), text("third"), text("fourth")],
        )],
        ..Setup::default()
    }
}
#[tokio::test]
async fn recorded_token_limit_stops_next_call_and_persists_after_restart() {
    let h = Harness::new(setup());
    let id = create(
        &h,
        None,
        Some(Budget {
            max_tokens: Some(110),
            ..Default::default()
        }),
    )
    .await;
    prompt(&h, &id).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert_eq!(signals(&h, &id, "budget.exceeded.1").len(), 1);
    prompt(&h, &id).await;
    let restarted = h.restart();
    restarted
        .admit(&id, Admission::text("after restart", Delivery::Steer))
        .await
        .unwrap();
    restarted.wait_idle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert_eq!(signals(&h, &id, "budget.exceeded.1").len(), 1);
    assert_eq!(
        restarted
            .state(&id)
            .await
            .unwrap()
            .info
            .budget
            .unwrap()
            .max_tokens,
        Some(110)
    );
}
#[tokio::test]
async fn nested_descendants_and_deleted_child_turns_keep_parent_exhausted() {
    let h = Harness::new(setup());
    let root = create(
        &h,
        None,
        Some(Budget {
            max_turns: Some(1),
            ..Default::default()
        }),
    )
    .await;
    let child = create(&h, Some(&root), None).await;
    let nested = create(&h, Some(&child), None).await;
    prompt(&h, &nested).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert_eq!(
        signals(&h, &nested, "budget.exceeded.1")[0]["scope_id"],
        root
    );
    h.runtime.delete(&child).await.unwrap();
    prompt(&h, &root).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert_eq!(
        signals(&h, &root, "budget.exceeded.1").len(),
        0,
        "scope signal is not repeated on another Session"
    );
}
#[tokio::test]
async fn child_cannot_widen_ancestor_cost_budget_and_unrelated_scopes_continue() {
    let h = Harness::new(setup());
    let root = create(
        &h,
        None,
        Some(Budget {
            max_cost_usd: Some(0.00012),
            ..Default::default()
        }),
    )
    .await;
    let child = create(
        &h,
        Some(&root),
        Some(Budget {
            max_cost_usd: Some(100.0),
            ..Default::default()
        }),
    )
    .await;
    prompt(&h, &child).await;
    prompt(&h, &child).await;
    let unrelated = create(&h, None, None).await;
    prompt(&h, &unrelated).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(
        signals(&h, &child, "budget.exceeded.1")[0]["scope_id"],
        root
    );
}
#[tokio::test]
async fn eighty_percent_warning_is_durable_and_not_repeated() {
    let h = Harness::new(setup());
    let id = create(
        &h,
        None,
        Some(Budget {
            max_tokens: Some(137),
            ..Default::default()
        }),
    )
    .await;
    prompt(&h, &id).await;
    assert_eq!(signals(&h, &id, "budget.warned.1").len(), 1);
    assert_eq!(signals(&h, &id, "budget.exceeded.1").len(), 0);
    prompt(&h, &id).await;
    assert_eq!(signals(&h, &id, "budget.warned.1").len(), 1);
    assert_eq!(signals(&h, &id, "budget.exceeded.1").len(), 1);
    assert_eq!(
        h.models.requests("test/main").len(),
        2,
        "soft scope lets the in-flight call settle with disclosed overshoot"
    );
}
#[tokio::test]
async fn elapsed_wall_activation_survives_restart_and_does_not_reset_on_input() {
    let h = Harness::new(setup());
    let id = create(
        &h,
        None,
        Some(Budget {
            max_wall_seconds: Some(10.0),
            ..Default::default()
        }),
    )
    .await;
    prompt(&h, &id).await;
    let scope = id.clone();
    h.store
        .transaction(move |tx| {
            tx.execute(
                "UPDATE session_budget SET activated_ms=activated_ms-11000 WHERE session_id=?1",
                [scope],
            )?;
            Ok(())
        })
        .unwrap();
    let restarted = h.restart();
    restarted
        .admit(&id, Admission::text("again", Delivery::Steer))
        .await
        .unwrap();
    restarted.wait_idle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert_eq!(
        signals(&h, &id, "budget.exceeded.1")[0]["limit"],
        "max_wall_seconds"
    );
}
#[tokio::test]
async fn reserved_budgets_are_refused_without_a_soft_fallback() {
    let h = Harness::new(setup());
    let result = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            budget: Some(Budget {
                max_tokens: Some(10),
                enforcement: Enforcement::Reserved,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Reserved Session budget enforcement is not available")
    );
    assert_eq!(
        h.runtime.list(&Default::default()).unwrap().sessions.len(),
        0
    );
}
#[tokio::test]
async fn incomplete_historical_child_accounting_refuses_provider_dispatch() {
    let h = Harness::new(setup());
    let id = create(
        &h,
        None,
        Some(Budget {
            max_cost_usd: Some(100.0),
            ..Default::default()
        }),
    )
    .await;
    let scope = id.clone();
    h.store
        .transaction(move |tx| {
            tx.execute(
                "UPDATE session SET children_usage_complete=0 WHERE id=?1",
                [scope],
            )?;
            Ok(())
        })
        .unwrap();
    prompt(&h, &id).await;
    assert_eq!(h.models.requests("test/main").len(), 0);
}
#[tokio::test]
async fn independent_fork_copies_budget_but_does_not_copy_billing_or_activation() {
    let h = Harness::new(setup());
    let id = create(
        &h,
        None,
        Some(Budget {
            max_tokens: Some(110),
            ..Default::default()
        }),
    )
    .await;
    prompt(&h, &id).await;
    let fork = h.runtime.fork(&id, None).await.unwrap();
    assert_eq!(fork.budget.as_ref().unwrap().max_tokens, Some(110));
    prompt(&h, &fork.id).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(h.state(&fork.id).await.totals.usage.input, 100);
}
#[tokio::test]
async fn zero_cap_blocks_visible_turns_and_background_title_requests() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("main")]),
            ("test/title", vec![text("title")]),
        ],
        roles: vec![("title", "test/title")],
        ..Setup::default()
    });
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            budget: Some(Budget {
                max_tokens: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    prompt(&h, &id).await;
    h.runtime.shutdown().await;
    assert_eq!(h.models.requests("test/main").len(), 0);
    assert_eq!(h.models.requests("test/title").len(), 0);
}
#[tokio::test]
async fn exhausted_scope_blocks_hidden_evaluator_calls() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/evaluator",
            vec![text(r#"{"decision":"allow","reason":"safe"}"#)],
        )],
        roles: vec![("evaluator", "test/evaluator")],
        ..Setup::default()
    });
    let id = create(
        &h,
        None,
        Some(Budget {
            max_cost_usd: Some(0.0),
            ..Default::default()
        }),
    )
    .await;
    let asker = h.runtime.operation_asker(&id).await.unwrap();
    let decision = asker
        .review_auto(
            cyber_server::runtime::AutoReview {
                action: "read".into(),
                tool: "read".into(),
                policy: "Only read this file".into(),
                resources: vec!["file".into()],
                input: json!({"path":"file"}),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(decision.reason.contains("BudgetExceededError"));
    assert_eq!(h.models.requests("test/evaluator").len(), 0);
}
#[tokio::test]
async fn exhausted_child_scope_blocks_hidden_compaction() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("first"), text("second")]),
            ("test/summary", vec![text("summary")]),
        ],
        roles: vec![("compaction", "test/summary")],
        compaction: cyber_server::runtime::CompactionConfig {
            auto: false,
            keep_tokens: 10,
            keep_turns: Some(1),
            ..Default::default()
        },
        ..Setup::default()
    });
    let root = create(&h, None, None).await;
    prompt(&h, &root).await;
    prompt(&h, &root).await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            fork_from: Some(root),
            title: Some("Child".into()),
            budget: Some(Budget {
                max_tokens: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(h.runtime.compact(&child.id, None).await.is_err());
    assert_eq!(
        signals(&h, &child.id, "budget.exceeded.1")[0]["limit"],
        "max_tokens"
    );
    assert_eq!(h.models.requests("test/summary").len(), 0);
}

#[tokio::test]
async fn tool_auxiliary_usage_is_replayed_and_exhausts_ancestor_budget() {
    let h = Harness::new(setup());
    let root = create(
        &h,
        None,
        Some(Budget {
            max_tokens: Some(24),
            ..Default::default()
        }),
    )
    .await;
    let child = create(&h, Some(&root), None).await;
    h.runtime
        .operation_asker(&child)
        .await
        .unwrap()
        .record_model_usage(cyber_server::runtime::AuxiliaryUsage {
            provider: "test".into(),
            model: "test/summary".into(),
            purpose: "webfetch_summary".into(),
            call_id: Some("call_webfetch".into()),
            duration_ms: 1,
            usage: cyber_llm::Usage {
                input: 10,
                output: 2,
                reasoning: 3,
                cache_read: 4,
                cache_write: 5,
            },
            cost: None,
        })
        .await
        .unwrap();
    assert_eq!(h.runtime.children_usage(&root).unwrap().children_tokens, 24);
    assert_eq!(
        h.runtime
            .children_usage(&root)
            .unwrap()
            .children_unpriced_steps,
        1
    );
    let replay = h.restart().state(&child).await.unwrap();
    assert_eq!(replay.totals.usage.cache_write, 5);
    assert_eq!(replay.totals.unpriced_steps, 1);
    prompt(&h, &child).await;
    assert_eq!(h.models.requests("test/main").len(), 0);
    assert_eq!(
        signals(&h, &child, "budget.exceeded.1")[0]["scope_id"],
        root
    );
}

#[tokio::test]
async fn discarded_empty_title_keeps_its_billing_and_default_title() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("main")]),
            ("test/title", vec![text(" ")]),
        ],
        roles: vec![("title", "test/title")],
        ..Setup::default()
    });
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            budget: Some(Budget {
                max_tokens: Some(1000),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    prompt(&h, &id).await;
    for _ in 0..50 {
        if !signals(&h, &id, "usage.recorded.1").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let state = h.state(&id).await;
    assert!(state.info.default_title);
    assert_eq!(state.totals.usage.input, 200);
    assert_eq!(state.totals.steps, 1);
    assert_eq!(
        signals(&h, &id, "usage.recorded.1")[0]["purpose"],
        "discarded_title"
    );
}
