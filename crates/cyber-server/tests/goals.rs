//! `goals` end to end with a scripted model and a scripted evaluator.

mod support;

use std::time::Duration;

use cyber_core::budget::Budget;
use cyber_core::goals::{CONDITION_TOO_LONG, GoalStatus, Verdict};
use cyber_llm::ErrorKind;
use cyber_server::runtime::RuntimeError;
use cyber_server::runtime::goals::GoalRequest;
use support::*;

fn verdict(v: &str, reason: &str) -> Vec<cyber_llm::adapters::ScriptStep> {
    text(&serde_json::json!({"verdict": v, "reason": reason}).to_string())
}

fn harness(
    main: Vec<Vec<cyber_llm::adapters::ScriptStep>>,
    evals: Vec<Vec<cyber_llm::adapters::ScriptStep>>,
) -> Harness {
    Harness::new(Setup {
        scripts: vec![("test/main", main), ("test/evaluator", evals)],
        roles: vec![("evaluator", "test/evaluator")],
        ..Setup::default()
    })
}

fn goal(condition: &str) -> GoalRequest {
    GoalRequest {
        condition: condition.into(),
        ..GoalRequest::default()
    }
}

/// Settle until no Drain runs and no goal continuation is pending.
async fn quiet(h: &Harness, id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        h.settle(id).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        if !h.runtime.is_running(id) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Session never went quiet"
        );
    }
}

fn user_texts(h: &Harness, request: usize) -> String {
    transcript(&h.models.requests("test/main")[request])
}

#[tokio::test]
async fn not_met_continues_with_the_reason_then_met_completes() {
    let h = harness(
        vec![text("first try"), text("fixed it")],
        vec![
            verdict("not_met", "3 tests still failing in auth_test.rs"),
            verdict("met", "all green"),
        ],
    );
    let id = h.session().await;
    let set = h
        .runtime
        .set_goal(&id, goal("cargo test passes"))
        .await
        .unwrap();
    assert_eq!(set.status, GoalStatus::Active);
    quiet(&h, &id).await;

    let goals = h.state(&id).await.goals;
    let done = &goals.goals[&set.goal_id];
    assert_eq!(done.status, GoalStatus::Met);
    assert_eq!((done.turns, done.eval_count), (2, 2));
    assert_eq!(done.evaluations[0].verdict, Verdict::NotMet);
    assert!(goals.current.is_none());
    assert!(
        user_texts(&h, 0).contains("cargo test passes"),
        "the goal prompt starts the work"
    );
    assert!(user_texts(&h, 1).contains("3 tests still failing in auth_test.rs"));
    // Evaluator usage is billed to the Session as a hidden call.
    assert!(done.eval_tokens > 0);
    let state = h.state(&id).await;
    assert!(state.totals.usage.input >= done.eval_tokens);
}

#[tokio::test]
async fn a_queue_runs_a_then_b_then_c_without_user_input() {
    let h = harness(
        vec![text("A done"), text("B done"), text("C done")],
        vec![
            verdict("met", "a"),
            verdict("met", "b"),
            verdict("met", "c"),
        ],
    );
    let id = h.session().await;
    let a = h.runtime.set_goal(&id, goal("A")).await.unwrap();
    let b = h.runtime.add_goal(&id, goal("B")).await.unwrap();
    let c = h.runtime.add_goal(&id, goal("C")).await.unwrap();
    assert_eq!(b.status, GoalStatus::Pending);
    quiet(&h, &id).await;
    let goals = h.state(&id).await.goals;
    for g in [&a, &b, &c] {
        assert_eq!(
            goals.goals[&g.goal_id].status,
            GoalStatus::Met,
            "{}",
            g.condition
        );
    }
    assert!(goals.queue.is_empty() && goals.current.is_none());
    assert_eq!(h.models.requests("test/main").len(), 3);
}

#[tokio::test]
async fn add_activates_when_no_goal_is_current() {
    let h = harness(vec![text("done")], vec![verdict("met", "ok")]);
    let id = h.session().await;
    let added = h.runtime.add_goal(&id, goal("lint passes")).await.unwrap();
    assert_eq!(added.status, GoalStatus::Active);
    quiet(&h, &id).await;
    assert_eq!(
        h.state(&id).await.goals.goals[&added.goal_id].status,
        GoalStatus::Met
    );
}

#[tokio::test]
async fn a_condition_over_4000_characters_is_rejected() {
    let h = harness(Vec::new(), Vec::new());
    let id = h.session().await;
    match h.runtime.set_goal(&id, goal(&"x".repeat(5000))).await {
        Err(RuntimeError::GoalCondition(message)) => assert_eq!(message, CONDITION_TOO_LONG),
        other => panic!("{other:?}"),
    }
    assert!(h.state(&id).await.goals.goals.is_empty());
}

#[tokio::test]
async fn a_failing_check_turns_met_into_not_met_with_its_output() {
    let h = harness(
        vec![text("I think it works"), text("now really fixed")],
        vec![verdict("met", "looks done"), verdict("met", "done")],
    );
    h.tools
        .goal_check_exits
        .lock()
        .unwrap()
        .extend([Some(101), Some(0)]);
    let id = h.session().await;
    let request = GoalRequest {
        check: Some("cargo test".into()),
        ..goal("tests pass")
    };
    let set = h.runtime.set_goal(&id, request).await.unwrap();
    quiet(&h, &id).await;
    let g = &h.state(&id).await.goals.goals[&set.goal_id];
    assert_eq!(g.status, GoalStatus::Met);
    assert_eq!(g.evaluations[0].verdict, Verdict::NotMet);
    assert_eq!(
        g.evaluations[0].reason,
        "check `cargo test` failed (exit 101)"
    );
    assert_eq!(
        g.evaluations[0].check.as_ref().unwrap().exit_code,
        Some(101)
    );
    assert!(
        user_texts(&h, 1).contains("exit 101"),
        "the continuation carries the check output"
    );
    assert_eq!(h.tools.checks.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn the_turn_budget_stops_the_goal() {
    let h = harness(
        vec![text("1"), text("2")],
        vec![verdict("not_met", "more"), verdict("not_met", "still more")],
    );
    let id = h.session().await;
    let request = GoalRequest {
        budget: Some(Budget {
            max_turns: Some(2),
            ..Budget::default()
        }),
        ..goal("ship it")
    };
    let set = h.runtime.set_goal(&id, request).await.unwrap();
    quiet(&h, &id).await;
    let g = &h.state(&id).await.goals.goals[&set.goal_id];
    assert_eq!(g.status, GoalStatus::BudgetExceeded);
    assert_eq!(g.reason.as_deref(), Some("max_turns 2/2"));
    assert_eq!(h.models.requests("test/main").len(), 2);
}

#[tokio::test]
async fn impossible_stops_the_queue_by_default() {
    let h = harness(
        vec![text("cannot")],
        vec![verdict("impossible", "needs prod access")],
    );
    let id = h.session().await;
    let a = h
        .runtime
        .set_goal(&id, goal("deploy to prod"))
        .await
        .unwrap();
    let b = h.runtime.add_goal(&id, goal("then this")).await.unwrap();
    quiet(&h, &id).await;
    let goals = h.state(&id).await.goals;
    assert_eq!(goals.goals[&a.goal_id].status, GoalStatus::Impossible);
    assert_eq!(goals.goals[&b.goal_id].status, GoalStatus::Pending);
    assert!(goals.current.is_none());
}

#[tokio::test]
async fn an_authentication_failure_pauses_the_goal() {
    let h = harness(
        vec![error(ErrorKind::Authentication, "invalid API key")],
        Vec::new(),
    );
    let id = h.session().await;
    let set = h.runtime.set_goal(&id, goal("anything")).await.unwrap();
    quiet(&h, &id).await;
    let g = &h.state(&id).await.goals.goals[&set.goal_id];
    assert_eq!(g.status, GoalStatus::Paused);
    assert!(
        g.reason.as_deref().unwrap().starts_with("authentication"),
        "{:?}",
        g.reason
    );
    assert!(
        h.models.requests("test/evaluator").is_empty(),
        "nothing is evaluated"
    );
}

#[tokio::test]
async fn pause_stops_continuation_and_resume_continues() {
    let h = harness(
        vec![tools(&[("c1", "write", "{}")]), text("half"), text("all")],
        vec![verdict("met", "done")],
    );
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    let set = h.runtime.set_goal(&id, goal("finish")).await.unwrap();
    h.tools.started.notified().await;
    let paused = h.runtime.pause_goal(&id).await.unwrap();
    assert_eq!(paused.status, GoalStatus::Paused);
    h.tools.release.notify_one();
    quiet(&h, &id).await;
    assert!(
        h.models.requests("test/evaluator").is_empty(),
        "a paused goal is not evaluated"
    );
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Paused
    );

    h.runtime.resume_goal(&id).await.unwrap();
    quiet(&h, &id).await;
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Met
    );
}

#[tokio::test]
async fn user_input_during_a_goal_keeps_it_active() {
    let h = harness(
        vec![
            tools(&[("c1", "write", "{}")]),
            text("used the hint"),
            text("done"),
        ],
        vec![verdict("met", "done")],
    );
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    let set = h.runtime.set_goal(&id, goal("finish")).await.unwrap();
    h.tools.started.notified().await;
    h.runtime
        .admit(
            &id,
            cyber_server::runtime::Admission::text(
                "hint: look at auth.rs",
                cyber_server::runtime::Delivery::Steer,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        h.state(&id).await.goals.current.as_deref(),
        Some(set.goal_id.as_str())
    );
    h.tools.release.notify_one();
    quiet(&h, &id).await;
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Met
    );
    assert!(user_texts(&h, 1).contains("hint: look at auth.rs"));
}

#[tokio::test]
async fn the_model_sees_the_goal_as_a_system_context_message() {
    let h = harness(vec![text("done")], vec![verdict("met", "ok")]);
    let id = h.session().await;
    h.runtime
        .set_goal(&id, goal("docs build cleanly"))
        .await
        .unwrap();
    quiet(&h, &id).await;
    let request = &h.models.requests("test/main")[0];
    let system = request.system.join("\n");
    assert!(
        system.contains("Active goal") && system.contains("docs build cleanly"),
        "{system}"
    );
}

#[tokio::test]
async fn reorder_publishes_the_new_order_and_clear_ends_goals() {
    let h = harness(vec![tools(&[("c1", "write", "{}")]), text("x")], Vec::new());
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    h.runtime.set_goal(&id, goal("A")).await.unwrap();
    h.tools.started.notified().await;
    let b = h.runtime.add_goal(&id, goal("B")).await.unwrap();
    let c = h.runtime.add_goal(&id, goal("C")).await.unwrap();
    let order = vec![c.goal_id.clone(), b.goal_id.clone()];
    h.runtime.reorder_goals(&id, order.clone()).await.unwrap();
    assert_eq!(h.state(&id).await.goals.queue, order);
    assert!(
        h.runtime
            .reorder_goals(&id, vec![b.goal_id.clone()])
            .await
            .is_err()
    );
    let cleared = h.runtime.clear_goals(&id, None).await.unwrap();
    assert_eq!(cleared.len(), 3);
    h.tools.release.notify_one();
    quiet(&h, &id).await;
    let goals = h.state(&id).await.goals;
    assert!(
        goals
            .goals
            .values()
            .all(|g| g.status == GoalStatus::Cleared)
    );
}

#[tokio::test]
async fn a_restart_evaluates_the_active_goal_before_continuing() {
    for (answer, continues) in [("met", false), ("not_met", true)] {
        let h = harness(
            vec![
                tools(&[("c1", "write", "{}")]),
                text("before restart"),
                text("after restart"),
            ],
            vec![
                verdict(answer, "judged after restart"),
                verdict("met", "now done"),
            ],
        );
        h.tools.set("write", Behavior::Gated);
        let id = h.session().await;
        let set = h
            .runtime
            .set_goal(&id, goal("survive restarts"))
            .await
            .unwrap();
        h.tools.started.notified().await;
        // The server stops mid-Turn; a new runtime over the same store takes over.
        let restarted = h.restart();
        restarted.recover_goals().await.unwrap();
        tokio::time::timeout(Duration::from_secs(30), restarted.wait_idle(&id))
            .await
            .unwrap();
        let g = restarted.state(&id).await.unwrap().goals.goals[&set.goal_id].clone();
        assert_eq!(g.evaluations[0].reason, "judged after restart", "{answer}");
        assert_eq!(g.status, GoalStatus::Met, "{answer}");
        // `met` completes on the restored evaluation; `not_met` runs one more Turn first.
        let evaluations = if continues { 2 } else { 1 };
        assert_eq!(g.evaluations.len(), evaluations, "{answer}");
        h.tools.release.notify_one();
    }
}

#[tokio::test]
async fn a_goal_never_raises_the_mode() {
    let h = harness(vec![text("done")], vec![verdict("met", "ok")]);
    let id = h.session().await;
    let before = h.state(&id).await.info.mode;
    h.runtime
        .set_goal(&id, goal("refactor the parser"))
        .await
        .unwrap();
    quiet(&h, &id).await;
    assert_eq!(h.state(&id).await.info.mode, before);
}

#[tokio::test]
async fn a_background_job_defers_evaluation_until_its_handback() {
    let h = harness(
        vec![text("waiting for the tests"), text("tests finished green")],
        vec![verdict("met", "tests are green")],
    );
    let id = h.session().await;
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(id.clone()),
            title: Some("cargo test".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    h.runtime
        .start_child_job(
            &id,
            &child,
            "tests".into(),
            "Run the test suite".into(),
            None,
            Box::pin(async move {
                let _ = finished.await;
                Ok(serde_json::json!({"text": "all tests passed"}))
            }),
        )
        .await
        .unwrap();
    let set = h
        .runtime
        .set_goal(&id, goal("the test suite passes"))
        .await
        .unwrap();
    quiet(&h, &id).await;
    assert!(
        h.models.requests("test/evaluator").is_empty(),
        "no verdict while the job runs"
    );
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Active
    );

    finish.send(()).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        quiet(&h, &id).await;
        if h.state(&id).await.goals.goals[&set.goal_id].status == GoalStatus::Met {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the goal was never evaluated"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(h.models.requests("test/evaluator").len(), 1);
}

async fn wait_status(h: &Harness, id: &str, goal_id: &str, status: GoalStatus) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.state(id).await.goals.goals[goal_id].status != status {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the goal never became {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The notification events of a Session, as `title: body`.
fn notices(h: &Harness, id: &str) -> Vec<String> {
    h.store
        .read_events(id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "session.notification.sent.1")
        .map(|e| format!("{}: {}", e.data["title"], e.data["body"]))
        .collect()
}

/// `goals` → Goal mode safety: a goal Turn's permission request that no client watches
/// for `unattended_ask_seconds` pauses the Goal with `Waiting for approval` and a
/// notification naming the request; the request stays pending, and answering it ends
/// the Turn without continuing the paused Goal. While a client watches, nothing pauses.
#[tokio::test]
async fn unattended_ask_pauses_goal_waiting_for_approval() {
    let h = harness(
        vec![tools(&[("c1", "shell", "{}")]), text("cleaned")],
        vec![],
    );
    h.tools.goal_settings.lock().unwrap().unattended_ask_seconds = 1;
    h.tools.set("shell", Behavior::Ask("rm -rf build".into()));
    let id = h.session().await;
    let watcher = h
        .runtime
        .attach_client(cyber_server::runtime::presence::Watching::Session(
            id.clone(),
        ));
    let set = h
        .runtime
        .set_goal(&id, goal("the build is clean"))
        .await
        .unwrap();
    let request = loop {
        if let Some(r) = h.runtime.pending_requests(Some(&id)).first().cloned() {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Active,
        "a watched Session's ask does not pause the goal"
    );

    drop(watcher);
    wait_status(&h, &id, &set.goal_id, GoalStatus::Paused).await;
    let state = h.state(&id).await;
    let g = &state.goals.goals[&set.goal_id];
    assert_eq!(g.reason.as_deref(), Some("Waiting for approval"));
    assert_eq!(h.runtime.pending_requests(Some(&id))[0].id, request.id);
    let notes = notices(&h, &id);
    assert!(
        notes.iter().any(|n| n.contains("waiting for approval")
            && n.contains(&request.id)
            && n.contains("rm -rf build")),
        "{notes:?}"
    );

    h.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    quiet(&h, &id).await;
    assert!(h.models.requests("test/evaluator").is_empty());
    assert_eq!(
        h.state(&id).await.goals.goals[&set.goal_id].status,
        GoalStatus::Paused
    );
}
