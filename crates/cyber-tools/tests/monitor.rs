//! `monitor` through real dispatch and a runtime-owned monitor Job (`background-tasks` →
//! Monitor tool, `builtin-tools` → monitor tool): filtering, the rate limit, `until`, the
//! final notice, queued `source: monitor` admissions and the source permissions.
mod support;
use std::time::{Duration, Instant};

use cyber_server::runtime::{Delivery, InboxRow, Job, JobStatus, ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

fn flow(config: Value, scripts: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> Flow {
    let flow = Flow::new(scripts, false);
    flow.f.set_config(config);
    flow
}

fn allowed() -> Value {
    json!({"permissions": {"bash": "allow", "external_directory": "allow"}})
}

/// Enough scripted answers for the Turns that queued monitor lines start.
fn answers(n: usize) -> Vec<Vec<cyber_llm::adapters::ScriptStep>> {
    (0..n).map(|_| text("noted")).collect()
}

async fn monitor(flow: &Flow, session: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation("default", "monitor", input);
    inv.session_id = session.into();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

async fn start(flow: &Flow, session: &str, input: Value) -> Value {
    let output = support::ok(monitor(flow, session, input).await);
    serde_json::from_str(&output).expect(&output)
}

async fn until(what: &str, seconds: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn terminal(flow: &Flow, id: &str) -> Job {
    let mut job = None;
    until("the monitor to settle", 30, || {
        let current = flow.runtime.job(id).unwrap();
        let settled = current.status != JobStatus::Running && current.notified;
        job = Some(current);
        settled
    })
    .await;
    job.unwrap()
}

/// The admitted rows from the monitor, in admission order.
async fn monitor_rows(flow: &Flow, session: &str) -> Vec<InboxRow> {
    let state = flow.runtime.state(session).await.unwrap();
    let mut rows: Vec<InboxRow> = state
        .inbox
        .iter()
        .filter(|row| row.source == "monitor")
        .cloned()
        .collect();
    rows.sort_by_key(|row| row.admitted_seq);
    rows
}

fn row_text(row: &InboxRow) -> String {
    serde_json::to_value(&row.parts).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn notice(flow: &Flow, session: &str, job: &str) -> Value {
    let state = flow.runtime.state(session).await.unwrap();
    let row = state
        .inbox
        .iter()
        .find(|row| row.message_id == format!("msg_handback_{job}"))
        .expect("final notice");
    let text = row_text(row);
    let body = text.strip_prefix("Monitor ended: ").expect(&text);
    serde_json::from_str(body).unwrap()
}

fn append(path: &std::path::Path, text: &str) {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_filters_and_rate_limits_to_30_per_minute() {
    let flow = flow(allowed(), answers(40));
    let session = flow.session("default").await;
    let command =
        r#"i=0; while [ $i -lt 100 ]; do echo "ERROR $i"; echo "INFO $i"; i=$((i+1)); done"#;
    let handle = start(
        &flow,
        &session,
        json!({"command": command, "filter": "ERROR|panic", "description": "errors"}),
    )
    .await;
    let id = handle["job_id"].as_str().unwrap();
    assert!(id.starts_with("job_"), "{handle}");
    let job = terminal(&flow, id).await;
    assert_eq!(job.kind, "monitor");
    assert_eq!(job.command.as_deref(), Some(command));
    assert_eq!((job.status, job.exit_code), (JobStatus::Completed, Some(0)));
    let rows = monitor_rows(&flow, &session).await;
    let texts: Vec<String> = rows.iter().map(row_text).collect();
    let expected: Vec<String> = (0..30)
        .map(|i| format!("Monitor {id}: ERROR {i}"))
        .chain([format!(
            "Monitor {id}: 70 lines suppressed (max_lines_per_minute is 30)"
        )])
        .collect();
    assert_eq!(texts, expected);
    assert!(rows.iter().all(|row| row.delivery == Delivery::Queue));
    // Every line is logged, matching or not, then the end.
    let log = std::fs::read_to_string(handle["output_path"].as_str().unwrap()).unwrap();
    assert!(log.contains("INFO 99\n"), "{log}");
    assert!(
        log.ends_with("[monitor ended: the command exited with code 0]\n"),
        "{log}"
    );
}

#[tokio::test]
async fn monitor_until_match_ends_with_final_notice() {
    let flow = flow(allowed(), answers(10));
    let session = flow.session("default").await;
    let path = flow.f.repo.join("deploy.log");
    append(&path, "old line before the monitor\n");
    let handle = start(
        &flow,
        &session,
        json!({"path": "deploy.log", "filter": "step", "until": "Deployment complete"}),
    )
    .await;
    let id = handle["job_id"].as_str().unwrap().to_owned();
    let mut stable = handle.clone();
    stable["job_id"] = json!("<job>");
    stable["output_path"] = json!("<data>/jobs/<job>.log");
    support::golden(&flow.f, "monitor", &stable.to_string());
    append(&path, "step 1 done\nnoise\n");
    until("the first step line", 10, || {
        std::fs::read_to_string(handle["output_path"].as_str().unwrap())
            .unwrap_or_default()
            .contains("noise")
    })
    .await;
    append(&path, "Deployment complete\nstep after the end\n");
    let job = terminal(&flow, &id).await;
    assert_eq!((job.status, job.exit_code), (JobStatus::Completed, Some(0)));
    let texts: Vec<String> = monitor_rows(&flow, &session)
        .await
        .iter()
        .map(row_text)
        .collect();
    // Lines from before the monitor and after `until` are not delivered.
    assert_eq!(texts, [format!("Monitor {id}: step 1 done")]);
    let notice = notice(&flow, &session, &id).await;
    assert_eq!(notice["job"]["kind"], "monitor");
    assert_eq!(notice["job"]["status"], "completed");
    let tail = notice["output_tail"].as_str().unwrap();
    assert!(tail.contains("Deployment complete"), "{tail}");
    assert!(tail.ends_with("[monitor ended: until matched]"), "{tail}");
    assert!(!tail.contains("step after the end"), "{tail}");
}

#[tokio::test]
async fn monitor_lines_are_queued_with_source_monitor() {
    let mut scripts = vec![
        call(
            "watch",
            "monitor",
            json!({"path": "app.log", "filter": "ERROR"}),
        ),
        text("watching"),
    ];
    scripts.extend(answers(5));
    let flow = flow(allowed(), scripts);
    let session = flow.session("default").await;
    flow.prompt(&session, "watch the log").await;
    flow.settle(&session).await;
    let handle: Value = serde_json::from_str(&flow.output(&session, "watch").await).unwrap();
    let id = handle["job_id"].as_str().unwrap().to_owned();
    assert_eq!(flow.runtime.job(&id).unwrap().status, JobStatus::Running);
    append(&flow.f.repo.join("app.log"), "INFO fine\nERROR boom\n");
    let line = format!("Monitor {id}: ERROR boom");
    let mut seen = false;
    until("a Turn that sees the monitor line", 15, || {
        seen = flow.requests("test/main").iter().any(|request| {
            serde_json::to_string(&request.messages)
                .unwrap()
                .contains(&line)
        });
        seen
    })
    .await;
    let rows = monitor_rows(&flow, &session).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].delivery, Delivery::Queue);
    assert_eq!(row_text(&rows[0]), line);
    let stopped = support::ok({
        let mut inv = flow
            .f
            .invocation("default", "task_stop", json!({"job_id": id}));
        inv.session_id = session.clone();
        flow.f.host.execute(inv, CancellationToken::new()).await
    });
    assert!(stopped.contains("cancelled"), "{stopped}");
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_command_and_file_sources_need_bash_permission() {
    let flow = flow(
        json!({"permissions": {"bash": {"*": "allow", "tail *": "deny", "rm *": "deny"}, "external_directory": "allow"}}),
        answers(2),
    );
    let session = flow.session("default").await;
    let refused =
        support::failed(monitor(&flow, &session, json!({"command": "rm -rf build"})).await);
    assert!(refused.to_lowercase().contains("denied"), "{refused}");
    let refused = support::failed(monitor(&flow, &session, json!({"path": "app.log"})).await);
    assert!(refused.to_lowercase().contains("denied"), "{refused}");
    let invalid = support::failed(
        monitor(
            &flow,
            &session,
            json!({"command": "true", "path": "app.log"}),
        )
        .await,
    );
    assert!(invalid.contains("exactly one"), "{invalid}");
    assert!(flow.runtime.jobs(Some(&session)).unwrap().is_empty());
}

#[tokio::test]
async fn monitor_url_needs_network_permission_and_streams_websocket_lines() {
    use futures::SinkExt as _;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/events", listener.local_addr().unwrap());
    let (go, ready) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ready.await;
        let message = "build ok\nERROR disk full\nDONE";
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                message.into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let denied = flow(json!({"permissions": {"network": "deny"}}), answers(1));
    let session = denied.session("default").await;
    let refused = support::failed(monitor(&denied, &session, json!({"url": url})).await);
    assert!(refused.contains("Network access to 127.0.0.1"), "{refused}");

    let flow = flow(json!({"permissions": {"network": "allow"}}), answers(5));
    let session = flow.session("default").await;
    let handle = start(
        &flow,
        &session,
        json!({"url": url, "filter": "ERROR", "until": "^DONE$"}),
    )
    .await;
    let id = handle["job_id"].as_str().unwrap().to_owned();
    assert_eq!(
        flow.runtime.job(&id).unwrap().command.as_deref(),
        Some(url.as_str())
    );
    go.send(()).unwrap();
    let job = terminal(&flow, &id).await;
    assert_eq!(job.status, JobStatus::Completed);
    let texts: Vec<String> = monitor_rows(&flow, &session)
        .await
        .iter()
        .map(row_text)
        .collect();
    assert_eq!(texts, [format!("Monitor {id}: ERROR disk full")]);
}

/// Regression (`background-tasks` → Monitor tool): a `url` monitor whose WebSocket
/// closes ended its log with "the command exited with code 0", naming a command it
/// never had; it now says the source closed.
#[tokio::test]
async fn monitor_url_end_note_says_the_source_closed() {
    use futures::SinkExt as _;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/events", listener.local_addr().unwrap());
    let (go, ready) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ready.await;
        let line = tokio_tungstenite::tungstenite::Message::Text("deploy step 1".into());
        socket.send(line).await.unwrap();
        socket.close(None).await.unwrap();
    });
    let flow = flow(json!({"permissions": {"network": "allow"}}), answers(5));
    let session = flow.session("default").await;
    let handle = start(&flow, &session, json!({"url": url})).await;
    let id = handle["job_id"].as_str().unwrap().to_owned();
    go.send(()).unwrap();
    let job = terminal(&flow, &id).await;
    assert_eq!(job.status, JobStatus::Completed);
    let log = std::fs::read_to_string(job.output_path.unwrap()).unwrap();
    assert_eq!(
        log.lines().last(),
        Some("[monitor ended: the source closed]"),
        "{log}"
    );
}

/// Regression (`sandbox` → network off): with `sandbox.network: off` a `url` monitor is
/// refused before any ask and never connects, even with the `network` permission.
#[tokio::test]
async fn monitor_url_is_refused_when_the_sandbox_network_is_off() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("ws://{}/events", listener.local_addr().unwrap());
    let flow = flow(
        json!({"permissions": {"network": "allow"}, "sandbox": {"network": "off"}}),
        answers(1),
    );
    let session = flow.session("default").await;
    let refused = support::failed(monitor(&flow, &session, json!({"url": url})).await);
    assert_eq!(refused, "Sandbox denied: network 127.0.0.1");
    assert!(
        listener
            .accept()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
        "the monitor connected"
    );
    assert!(flow.runtime.jobs(Some(&session)).unwrap().is_empty());
}

/// Regression: a `path` monitor of a missing file ends, without queueing anything, when
/// a symbolic link to a `deny_read` file is created at that path after it started.
#[cfg(unix)]
#[tokio::test]
async fn monitor_path_ends_when_a_symlink_to_a_hidden_file_is_swapped_in() {
    let mut config = allowed();
    config["sandbox"] = json!({"profile": "ci", "profiles": {
        "ci": {"extends": "workspace", "deny_read": ["**/*.pem"]}
    }});
    let flow = flow(config, answers(2));
    flow.f.write("key.pem", "PRIVATE-KEY-MATERIAL\n");
    std::fs::create_dir_all(flow.f.repo.join("logs")).unwrap();
    let session = flow.session("default").await;
    let handle = start(&flow, &session, json!({"path": "logs/new.log"})).await;
    let id = handle["job_id"].as_str().unwrap();
    std::os::unix::fs::symlink(
        flow.f.repo.join("key.pem"),
        flow.f.repo.join("logs/new.log"),
    )
    .unwrap();
    let job = terminal(&flow, id).await;
    assert_eq!(job.status, JobStatus::Completed);
    let rows = monitor_rows(&flow, &session).await;
    assert!(
        rows.iter().all(|row| !row_text(row).contains("PRIVATE")),
        "{:?}",
        rows.iter().map(row_text).collect::<Vec<_>>()
    );
    let log = std::fs::read_to_string(handle["output_path"].as_str().unwrap()).unwrap();
    assert!(!log.contains("PRIVATE"), "{log}");
    assert!(log.contains("[monitor ended:"), "{log}");
}
