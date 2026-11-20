//! Session loops on the production App (`loops-scheduling`): `/loop` over HTTP, the real
//! `schedule_wakeup` tool and the scripted OpenAI-compatible provider.

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{ModeApp, Setup};

async fn session(app: &ModeApp) -> String {
    let (status, body) = app.create("default").await;
    assert_eq!(status, 201, "{body}");
    body["data"]["id"].as_str().unwrap().to_string()
}

async fn create(app: &ModeApp, id: &str, command: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/loops"),
            Some(json!({ "command": command })),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    body["data"].clone()
}

/// The loop once `done` holds for it.
async fn wait_loop(app: &ModeApp, id: &str, loop_id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (_, body) = app
            .send("GET", &format!("/sessions/{id}/loops"), None)
            .await;
        let found = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["loop_id"] == loop_id)
            .cloned()
            .unwrap();
        if done(&found) {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "loop never settled: {found}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The tool names a model request offered.
fn offered(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_self_paced_loop_waits_the_clamped_wakeup_and_records_noop() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    let created = create(
        &app,
        &id,
        r#"TOOL<<schedule_wakeup>>{"delay_seconds": 10, "reason": "CI takes a while", "noop": true}"#,
    )
    .await;
    assert_eq!(created["kind"], "self_paced");
    let loop_id = created["loop_id"].as_str().unwrap();

    let after = wait_loop(&app, &id, loop_id, |l| {
        l["history"].as_array().is_some_and(|h| h.len() == 1)
    })
    .await;
    assert_eq!(after["status"], "active");
    let record = &after["history"][0];
    assert_eq!(
        (record["outcome"].as_str(), record["noop"].as_bool()),
        (Some("completed"), Some(true))
    );
    assert_eq!(record["reason"], "CI takes a while");
    // 10 s is clamped to the 60 s minimum, counted from the iteration's end.
    let next = after["next_due_ms"].as_i64().unwrap();
    let ended = record["ended_ms"].as_i64().unwrap();
    assert!((59_000..=61_000).contains(&(next - ended)), "{after}");

    // The iteration's request offered the tool (the title request offers none).
    let requests = app.model.requests();
    let iteration = requests.iter().find(|r| !offered(r).is_empty()).unwrap();
    assert!(offered(iteration).contains(&"schedule_wakeup".to_string()));
    let wakeup_events: Vec<_> = app
        .events(&id)
        .into_iter()
        .filter(|e| e.kind == "loop.wakeup.1")
        .collect();
    assert_eq!(wakeup_events[0].data["delay_seconds"], 60);
}

#[tokio::test]
async fn a_self_paced_iteration_without_a_wakeup_completes_the_loop() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    let created = create(&app, &id, "summarize the build").await;
    let loop_id = created["loop_id"].as_str().unwrap();
    let ended = wait_loop(&app, &id, loop_id, |l| l["status"] != "active").await;
    assert_eq!(ended["status"], "completed");
    assert_eq!(ended["iterations"], 1);
}

#[tokio::test]
async fn schedule_wakeup_is_offered_only_inside_a_loop_iteration() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    app.run(&id, "hello").await;
    let requests = app.model.requests();
    let turn = requests.iter().find(|r| !offered(r).is_empty()).unwrap();
    assert!(offered(turn).contains(&"read".to_string()));
    assert!(!offered(turn).contains(&"schedule_wakeup".to_string()));

    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/loops"),
            Some(json!({ "command": "10s ping" })),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["message"], "Loop interval must be at least 60s");
}

#[tokio::test]
async fn interval_loops_are_listed_and_stopped_over_http() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    let created = create(&app, &id, "5m --max 3 check the deploy").await;
    let loop_id = created["loop_id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            created["kind"].as_str(),
            created["interval_seconds"].as_u64()
        ),
        (Some("interval"), Some(300))
    );
    wait_loop(&app, &id, &loop_id, |l| {
        l["iterations"] == 1 && l["inflight"].is_null()
    })
    .await;

    let (status, body) = app
        .send("DELETE", &format!("/sessions/{id}/loops/{loop_id}"), None)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"], json!([loop_id]));
    let (status, body) = app
        .send("DELETE", &format!("/sessions/{id}/loops/{loop_id}"), None)
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("LoopNotFoundError"))
    );
    let stopped = wait_loop(&app, &id, &loop_id, |l| l["status"] != "active").await;
    assert_eq!(stopped["status"], "stopped");
}

/// Regression: the App's host forwards the loop methods, so `--until` runs a real
/// sandboxed check; a check that exits 0 stops the loop before any iteration.
#[tokio::test]
async fn until_runs_a_real_check_through_the_app_host() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    let created = create(&app, &id, r#"5m --until "test -d .git" deploy check"#).await;
    let loop_id = created["loop_id"].as_str().unwrap();
    let ended = wait_loop(&app, &id, loop_id, |l| l["status"] != "active").await;
    assert_eq!(ended["status"], "until_met", "{ended}");
    assert_eq!(ended["iterations"], 0);
}

/// `loops-scheduling` → In-session cron tools: the real tools, offered to a root
/// Session; `cron_delete` cannot reach a `/loop` the user started.
#[tokio::test]
async fn the_model_manages_scheduled_tasks_with_the_cron_tools() {
    let app = ModeApp::start(Setup::default()).await;
    let id = session(&app).await;
    let at = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    app.run(
        &id,
        &format!(r#"TOOL<<cron_create>>{{"at": "{at}", "prompt": "summarize overnight CI"}}"#),
    )
    .await;
    let outputs = app.outputs(&id).await;
    let created: Value = serde_json::from_str(&outputs[0]).unwrap_or(Value::Null);
    let task_id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{outputs:?}"))
        .to_owned();
    assert!(task_id.starts_with("cron_"), "{created}");
    assert!(created["next_run"].is_string(), "{created}");
    let requests = app.model.requests();
    let turn = requests.iter().find(|r| !offered(r).is_empty()).unwrap();
    for tool in ["cron_create", "cron_list", "cron_delete"] {
        assert!(offered(turn).contains(&tool.to_string()), "{tool}");
    }

    let (_, listed) = app
        .send("GET", &format!("/sessions/{id}/loops"), None)
        .await;
    let task = &listed["data"][0];
    assert_eq!(
        (task["kind"].as_str(), task["loop_id"].as_str()),
        (Some("cron"), Some(task_id.as_str()))
    );
    assert!(task["at_ms"].is_i64(), "{task}");

    // A `/loop` of the user (here one whose check already passed) is not a task.
    let user_loop = create(&app, &id, r#"1h --until "test -d .git" check the deploy"#).await;
    let loop_id = user_loop["loop_id"].as_str().unwrap().to_owned();
    app.run(&id, &format!(r#"TOOL<<cron_delete>>{{"id": "{loop_id}"}}"#))
        .await;
    assert_output(&app, &id, &format!("Unknown scheduled task {loop_id}")).await;
    app.run(&id, "TOOL<<cron_list>>{}").await;
    assert_output(&app, &id, "\"tasks\":[{\"id\":\"cron_").await;
    app.run(&id, &format!(r#"TOOL<<cron_delete>>{{"id": "{task_id}"}}"#))
        .await;
    let ended = wait_loop(&app, &id, &task_id, |l| l["status"] != "active").await;
    assert_eq!(ended["status"], "stopped");
}

/// `loops-scheduling` → Loop events and hooks: `LoopIteration` receives the loop and the
/// iteration on stdin after `loop.iteration.ended.1` commits.
#[tokio::test]
async fn loop_iteration_hook_receives_the_iteration() {
    let app = ModeApp::start(Setup {
        config: json!({"hooks": {"LoopIteration": [{"hooks": [
            {"type": "command", "command": "cat > iteration.json"}]}]}}),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    let created = create(&app, &id, "5m --max 1 check the deploy").await;
    let loop_id = created["loop_id"].as_str().unwrap();
    wait_loop(&app, &id, loop_id, |l| l["status"] != "active").await;
    let path = app.repo.join("iteration.json");
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
    assert_eq!(
        written["hook_event_name"]
            .as_str()
            .or(written["event"].as_str()),
        Some("LoopIteration"),
        "{written}"
    );
    assert_eq!(written["loop"]["loop_id"], loop_id, "{written}");
    assert_eq!(written["iteration"]["n"], 1);
    assert_eq!(written["iteration"]["outcome"], "completed");
    assert_eq!(written["iteration"]["noop"], false);
    let kinds: Vec<String> = app.events(&id).into_iter().map(|e| e.kind).collect();
    assert!(
        kinds.iter().any(|k| k == "loop.iteration.ended.1"),
        "{kinds:?}"
    );
}

/// Some settled call of the Session returned output containing `needle`.
async fn assert_output(app: &ModeApp, id: &str, needle: &str) {
    let outputs = app.outputs(id).await;
    assert!(
        outputs.iter().any(|o| o.contains(needle)),
        "{needle}: {outputs:?}"
    );
}
