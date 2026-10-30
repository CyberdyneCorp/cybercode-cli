//! `session-runtime` → Usage limit wait and Usage limit wait recovery, end to end with a
//! scripted provider that answers `usage_limit_reached` and an injected wait clock, so no
//! test sleeps for the reset.

mod support;

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_core::budget::Budget;
use cyber_core::goals::{GoalStatus, Verdict};
use cyber_core::usage_limit::{JITTER_MS, MARGIN_MS, UsageLimitSettings};
use cyber_llm::ErrorKind;
use cyber_llm::adapters::ScriptStep;
use cyber_server::runtime::goals::GoalRequest;
use cyber_server::runtime::{
    Admission, CreateSession, Delivery, LiveEvent, Runtime, RuntimeError, WaitClock,
};
use futures::future::BoxFuture;
use serde_json::Value;
use support::*;

const MINUTE_MS: i64 = 60_000;

/// The host clock shifted by the time waits skipped. A holding clock never finishes a
/// sleep, so a wait lasts until it is interrupted or the runtime stops.
#[derive(Default)]
struct TestClock {
    offset_ms: AtomicI64,
    hold: AtomicBool,
    sleeps: Mutex<Vec<Duration>>,
}

impl TestClock {
    fn skipping() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn holding() -> Arc<Self> {
        let clock = Self::default();
        clock.hold.store(true, Ordering::SeqCst);
        Arc::new(clock)
    }
}

impl WaitClock for TestClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis() + self.offset_ms.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.sleeps.lock().unwrap().push(duration);
        if self.hold.load(Ordering::SeqCst) {
            return Box::pin(std::future::pending());
        }
        let skipped = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        self.offset_ms.fetch_add(skipped, Ordering::SeqCst);
        Box::pin(tokio::time::sleep(Duration::from_millis(1)))
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A usage limit that resets `minutes` from now.
fn limit(minutes: i64) -> Vec<ScriptStep> {
    vec![ScriptStep::UsageLimit {
        resets_at: (now_ms() + minutes * MINUTE_MS) / 1000,
    }]
}

fn harness(main: Vec<Vec<ScriptStep>>, clock: &Arc<TestClock>) -> Harness {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", main)],
        ..Setup::default()
    });
    h.runtime
        .configure_wait_clock(Arc::clone(clock) as Arc<dyn WaitClock>);
    h
}

/// A Session nobody attends, as `cyber exec` creates them.
async fn unattended(h: &Harness, budget: Option<Budget>) -> String {
    let req = CreateSession {
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        unattended: true,
        budget,
        ..Default::default()
    };
    h.runtime.create_session(req).await.unwrap().id
}

async fn prompt(runtime: &Runtime, id: &str, text: &str) {
    runtime
        .admit(id, Admission::text(text, Delivery::Steer))
        .await
        .unwrap();
}

/// The Session's `session.usage_limit.*` events, `(type, data)` in order.
fn waits(h: &Harness, id: &str) -> Vec<(String, Value)> {
    h.store
        .read_events(id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind.starts_with("session.usage_limit."))
        .map(|e| (e.kind, e.data))
        .collect()
}

fn kinds(waits: &[(String, Value)]) -> Vec<&str> {
    waits.iter().map(|(kind, _)| kind.as_str()).collect()
}

/// Wait until the Session records a pending wait.
async fn waiting(h: &Harness, id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !kinds(&waits(h, id)).contains(&"session.usage_limit.waiting.1") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no wait was recorded"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn errors(rx: &mut tokio::sync::broadcast::Receiver<LiveEvent>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let LiveEvent::Error { kind, message, .. } = event {
            found.push((kind, message));
        }
    }
    found
}

#[tokio::test]
async fn an_unattended_turn_waits_for_the_reset_and_resends_the_same_request() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(40), text("done after the reset")], &clock);
    let mut live = h.runtime.subscribe();
    let id = unattended(&h, None).await;
    let started = now_ms();
    prompt(&h.runtime, &id, "nightly task").await;
    h.settle(&id).await;

    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 2, "one new attempt after the wait");
    assert_eq!(requests[0], requests[1], "the retry sends the same request");
    let recorded = waits(&h, &id);
    assert_eq!(
        kinds(&recorded),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
    let wait = &recorded[0].1;
    let (resets, resume) = (
        wait["resets_at_ms"].as_i64().unwrap(),
        wait["resume_at_ms"].as_i64().unwrap(),
    );
    assert!(resets >= started + 39 * MINUTE_MS, "{wait}");
    assert!(
        (resets + MARGIN_MS..resets + MARGIN_MS + JITTER_MS).contains(&resume),
        "{wait}"
    );
    assert_eq!(wait["by"], "runtime");
    let slept = clock.sleeps.lock().unwrap()[0];
    assert!(slept >= Duration::from_secs(40 * 60), "{slept:?}");
    assert!(
        errors(&mut live).is_empty(),
        "a wait publishes no session.error"
    );
    let state = h.state(&id).await;
    assert!(state.open_step.is_none());
    let answer = serde_json::to_string(state.entries.last().unwrap()).unwrap();
    assert!(answer.contains("done after the reset"), "{answer}");
}

#[tokio::test]
async fn an_interactive_session_fails_and_can_resume_at_the_reset() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(20), text("resumed")], &clock);
    let mut live = h.runtime.subscribe();
    let id = h.session().await;
    prompt(&h.runtime, &id, "hello").await;
    h.settle(&id).await;
    let (kind, message) = errors(&mut live).pop().expect("the Turn failed");
    assert_eq!(kind, "quota_exceeded");
    assert!(
        message.contains("usage limit reached; resets at "),
        "{message}"
    );
    assert!(!message.contains("not waiting"), "{message}");
    assert!(
        waits(&h, &id).is_empty(),
        "an interactive Session does not wait"
    );
    assert_eq!(h.models.requests("test/main").len(), 1);

    let wait = h.runtime.resume_at_reset(&id).await.unwrap();
    assert_eq!(wait.by, cyber_server::runtime::WaitedBy::User);
    h.settle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(
        kinds(&waits(&h, &id)),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
    // Nothing left to resume after the Turn answered.
    let again = h.runtime.resume_at_reset(&id).await.unwrap_err();
    assert!(
        matches!(&again, RuntimeError::Invalid(m) if m.starts_with("nothing to resume")),
        "{again}"
    );
}

#[tokio::test]
async fn resume_at_reset_refuses_a_running_session_and_a_turn_that_did_not_hit_a_limit() {
    let clock = TestClock::holding();
    let h = harness(vec![text("fine"), limit(20)], &clock);
    let id = h.session().await;
    prompt(&h.runtime, &id, "hello").await;
    h.settle(&id).await;
    let refused = h.runtime.resume_at_reset(&id).await.unwrap_err();
    assert!(matches!(refused, RuntimeError::Invalid(_)), "{refused}");

    // A user-scheduled wait holds the Drain; a second request finds it running.
    let unattended = unattended(&h, None).await;
    prompt(&h.runtime, &unattended, "limit").await;
    waiting(&h, &unattended).await;
    let busy = h.runtime.resume_at_reset(&unattended).await.unwrap_err();
    assert!(matches!(busy, RuntimeError::Busy(_)), "{busy}");
    h.runtime.interrupt(&unattended).await.unwrap();
}

#[tokio::test]
async fn a_reset_beyond_max_wait_fails_and_says_why() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(7 * 60)], &clock);
    let mut live = h.runtime.subscribe();
    let id = unattended(&h, None).await;
    prompt(&h.runtime, &id, "hello").await;
    h.settle(&id).await;
    let (_, message) = errors(&mut live).pop().expect("the Turn failed");
    assert!(
        message.ends_with("(not waiting: the reset is more than usage_limit.max_wait (6h) away)"),
        "{message}"
    );
    assert!(waits(&h, &id).is_empty());
    // A shorter configured bound applies the same way.
    *h.tools.usage_limit.lock().unwrap() = UsageLimitSettings {
        max_wait_seconds: 30 * 60,
        ..UsageLimitSettings::default()
    };
    let h2 = harness(vec![limit(40)], &clock);
    *h2.tools.usage_limit.lock().unwrap() = h.tools.usage_limit.lock().unwrap().clone();
    let mut live = h2.runtime.subscribe();
    let id = unattended(&h2, None).await;
    prompt(&h2.runtime, &id, "hello").await;
    h2.settle(&id).await;
    let (_, message) = errors(&mut live).pop().expect("the Turn failed");
    assert!(message.contains("usage_limit.max_wait (30m)"), "{message}");
}

#[tokio::test]
async fn a_limit_without_a_reset_time_and_mode_never_fail_with_the_reason() {
    let clock = TestClock::skipping();
    let h = harness(
        vec![
            error(ErrorKind::QuotaExceeded, "insufficient_quota"),
            limit(5),
        ],
        &clock,
    );
    let mut live = h.runtime.subscribe();
    let id = unattended(&h, None).await;
    prompt(&h.runtime, &id, "one").await;
    h.settle(&id).await;
    let (_, message) = errors(&mut live).pop().expect("the Turn failed");
    assert!(
        message.ends_with("(not waiting: the provider did not say when the limit resets)"),
        "{message}"
    );
    *h.tools.usage_limit.lock().unwrap() = UsageLimitSettings {
        mode: cyber_core::usage_limit::WaitMode::Never,
        ..UsageLimitSettings::default()
    };
    prompt(&h.runtime, &id, "two").await;
    h.settle(&id).await;
    let (_, message) = errors(&mut live).pop().expect("the Turn failed");
    assert!(
        message.ends_with("(not waiting: usage_limit.mode is never)"),
        "{message}"
    );
    assert!(waits(&h, &id).is_empty());
}

#[tokio::test]
async fn an_interrupt_cancels_the_wait() {
    let clock = TestClock::holding();
    let h = harness(vec![limit(30), text("never sent")], &clock);
    let id = unattended(&h, None).await;
    prompt(&h.runtime, &id, "hello").await;
    waiting(&h, &id).await;
    assert!(h.runtime.is_running(&id), "the Drain holds the wait");
    assert!(h.state(&id).await.info.unattended);
    h.runtime.interrupt(&id).await.unwrap();
    h.settle(&id).await;
    let recorded = waits(&h, &id);
    assert_eq!(
        kinds(&recorded),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.abandoned.1"
        ]
    );
    assert_eq!(recorded[1].1["reason"], "interrupted");
    assert_eq!(h.models.requests("test/main").len(), 1);
}

/// Shutdown keeps the wait pending; the next server resumes it for the time left.
#[tokio::test]
async fn a_restart_resumes_a_pending_wait() {
    let clock = TestClock::holding();
    let h = harness(vec![limit(20), text("after the restart")], &clock);
    let id = unattended(&h, None).await;
    prompt(&h.runtime, &id, "hello").await;
    waiting(&h, &id).await;
    h.runtime.shutdown().await;
    assert_eq!(kinds(&waits(&h, &id)), ["session.usage_limit.waiting.1"]);

    let restarted = h.restart();
    restarted.configure_wait_clock(TestClock::skipping() as Arc<dyn WaitClock>);
    assert_eq!(restarted.recover_usage_limit_waits().await.unwrap(), 1);
    tokio::time::timeout(Duration::from_secs(30), restarted.wait_idle(&id))
        .await
        .unwrap();
    assert_eq!(
        kinds(&waits(&h, &id)),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(restarted.recover_usage_limit_waits().await.unwrap(), 0);
}

/// A subagent's owner recovers it, so a restart ends the child's wait instead.
#[tokio::test]
async fn a_restart_abandons_a_childs_wait() {
    let clock = TestClock::holding();
    let h = harness(vec![limit(20)], &clock);
    let root = unattended(&h, None).await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    prompt(&h.runtime, &child, "child work").await;
    waiting(&h, &child).await;
    h.runtime.shutdown().await;

    let restarted = h.restart();
    let mut live = restarted.subscribe();
    assert_eq!(restarted.recover_usage_limit_waits().await.unwrap(), 0);
    let recorded = waits(&h, &child);
    assert_eq!(recorded.last().unwrap().1["reason"], "restarted");
    assert!(!restarted.is_running(&child));
    let (kind, message) = next_error(&mut live).await;
    assert_eq!(kind, "quota_exceeded");
    assert!(message.contains("server restart"), "{message}");
}

#[tokio::test]
async fn a_session_wall_budget_before_the_reset_ends_the_run() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(30)], &clock);
    let mut live = h.runtime.subscribe();
    let budget = Budget {
        max_wall_seconds: Some(600.0),
        ..Budget::default()
    };
    let id = unattended(&h, Some(budget)).await;
    prompt(&h.runtime, &id, "hello").await;
    h.settle(&id).await;
    assert!(waits(&h, &id).is_empty(), "no wait past the deadline");
    let (kind, message) = errors(&mut live).pop().expect("the Drain failed");
    assert_eq!(kind, "budget_exceeded");
    assert!(message.contains("max_wall_seconds"), "{message}");
}

fn goal_harness(main: Vec<Vec<ScriptStep>>, clock: &Arc<TestClock>) -> Harness {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", main),
            (
                "test/evaluator",
                vec![text(r#"{"verdict": "met", "reason": "done"}"#)],
            ),
        ],
        roles: vec![("evaluator", "test/evaluator")],
        ..Setup::default()
    });
    h.runtime
        .configure_wait_clock(Arc::clone(clock) as Arc<dyn WaitClock>);
    h
}

/// `goals` → Usage limit waits under a Goal: the Goal is unattended, stays active
/// through the wait and is evaluated on the retried Turn.
#[tokio::test]
async fn a_goal_waits_through_a_usage_limit_and_is_then_evaluated() {
    let clock = TestClock::skipping();
    let h = goal_harness(vec![limit(20), text("all fixed")], &clock);
    let id = h.session().await;
    let goal = h
        .runtime
        .set_goal(
            &id,
            GoalRequest {
                condition: "tests pass".into(),
                ..GoalRequest::default()
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.settle(&id).await;
    let goals = h.state(&id).await.goals;
    let done = &goals.goals[&goal.goal_id];
    assert_eq!(done.status, GoalStatus::Met, "{done:?}");
    assert_eq!(done.evaluations[0].verdict, Verdict::Met);
    assert_eq!(
        kinds(&waits(&h, &id)),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
}

#[tokio::test]
async fn a_goal_wall_budget_before_the_reset_ends_the_goal() {
    let clock = TestClock::skipping();
    let h = goal_harness(vec![limit(20)], &clock);
    let id = h.session().await;
    let goal = h
        .runtime
        .set_goal(
            &id,
            GoalRequest {
                condition: "tests pass".into(),
                budget: Some(Budget {
                    max_wall_seconds: Some(600.0),
                    ..Budget::default()
                }),
                ..GoalRequest::default()
            },
        )
        .await
        .unwrap();
    h.settle(&id).await;
    assert!(waits(&h, &id).is_empty());
    let goals = h.state(&id).await.goals;
    let ended = &goals.goals[&goal.goal_id];
    assert_eq!(ended.status, GoalStatus::BudgetExceeded, "{ended:?}");
    assert!(
        ended
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("max_wall_seconds"),
        "{ended:?}"
    );
}

/// `max_wait` bounds the whole streak: a second limit after a wait is measured from
/// the first wait's start.
#[tokio::test]
async fn a_second_limit_past_max_wait_from_the_first_wait_fails() {
    let clock = TestClock::skipping();
    // The second reset is 70 minutes after the first wait started.
    let h = harness(vec![limit(40), limit(70)], &clock);
    *h.tools.usage_limit.lock().unwrap() = UsageLimitSettings {
        max_wait_seconds: 3600,
        ..UsageLimitSettings::default()
    };
    let mut live = h.runtime.subscribe();
    let id = unattended(&h, None).await;
    prompt(&h.runtime, &id, "hello").await;
    h.settle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(
        kinds(&waits(&h, &id)),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
    let (_, message) = errors(&mut live).pop().expect("the retry failed");
    assert!(message.contains("usage_limit.max_wait (1h)"), "{message}");
}

fn every_half_hour(budget: Option<Budget>) -> cyber_core::loops::LoopSpec {
    cyber_core::loops::LoopSpec {
        prompt: "check the deploy".into(),
        interval_seconds: Some(1800),
        budget,
        ..cyber_core::loops::LoopSpec::default()
    }
}

/// Wait until the loop's first iteration ended and the Session is idle.
async fn first_iteration(
    h: &Harness,
    id: &str,
    loop_id: &str,
) -> cyber_server::runtime::loops::LoopState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        h.settle(id).await;
        let state = h.state(id).await.loops[loop_id].clone();
        if !state.history.is_empty() || !state.active() {
            return state;
        }
        assert!(tokio::time::Instant::now() < deadline, "{state:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `loops-scheduling` → Usage limit waits in loop iterations: an iteration in flight
/// makes an interactive Session unattended, so it waits and ends normally.
#[tokio::test]
async fn a_loop_iteration_waits_for_the_reset_and_completes() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(10), text("deploy is green")], &clock);
    let id = h.session().await;
    let created = h
        .runtime
        .create_loop(&id, every_half_hour(None))
        .await
        .unwrap();
    let state = first_iteration(&h, &id, &created.loop_id).await;
    assert_eq!(
        state.history[0].outcome,
        cyber_server::runtime::loops::Outcome::Completed,
        "{state:?}"
    );
    assert!(state.active());
    assert_eq!(
        kinds(&waits(&h, &id)),
        [
            "session.usage_limit.waiting.1",
            "session.usage_limit.resumed.1"
        ]
    );
}

#[tokio::test]
async fn a_loop_wall_budget_before_the_reset_stops_the_loop() {
    let clock = TestClock::skipping();
    let h = harness(vec![limit(20)], &clock);
    let id = h.session().await;
    let budget = Budget {
        max_wall_seconds: Some(600.0),
        ..Budget::default()
    };
    let created = h
        .runtime
        .create_loop(&id, every_half_hour(Some(budget)))
        .await
        .unwrap();
    let state = first_iteration(&h, &id, &created.loop_id).await;
    assert!(!state.active(), "{state:?}");
    assert_eq!(state.status, "budget_exceeded", "{state:?}");
    assert!(waits(&h, &id).is_empty());
}
