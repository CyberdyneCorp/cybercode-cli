//! `loops-scheduling` (Session loops), end to end with a scripted model. Time is passed to
//! the sweep explicitly; the first iteration is admitted by the nudge after creation.

mod support;

use std::time::Duration;

use cyber_core::budget::Budget;
use cyber_core::loops::{LoopSpec, Missed, Overlap};
use cyber_server::runtime::loops::{LoopState, Outcome};
use cyber_server::runtime::{InputStatus, RuntimeError};
use support::*;

fn every(seconds: u64, prompt: &str) -> LoopSpec {
    LoopSpec {
        prompt: prompt.into(),
        interval_seconds: Some(seconds),
        ..LoopSpec::default()
    }
}

async fn loop_state(h: &Harness, id: &str, loop_id: &str) -> LoopState {
    h.state(id).await.loops[loop_id].clone()
}

/// Wait until `done` holds for the loop and the Session is idle.
async fn wait_for(
    h: &Harness,
    id: &str,
    loop_id: &str,
    done: impl Fn(&LoopState) -> bool,
) -> LoopState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        h.settle(id).await;
        let state = loop_state(h, id, loop_id).await;
        if done(&state) {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "loop never reached the state: {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_interval_loop_runs_now_and_again_every_interval() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("checked 1"), text("checked 2")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let created = h
        .runtime
        .create_loop(&id, every(300, "check the deploy status"))
        .await
        .unwrap();
    assert_eq!(created.next_due_ms, Some(created.created_ms));

    let first = wait_for(&h, &id, &created.loop_id, |l| l.history.len() == 1).await;
    assert_eq!(first.iterations, 1);
    assert_eq!(first.history[0].outcome, Outcome::Completed);
    let tick = created.created_ms + 300_000;
    assert_eq!(first.next_due_ms, Some(tick));
    let state = h.state(&id).await;
    let row = state
        .inbox
        .iter()
        .find(|r| r.message_id.starts_with("loopi_"))
        .unwrap();
    assert_eq!(
        (row.source.as_str(), row.status),
        ("loop", InputStatus::Promoted)
    );

    assert_eq!(
        h.runtime.sweep_loops(tick - 1).await.unwrap(),
        0,
        "not due yet"
    );
    h.runtime.sweep_loops(tick).await.unwrap();
    let second = wait_for(&h, &id, &created.loop_id, |l| l.history.len() == 2).await;
    assert_eq!(second.iterations, 2);
    assert_eq!(second.next_due_ms, Some(tick + 300_000));
    assert_eq!(h.models.requests("test/main").len(), 2);
}

#[tokio::test]
async fn a_short_interval_is_refused() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let error = h
        .runtime
        .create_loop(&id, every(10, "ping"))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::Invalid(m) if m == "Loop interval must be at least 60s"),
        "{error}"
    );
    assert!(h.state(&id).await.loops.is_empty());
}

#[tokio::test]
async fn a_tick_during_a_running_iteration_is_skipped_or_coalesced() {
    for overlap in [Overlap::Skip, Overlap::Coalesce] {
        let h = Harness::new(Setup {
            scripts: vec![(
                "test/main",
                vec![
                    tools(&[("c1", "write", "{}")]),
                    text("slow done"),
                    text("caught up"),
                ],
            )],
            ..Setup::default()
        });
        h.tools.set("write", Behavior::Gated);
        let id = h.session().await;
        let spec = LoopSpec {
            overlap,
            ..every(60, "poll")
        };
        let created = h.runtime.create_loop(&id, spec).await.unwrap();
        h.tools.started.notified().await;
        let tick = created.created_ms + 60_000;
        h.runtime.sweep_loops(tick).await.unwrap();
        let during = loop_state(&h, &id, &created.loop_id).await;
        assert_eq!(during.skipped, 1, "{overlap:?}");
        assert_eq!(during.history[0].outcome, Outcome::SkippedOverlap);
        assert_eq!(during.next_due_ms, Some(tick + 60_000));
        h.tools.release.notify_one();
        let after = wait_for(&h, &id, &created.loop_id, |l| {
            l.inflight.is_none() && l.iterations >= 1
        })
        .await;
        match overlap {
            // Skipped: the next iteration waits for the next tick.
            Overlap::Skip => {
                assert_eq!(after.iterations, 1);
                assert_eq!(after.next_due_ms, Some(tick + 60_000));
            }
            // Coalesced: one iteration runs as soon as the slow one ended.
            Overlap::Coalesce => {
                let caught_up = wait_for(&h, &id, &created.loop_id, |l| {
                    l.iterations == 2 && l.inflight.is_none()
                })
                .await;
                assert_eq!(
                    caught_up.history.back().unwrap().outcome,
                    Outcome::Completed
                );
            }
        }
    }
}

#[tokio::test]
async fn max_iterations_stop_the_loop() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("1"), text("2")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let spec = LoopSpec {
        max_iterations: Some(2),
        ..every(60, "count")
    };
    let created = h.runtime.create_loop(&id, spec).await.unwrap();
    wait_for(&h, &id, &created.loop_id, |l| {
        l.iterations == 1 && l.inflight.is_none()
    })
    .await;
    h.runtime
        .sweep_loops(created.created_ms + 60_000)
        .await
        .unwrap();
    let ended = wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "max_iterations");
    assert_eq!(ended.iterations, 2);
    assert_eq!(
        h.runtime
            .sweep_loops(created.created_ms + 600_000)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn until_stops_the_loop_once_the_check_succeeds() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("not yet")])],
        ..Setup::default()
    });
    *h.tools.check_exit.lock().unwrap() = Some(1);
    let id = h.session().await;
    let spec = LoopSpec {
        until: Some("curl -sf https://staging/health".into()),
        ..every(60, "deploy")
    };
    let created = h.runtime.create_loop(&id, spec).await.unwrap();
    wait_for(&h, &id, &created.loop_id, |l| {
        l.iterations == 1 && l.inflight.is_none()
    })
    .await;

    *h.tools.check_exit.lock().unwrap() = Some(0);
    h.runtime
        .sweep_loops(created.created_ms + 60_000)
        .await
        .unwrap();
    let ended = loop_state(&h, &id, &created.loop_id).await;
    assert_eq!(ended.status, "until_met");
    assert!(ended.stop_reason.unwrap().contains("exited 0"));
    assert_eq!(
        ended.iterations, 1,
        "no iteration after the check succeeded"
    );
    assert_eq!(h.tools.checks.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_spent_budget_stops_the_loop_between_iterations() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("1")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let spec = LoopSpec {
        budget: Some(Budget {
            max_turns: Some(1),
            ..Budget::default()
        }),
        ..every(60, "spend")
    };
    let created = h.runtime.create_loop(&id, spec).await.unwrap();
    let ended = wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "budget_exceeded");
    assert_eq!(ended.stop_reason.as_deref(), Some("max_turns 1/1"));
}

#[tokio::test]
async fn failing_iterations_stop_the_loop_after_three() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                error(cyber_llm::ErrorKind::InvalidRequest, "bad 1"),
                error(cyber_llm::ErrorKind::InvalidRequest, "bad 2"),
                error(cyber_llm::ErrorKind::InvalidRequest, "bad 3"),
            ],
        )],
        ..Setup::default()
    });
    let id = h.session().await;
    let created = h
        .runtime
        .create_loop(&id, every(60, "flaky"))
        .await
        .unwrap();
    for n in 1..=3_i64 {
        wait_for(&h, &id, &created.loop_id, |l| l.history.len() as i64 == n).await;
        if n < 3 {
            h.runtime
                .sweep_loops(created.created_ms + 60_000 * n)
                .await
                .unwrap();
        }
    }
    let ended = wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "failed");
    assert!(ended.history.iter().all(|r| r.outcome == Outcome::Failed));
}

#[tokio::test]
async fn stop_ends_one_or_every_loop() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("a"), text("b"), text("c")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let a = h.runtime.create_loop(&id, every(60, "a")).await.unwrap();
    let b = h.runtime.create_loop(&id, every(60, "b")).await.unwrap();
    let c = h.runtime.create_loop(&id, every(60, "c")).await.unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.runtime.stop_loops(&id, Some(&a.loop_id)).await.unwrap(),
        vec![a.loop_id.clone()]
    );
    assert!(matches!(
        h.runtime.stop_loops(&id, Some(&a.loop_id)).await,
        Err(RuntimeError::LoopNotFound(_))
    ));
    let mut rest = h.runtime.stop_loops(&id, None).await.unwrap();
    rest.sort();
    let mut expected = vec![b.loop_id.clone(), c.loop_id.clone()];
    expected.sort();
    assert_eq!(rest, expected);
    let listed = h.runtime.loops(&id).await.unwrap();
    assert!(listed.iter().all(|l| l.status == "stopped"));
}

#[tokio::test]
async fn missed_ticks_follow_the_policy_after_a_restart() {
    for missed in [Missed::Skip, Missed::RunOnce] {
        let h = Harness::new(Setup {
            scripts: vec![("test/main", vec![text("first"), text("catch-up")])],
            ..Setup::default()
        });
        let id = h.session().await;
        let spec = LoopSpec {
            missed,
            ..every(3600, "hourly")
        };
        let created = h.runtime.create_loop(&id, spec).await.unwrap();
        wait_for(&h, &id, &created.loop_id, |l| {
            l.iterations == 1 && l.inflight.is_none()
        })
        .await;

        // The server was down for five hours.
        let now = created.created_ms + 5 * 3_600_000 + 1_000;
        let restarted = h.restart();
        restarted.recover_loops_at(now).await.unwrap();
        let state = restarted.state(&id).await.unwrap().loops[&created.loop_id].clone();
        match missed {
            Missed::Skip => {
                assert_eq!(
                    state.history.back().unwrap().outcome,
                    Outcome::SkippedMissed
                );
                assert_eq!(state.next_due_ms, Some(created.created_ms + 6 * 3_600_000));
                assert_eq!(restarted.sweep_loops(now).await.unwrap(), 0, "no catch-up");
            }
            Missed::RunOnce => {
                assert_eq!(restarted.sweep_loops(now).await.unwrap(), 1, "one catch-up");
                tokio::time::timeout(Duration::from_secs(30), restarted.wait_idle(&id))
                    .await
                    .unwrap();
                let after = restarted.state(&id).await.unwrap().loops[&created.loop_id].clone();
                assert_eq!(after.iterations, 2);
                assert_eq!(after.next_due_ms, Some(created.created_ms + 6 * 3_600_000));
            }
        }
    }
}

#[tokio::test]
async fn an_iteration_cut_off_by_a_restart_ends_interrupted() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "write", "{}")]), text("never")],
        )],
        ..Setup::default()
    });
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    let created = h.runtime.create_loop(&id, every(60, "work")).await.unwrap();
    h.tools.started.notified().await;

    let restarted = h.restart();
    let now = created.created_ms + 10_000;
    restarted.recover_loops_at(now).await.unwrap();
    let state = restarted.state(&id).await.unwrap().loops[&created.loop_id].clone();
    assert!(state.inflight.is_none());
    assert_eq!(state.history[0].outcome, Outcome::Interrupted);
    assert_eq!(state.next_due_ms, Some(created.created_ms + 60_000));
    h.tools.release.notify_one();
}

// ---- In-session cron tasks, --until-goal and notifications (`implement-m2-4-schedules`).

use cyber_core::loops::Notify;
use cyber_server::runtime::loops::{CronRequest, LoopKind};

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339()
}

fn at(prompt: &str, at_ms: i64) -> CronRequest {
    CronRequest {
        at: Some(rfc3339(at_ms)),
        prompt: prompt.into(),
        ..CronRequest::default()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
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

#[tokio::test]
async fn a_one_shot_reminder_runs_at_its_time_then_completes() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("CI was green overnight")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let when = now_ms() + 3_600_000;
    let task = h
        .runtime
        .create_cron(&id, at("summarize overnight CI", when))
        .await
        .unwrap();
    assert!(task.loop_id.starts_with("cron_"), "{}", task.loop_id);
    assert_eq!((task.kind, task.at_ms), (LoopKind::Cron, Some(when)));
    let due = task.next_due_ms.unwrap();
    assert_eq!(
        h.runtime.sweep_loops(due - 1).await.unwrap(),
        0,
        "not due yet"
    );
    assert_eq!(h.runtime.sweep_loops(due).await.unwrap(), 1);
    let ended = wait_for(&h, &id, &task.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "completed");
    assert_eq!(ended.iterations, 1);
    let state = h.state(&id).await;
    let row = state
        .inbox
        .iter()
        .find(|r| r.message_id.starts_with("loopi_cron_"))
        .unwrap();
    assert_eq!(row.source, "loop");
    assert!(
        transcript(&h.models.requests("test/main")[0]).contains("summarize overnight CI"),
        "the prompt was admitted"
    );
    // An iteration that reported nothing quiet notifies under the default `on_change`.
    let sent = notices(&h, &id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("Scheduled task"), "{sent:?}");
}

#[tokio::test]
async fn a_recurring_task_is_due_at_its_next_local_occurrence() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let before = now_ms();
    let task = h
        .runtime
        .create_cron(
            &id,
            CronRequest {
                cron: Some("0 9 * * 1-5".into()),
                prompt: "standup notes".into(),
                ..CronRequest::default()
            },
        )
        .await
        .unwrap();
    let expected = cyber_core::cron::Cron::parse("0 9 * * 1-5")
        .unwrap()
        .next_after(
            chrono::DateTime::from_timestamp_millis(before).unwrap(),
            &chrono::Local,
        )
        .unwrap()
        .timestamp_millis();
    assert_eq!(task.next_due_ms, Some(expected));
    assert_eq!(task.cron.as_deref(), Some("0 9 * * 1-5"));
    let listed = h.runtime.cron_tasks(&id).await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn invalid_schedules_are_rejected_with_the_reason() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let bad = |request: CronRequest| {
        let runtime = h.runtime.clone();
        let id = id.clone();
        async move {
            runtime
                .create_cron(&id, request)
                .await
                .unwrap_err()
                .to_string()
        }
    };
    let error = bad(CronRequest {
        cron: Some("61 * * * *".into()),
        prompt: "x".into(),
        ..CronRequest::default()
    })
    .await;
    assert!(
        error.contains("Invalid cron expression \"61 * * * *\""),
        "{error}"
    );
    let error = bad(at("x", now_ms() - 60_000)).await;
    assert!(error.contains("at must be in the future"), "{error}");
    let error = bad(CronRequest {
        cron: Some("* * * * *".into()),
        at: Some(rfc3339(now_ms() + 60_000)),
        prompt: "x".into(),
        ..CronRequest::default()
    })
    .await;
    assert!(error.contains("exactly one of cron"), "{error}");
    assert!(h.state(&id).await.loops.is_empty());
}

#[tokio::test]
async fn loops_and_scheduled_tasks_share_the_per_session_cap() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let later = now_ms() + 86_400_000;
    for n in 0..50 {
        h.runtime
            .create_cron(&id, at(&format!("task {n}"), later))
            .await
            .unwrap();
    }
    let error = h
        .runtime
        .create_cron(&id, at("one more", later))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "LoopLimitError: Scheduled task limit reached (50)"
    );
    let error = h
        .runtime
        .create_loop(&id, every(60, "and a loop"))
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::LoopLimit(50)), "{error}");
}

#[tokio::test]
async fn cron_delete_stops_only_scheduled_tasks() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("checked")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let user_loop = h
        .runtime
        .create_loop(&id, every(300, "deploy"))
        .await
        .unwrap();
    h.settle(&id).await;
    let error = h
        .runtime
        .delete_cron(&id, &user_loop.loop_id)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Unknown scheduled task"),
        "{error}"
    );
    assert!(loop_state(&h, &id, &user_loop.loop_id).await.active());
    let task = h
        .runtime
        .create_cron(&id, at("later", now_ms() + 3_600_000))
        .await
        .unwrap();
    h.runtime.delete_cron(&id, &task.loop_id).await.unwrap();
    assert_eq!(loop_state(&h, &id, &task.loop_id).await.status, "stopped");
    assert!(h.runtime.cron_tasks(&id).await.unwrap().is_empty());
}

#[tokio::test]
async fn scheduled_tasks_belong_to_root_sessions() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    assert!(h.runtime.cron_tools_offered(&root));
    assert!(!h.runtime.cron_tools_offered(&child));
    let error = h
        .runtime
        .create_cron(&child, at("x", now_ms() + 3_600_000))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("root Sessions"), "{error}");
}

#[tokio::test]
async fn missed_scheduled_tasks_follow_the_policy_after_a_restart() {
    for missed in [Missed::Skip, Missed::RunOnce] {
        let h = Harness::new(Setup {
            scripts: vec![("test/main", vec![text("caught up")])],
            ..Setup::default()
        });
        let id = h.session().await;
        let when = now_ms() + 3_600_000;
        let once = h
            .runtime
            .create_cron(
                &id,
                CronRequest {
                    missed,
                    ..at("reminder", when)
                },
            )
            .await
            .unwrap();
        let daily = h
            .runtime
            .create_cron(
                &id,
                CronRequest {
                    cron: Some("0 2 * * *".into()),
                    prompt: "nightly".into(),
                    missed: Missed::Skip,
                    ..CronRequest::default()
                },
            )
            .await
            .unwrap();
        // The server was down past both due times.
        let now = once.next_due_ms.unwrap().max(daily.next_due_ms.unwrap()) + 60_000;
        let restarted = h.restart();
        restarted.recover_loops_at(now).await.unwrap();
        let state = restarted.state(&id).await.unwrap();
        let recurring = &state.loops[&daily.loop_id];
        assert_eq!(
            recurring.history.back().unwrap().outcome,
            Outcome::SkippedMissed
        );
        assert!(recurring.next_due_ms.unwrap() > now);
        let reminder = &state.loops[&once.loop_id];
        match missed {
            Missed::Skip => {
                assert_eq!(reminder.status, "expired");
                assert_eq!(restarted.sweep_loops(now).await.unwrap(), 0);
            }
            Missed::RunOnce => {
                assert!(reminder.active());
                assert_eq!(restarted.sweep_loops(now).await.unwrap(), 1);
                tokio::time::timeout(Duration::from_secs(30), restarted.wait_idle(&id))
                    .await
                    .unwrap();
                let after = restarted.state(&id).await.unwrap().loops[&once.loop_id].clone();
                assert_eq!((after.status.as_str(), after.iterations), ("completed", 1));
            }
        }
    }
}

#[tokio::test]
async fn until_goal_stops_the_loop_once_the_goal_ends() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![
                    tools(&[("c1", "write", "{}")]),
                    text("goal work"),
                    text("loop work"),
                ],
            ),
            (
                "test/evaluator",
                vec![text("{\"verdict\": \"not_met\", \"reason\": \"x\"}")],
            ),
        ],
        roles: vec![("evaluator", "test/evaluator")],
        ..Setup::default()
    });
    h.tools.set("write", Behavior::Gated);
    let id = h.session().await;
    let no_goal = LoopSpec {
        until_goal: true,
        ..every(60, "watch")
    };
    let error = h
        .runtime
        .create_loop(&id, no_goal.clone())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("--until-goal needs a Goal"),
        "{error}"
    );

    let goal = h
        .runtime
        .set_goal(
            &id,
            cyber_server::runtime::goals::GoalRequest {
                condition: "the build is green".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.tools.started.notified().await;
    let created = h.runtime.create_loop(&id, no_goal).await.unwrap();
    assert_eq!(
        created.stop.until_goal.as_deref(),
        Some(goal.goal_id.as_str())
    );
    h.runtime.clear_goals(&id, None).await.unwrap();
    h.tools.release.notify_one();
    let ended = wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "goal_ended");
    assert_eq!(
        ended.stop_reason.as_deref(),
        Some(format!("goal {} cleared", goal.goal_id).as_str())
    );
}

#[tokio::test]
async fn notify_never_keeps_iterations_and_stops_quiet() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("1")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let spec = LoopSpec {
        notify: Notify::Never,
        budget: Some(Budget {
            max_turns: Some(1),
            ..Budget::default()
        }),
        ..every(60, "quiet")
    };
    let created = h.runtime.create_loop(&id, spec).await.unwrap();
    let ended = wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    assert_eq!(ended.status, "budget_exceeded");
    assert!(notices(&h, &id).is_empty());
}

#[tokio::test]
async fn a_budget_stop_notifies_once() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("1")])],
        ..Setup::default()
    });
    let id = h.session().await;
    let spec = LoopSpec {
        budget: Some(Budget {
            max_turns: Some(1),
            ..Budget::default()
        }),
        ..every(60, "spend")
    };
    let created = h.runtime.create_loop(&id, spec).await.unwrap();
    wait_for(&h, &id, &created.loop_id, |l| !l.active()).await;
    let sent = notices(&h, &id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].contains("stopped (budget_exceeded): max_turns 1/1"),
        "{sent:?}"
    );
}
