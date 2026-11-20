//! Persistent schedules on the production App (`loops-scheduling` → Persistent local
//! schedules, Unattended Mode for schedules, Completion notifications, Loop events and
//! hooks): the `/schedules` routes, the real tool host's gate and denials, and the real
//! `schedule_wakeup` tool, with the scripted OpenAI-compatible provider.

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{ModeApp, Setup};

async fn add(app: &ModeApp, body: Value) -> Value {
    let (status, created) = app.send("POST", "/schedules", Some(body)).await;
    assert_eq!(status, 201, "{created}");
    created["data"].clone()
}

/// Start a run now and return the schedule once that run ended.
async fn run_now(app: &ModeApp, id: &str) -> Value {
    let (status, body) = app
        .send("POST", &format!("/schedules/{id}/run"), None)
        .await;
    assert_eq!(status, 200, "{body}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (_, shown) = app.send("GET", &format!("/schedules/{id}"), None).await;
        let run = &shown["data"]["recent"][0];
        if run["outcome"].is_string() && run["outcome"] != "running" {
            return shown["data"].clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "{shown}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `loops-scheduling` → Unattended Mode for schedules: under `dont-ask` an ask is denied
/// with the schedule's message and the run continues to its end.
#[tokio::test]
async fn an_ask_in_a_scheduled_run_is_denied_with_feedback() {
    let app = ModeApp::start(Setup::default()).await;
    let schedule = add(
        &app,
        json!({"cron": "0 2 * * *", "name": "nightly",
            "prompt": "RUN<<curl -sf https://staging.example/health>>"}),
    )
    .await;
    assert_eq!(schedule["mode"], "dont-ask");
    let id = schedule["id"].as_str().unwrap();
    let shown = run_now(&app, id).await;
    let run = &shown["recent"][0];
    assert_eq!(run["outcome"], "completed", "{shown}");
    let session = run["session_id"].as_str().unwrap();
    let outputs = app.outputs(session).await;
    assert!(
        outputs
            .iter()
            .any(|o| o.contains("Denied: no approver in unattended schedule")),
        "{outputs:?}"
    );
}

/// `loops-scheduling` → Completion notifications, Loop events and hooks: a run that calls
/// `schedule_wakeup` with `noop: true` is quiet under `on_change`, and `ScheduleRun` gets
/// the schedule and the run on stdin.
#[tokio::test]
async fn a_quiet_run_records_noop_and_fires_the_schedule_run_hook() {
    let app = ModeApp::start(Setup {
        config: json!({"hooks": {"ScheduleRun": [{"hooks": [
            {"type": "command", "command": "cat > run.json"}]}]}}),
        ..Setup::default()
    })
    .await;
    let schedule = add(
        &app,
        json!({"cron": "0 2 * * *", "name": "quiet",
            "prompt": r#"TOOL<<schedule_wakeup>>{"delay_seconds": 60, "reason": "nothing new", "noop": true}"#}),
    )
    .await;
    let id = schedule["id"].as_str().unwrap();
    let shown = run_now(&app, id).await;
    let run = &shown["recent"][0];
    assert_eq!(
        (run["outcome"].as_str(), run["noop"].as_bool()),
        (Some("completed"), Some(true)),
        "{shown}"
    );
    assert_eq!(run["reason"], "nothing new");
    let session = run["session_id"].as_str().unwrap();
    let notified = app
        .events(session)
        .into_iter()
        .filter(|e| e.kind == "session.notification.sent.1")
        .count();
    assert_eq!(notified, 0, "on_change stays quiet for a noop run");

    let path = app.repo.join("run.json");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let written = loop {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            break value;
        }
        assert!(tokio::time::Instant::now() < deadline, "the hook never ran");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(written["schedule"]["id"], id, "{written}");
    assert_eq!(written["run"]["noop"], true);
    assert_eq!(written["run"]["outcome"], "completed");
}

#[tokio::test]
async fn schedules_are_managed_over_http() {
    let app = ModeApp::start(Setup::default()).await;
    let (status, body) = app
        .send(
            "POST",
            "/schedules",
            Some(json!({"cron": "61 * * * *", "prompt": "x"})),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (400, Some("InvalidRequestError")),
        "{body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Invalid cron expression")
    );
    let (status, body) = app
        .send(
            "POST",
            "/schedules",
            Some(json!({"cron": "0 2 * * *", "prompt": "x", "mode": "bypass"})),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("loops.allow_bypass"),
        "{body}"
    );

    let schedule = add(&app, json!({"cron": "0 2 * * *", "prompt": "review"})).await;
    let id = schedule["id"].as_str().unwrap();
    assert_eq!(schedule["directory"], app.repo.display().to_string());
    assert!(schedule["next_due_ms"].is_i64());
    let (_, listed) = app.send("GET", "/schedules", None).await;
    assert_eq!(listed["data"][0]["id"], id);
    let (_, paused) = app
        .send("POST", &format!("/schedules/{id}/pause"), None)
        .await;
    assert_eq!(paused["data"]["status"], "paused");
    assert!(paused["data"]["next_due_ms"].is_null());
    let (_, resumed) = app
        .send("POST", &format!("/schedules/{id}/resume"), None)
        .await;
    assert_eq!(resumed["data"]["status"], "active");
    let (status, _) = app.send("DELETE", &format!("/schedules/{id}"), None).await;
    assert_eq!(status, 200);
    let (status, body) = app.send("GET", &format!("/schedules/{id}"), None).await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("ScheduleNotFoundError"))
    );
}

/// A user config that allows `bypass` admits a `bypass` schedule only through the user
/// layer: the same key in the trusted checkout's project config does not.
#[tokio::test]
async fn only_user_config_allows_bypass_schedules() {
    let app = ModeApp::start(Setup::default()).await;
    std::fs::write(
        app.repo.join("cyber.jsonc"),
        r#"{"loops": {"allow_bypass": true}}"#,
    )
    .unwrap();
    let (status, body) = app
        .send(
            "POST",
            "/schedules",
            Some(json!({"cron": "0 2 * * *", "prompt": "x", "mode": "bypass"})),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("loops.allow_bypass"),
        "{body}"
    );
}
