//! `loops-scheduling` (persistent local schedules), end to end with a scripted model.
//! Time is passed to the sweep explicitly; `run_schedule_now` starts a run at once.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_core::budget::Budget;
use cyber_core::loops::{Missed, Notify};
use cyber_server::runtime::RuntimeError;
use cyber_server::runtime::schedules::{
    DesktopNotifier, RunRecord, Schedule, SchedulePolicy, ScheduleRequest, ScheduleStatus,
    jitter_ms, next_run,
};
use support::*;

/// Desktop notifications, as `title: body`.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl DesktopNotifier for Recorder {
    fn notify(&self, title: &str, body: &str) {
        self.0.lock().unwrap().push(format!("{title}: {body}"));
    }
}

impl Recorder {
    fn sent(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn harness(replies: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> (Harness, Arc<Recorder>) {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", replies)],
        ..Setup::default()
    });
    let recorder = Arc::new(Recorder::default());
    h.runtime.set_desktop_notifier(recorder.clone());
    (h, recorder)
}

fn request(h: &Harness, cron: &str, prompt: &str) -> ScheduleRequest {
    ScheduleRequest {
        cron: cron.into(),
        prompt: prompt.into(),
        name: Some("nightly".into()),
        directory: Some(h.repo.display().to_string()),
        model: Some("test/main".into()),
        ..ScheduleRequest::default()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The schedule once its newest run ended, after its Session settled.
async fn ended_run(h: &Harness, id: &str) -> (Schedule, RunRecord) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let shown = h.runtime.schedule(id, true).unwrap();
        if let Some(run) = shown.recent.first() {
            if let Some(session) = &run.session_id {
                h.settle(session).await;
            }
            if run.outcome != "running" {
                let run = run.clone();
                return (h.runtime.schedule(id, true).unwrap(), run);
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "{shown:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn notices(h: &Harness, session: &str) -> Vec<String> {
    h.store
        .read_events(session, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "session.notification.sent.1")
        .map(|e| format!("{}: {}", e.data["title"], e.data["body"]))
        .collect()
}

#[tokio::test]
async fn a_nightly_schedule_runs_in_a_new_unattended_session() {
    let (h, desktop) = harness(vec![text("3 commits reviewed, all fine")]);
    let before = now_ms();
    let schedule = h
        .runtime
        .create_schedule(request(&h, "0 2 * * *", "review last night's commits"))
        .await
        .unwrap();
    assert!(schedule.id.starts_with("cron_"), "{}", schedule.id);
    assert_eq!(
        (schedule.mode.as_str(), schedule.status),
        ("dont-ask", ScheduleStatus::Active)
    );
    let due = schedule.next_due_ms.unwrap();
    assert!(due > before && due <= before + 86_400_000, "{due}");
    assert_eq!(Some(due), next_run("0 2 * * *", 0, due - 60_000));

    assert_eq!(
        h.runtime.sweep_schedules(due - 1).await.unwrap(),
        0,
        "not due yet"
    );
    assert_eq!(h.runtime.sweep_schedules(due).await.unwrap(), 1);
    let (after, run) = ended_run(&h, &schedule.id).await;
    assert_eq!(run.outcome, "completed", "{run:?}");
    assert_eq!(after.runs, 1);
    assert!(after.next_due_ms.unwrap() > due, "the next night");
    assert!(after.inflight.is_none());

    let session = run.session_id.clone().unwrap();
    let state = h.state(&session).await;
    assert!(state.info.unattended, "no client attends a scheduled run");
    assert_eq!(state.info.mode, "dont-ask");
    assert!(
        state.info.title.starts_with("nightly · "),
        "{}",
        state.info.title
    );
    let row = &state.inbox[0];
    assert!(row.message_id.starts_with("schedi_"), "{}", row.message_id);
    assert_eq!(row.source, "loop");

    let kinds: Vec<String> = h
        .store
        .read_events(&schedule.id, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        ["schedule.created.1", "schedule.run.1", "schedule.run.1"]
    );
    // A run that reported no `noop` changed something: `on_change` notifies.
    assert_eq!(desktop.sent().len(), 1, "{:?}", desktop.sent());
    assert!(desktop.sent()[0].starts_with("Schedule nightly: run 1 completed"));
    assert_eq!(notices(&h, &session).len(), 1);
}

#[tokio::test]
async fn a_due_run_while_one_is_in_flight_is_skipped() {
    let (h, _) = harness(vec![tools(&[("c1", "write", "{}")]), text("done")]);
    h.tools.set("write", Behavior::Gated);
    let schedule = h
        .runtime
        .create_schedule(request(&h, "*/5 * * * *", "poll"))
        .await
        .unwrap();
    h.runtime.run_schedule_now(&schedule.id).await.unwrap();
    h.tools.started.notified().await;
    let busy = h.runtime.schedule(&schedule.id, false).unwrap();
    assert!(busy.inflight.is_some());
    let error = h.runtime.run_schedule_now(&schedule.id).await.unwrap_err();
    assert!(error.to_string().contains("run in flight"), "{error}");

    let due = busy.next_due_ms.unwrap();
    h.runtime.sweep_schedules(due).await.unwrap();
    let shown = h.runtime.schedule(&schedule.id, true).unwrap();
    assert_eq!(shown.recent[0].outcome, "skipped_overlap", "{shown:?}");
    assert!(shown.next_due_ms.unwrap() > due);
    h.tools.release.notify_one();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let shown = h.runtime.schedule(&schedule.id, true).unwrap();
        if shown.inflight.is_none() {
            assert!(
                shown.recent.iter().any(|r| r.outcome == "completed"),
                "{shown:?}"
            );
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn jitter_is_fixed_per_schedule_and_included_in_the_next_run() {
    let (h, _) = harness(Vec::new());
    h.tools.schedule_policy.lock().unwrap().jitter_seconds = 60;
    let schedule = h
        .runtime
        .create_schedule(request(&h, "* * * * *", "tick"))
        .await
        .unwrap();
    assert_eq!(schedule.jitter_ms, jitter_ms(&schedule.id, 60));
    let next = schedule.next_due_ms.unwrap();
    assert_eq!((next - schedule.jitter_ms).rem_euclid(60_000), 0);
    let listed = h.runtime.schedules().unwrap();
    assert_eq!(listed[0].next_due_ms, Some(next));
}

#[tokio::test]
async fn the_gate_refuses_untrusted_directories_and_unallowed_bypass() {
    let (h, desktop) = harness(Vec::new());
    let policy = |p: SchedulePolicy| *h.tools.schedule_policy.lock().unwrap() = p;
    policy(SchedulePolicy::default());
    let error = h
        .runtime
        .create_schedule(request(&h, "0 2 * * *", "x"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not trusted"), "{error}");

    policy(SchedulePolicy {
        trusted: true,
        ..SchedulePolicy::default()
    });
    let bypass = ScheduleRequest {
        mode: Some("bypass".into()),
        ..request(&h, "0 2 * * *", "x")
    };
    let error = h.runtime.create_schedule(bypass.clone()).await.unwrap_err();
    assert!(error.to_string().contains("loops.allow_bypass"), "{error}");
    let missing = ScheduleRequest {
        directory: Some(h.repo.join("gone").display().to_string()),
        ..request(&h, "0 2 * * *", "x")
    };
    let error = h.runtime.create_schedule(missing).await.unwrap_err();
    assert!(
        error.to_string().contains("not an existing directory"),
        "{error}"
    );
    assert!(h.runtime.schedules().unwrap().is_empty());

    policy(SchedulePolicy {
        trusted: true,
        allow_bypass: true,
        ..SchedulePolicy::default()
    });
    let allowed = h.runtime.create_schedule(bypass).await.unwrap();
    assert_eq!(allowed.mode, "bypass");

    // The checkout changed after approval: the next run is refused, notified, and no
    // Session runs the prompt.
    policy(SchedulePolicy::default());
    h.runtime.run_schedule_now(&allowed.id).await.unwrap();
    let shown = h.runtime.schedule(&allowed.id, true).unwrap();
    let run = &shown.recent[0];
    assert_eq!(run.outcome, "refused");
    assert!(
        run.reason.as_deref().unwrap().contains("is not trusted"),
        "{run:?}"
    );
    assert!(run.session_id.is_none());
    assert_eq!(
        shown.status,
        ScheduleStatus::Active,
        "a refusal does not pause it"
    );
    assert!(
        desktop.sent()[0].contains("refused"),
        "{:?}",
        desktop.sent()
    );
}

#[tokio::test]
async fn a_spent_budget_stops_the_schedule_and_notifies() {
    let (h, desktop) = harness(vec![text("1")]);
    let schedule = h
        .runtime
        .create_schedule(ScheduleRequest {
            budget: Some(Budget {
                max_turns: Some(1),
                ..Budget::default()
            }),
            notify: Notify::Never,
            ..request(&h, "0 2 * * *", "spend")
        })
        .await
        .unwrap();
    h.runtime.run_schedule_now(&schedule.id).await.unwrap();
    ended_run(&h, &schedule.id).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let stopped = loop {
        let shown = h.runtime.schedule(&schedule.id, false).unwrap();
        if shown.status != ScheduleStatus::Active {
            break shown;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(stopped.status, ScheduleStatus::BudgetExceeded);
    assert_eq!(stopped.reason.as_deref(), Some("max_turns 1/1"));
    assert_eq!(stopped.next_due_ms, None);
    assert!(
        desktop.sent().is_empty(),
        "notify never: {:?}",
        desktop.sent()
    );
    let error = h.runtime.resume_schedule(&schedule.id).unwrap_err();
    assert!(
        error.to_string().contains("remove it and add a new one"),
        "{error}"
    );
    assert_eq!(h.runtime.sweep_schedules(i64::MAX / 2).await.unwrap(), 0);
}

#[tokio::test]
async fn pause_resume_and_remove() {
    let (h, _) = harness(Vec::new());
    let schedule = h
        .runtime
        .create_schedule(request(&h, "0 2 * * *", "x"))
        .await
        .unwrap();
    let paused = h.runtime.pause_schedule(&schedule.id).unwrap();
    assert_eq!(
        (paused.status, paused.next_due_ms),
        (ScheduleStatus::Paused, None)
    );
    assert_eq!(h.runtime.sweep_schedules(i64::MAX / 2).await.unwrap(), 0);
    let resumed = h.runtime.resume_schedule(&schedule.id).unwrap();
    assert_eq!(resumed.status, ScheduleStatus::Active);
    assert!(resumed.next_due_ms.unwrap() > now_ms());
    h.runtime.remove_schedule(&schedule.id).unwrap();
    assert!(h.runtime.schedules().unwrap().is_empty());
    assert!(matches!(
        h.runtime.schedule(&schedule.id, false),
        Err(RuntimeError::ScheduleNotFound(_))
    ));
    assert!(matches!(
        h.runtime.pause_schedule(&schedule.id),
        Err(RuntimeError::ScheduleNotFound(_))
    ));
    let error = h
        .runtime
        .create_schedule(request(&h, "0 2 31 2 *", "x"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Invalid cron expression"),
        "{error}"
    );
}

#[tokio::test]
async fn a_run_resumes_its_own_session_in_the_schedules_mode() {
    let (h, _) = harness(vec![text("first"), text("again")]);
    let session = h.session().await;
    assert_eq!(h.state(&session).await.info.mode, "default");
    let schedule = h
        .runtime
        .create_schedule(ScheduleRequest {
            session_id: Some(session.clone()),
            ..request(&h, "0 2 * * *", "check the queue")
        })
        .await
        .unwrap();
    h.runtime.run_schedule_now(&schedule.id).await.unwrap();
    let (_, run) = ended_run(&h, &schedule.id).await;
    assert_eq!(run.session_id.as_deref(), Some(session.as_str()));
    assert_eq!(run.outcome, "completed");
    let state = h.state(&session).await;
    assert_eq!(state.info.mode, "dont-ask");
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(session),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let error = h
        .runtime
        .create_schedule(ScheduleRequest {
            session_id: Some(child),
            ..request(&h, "0 2 * * *", "x")
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not a root Session"), "{error}");
}

#[tokio::test]
async fn a_run_cut_off_by_a_restart_ends_interrupted() {
    let (h, _) = harness(vec![tools(&[("c1", "write", "{}")]), text("never")]);
    h.tools.set("write", Behavior::Gated);
    let schedule = h
        .runtime
        .create_schedule(request(&h, "0 2 * * *", "work"))
        .await
        .unwrap();
    h.runtime.run_schedule_now(&schedule.id).await.unwrap();
    h.tools.started.notified().await;

    let restarted = h.restart();
    restarted.recover_schedules_at(now_ms()).await.unwrap();
    let shown = restarted.schedule(&schedule.id, true).unwrap();
    assert!(shown.inflight.is_none(), "{shown:?}");
    assert_eq!(shown.recent[0].outcome, "interrupted");
    h.tools.release.notify_one();
}

#[tokio::test]
async fn missed_runs_follow_the_policy_after_a_restart() {
    for missed in [Missed::Skip, Missed::RunOnce] {
        let (h, _) = harness(vec![text("caught up")]);
        let schedule = h
            .runtime
            .create_schedule(ScheduleRequest {
                missed,
                ..request(&h, "0 2 * * *", "nightly")
            })
            .await
            .unwrap();
        // The service was down for two days.
        let now = schedule.next_due_ms.unwrap() + 2 * 86_400_000;
        let restarted = h.restart();
        restarted.recover_schedules_at(now).await.unwrap();
        let shown = restarted.schedule(&schedule.id, true).unwrap();
        match missed {
            Missed::Skip => {
                assert_eq!(shown.recent[0].outcome, "skipped_missed");
                assert!(shown.next_due_ms.unwrap() > now);
                assert_eq!(
                    restarted.sweep_schedules(now).await.unwrap(),
                    0,
                    "no catch-up"
                );
            }
            Missed::RunOnce => {
                assert!(shown.recent.is_empty());
                assert_eq!(
                    restarted.sweep_schedules(now).await.unwrap(),
                    1,
                    "one catch-up"
                );
                let started = restarted.schedule(&schedule.id, false).unwrap();
                assert_eq!(started.runs, 1);
                assert!(started.next_due_ms.unwrap() > now);
            }
        }
    }
}
