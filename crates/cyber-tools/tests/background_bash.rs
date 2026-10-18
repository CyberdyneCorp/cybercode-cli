//! `bash` `background: true` through real dispatch and runtime-owned process Jobs
//! (`background-tasks` → Background bash, Completion notices, Reading job output, Stop
//! kills process trees, Concurrency limits, Jobs end with their session).
#![cfg(unix)]
mod support;
use std::path::Path;
use std::time::{Duration, Instant};

use cyber_server::runtime::{Job, JobStatus, LiveEvent, ToolHost};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

const ALLOW: &str = r#"{"permissions":{"bash":"allow","external_directory":"allow"}}"#;

fn flow(scripts: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> Flow {
    let flow = Flow::new(scripts, false);
    flow.f.set_config(serde_json::from_str(ALLOW).unwrap());
    flow
}

async fn execute(
    flow: &Flow,
    session: &str,
    name: &str,
    input: Value,
) -> cyber_server::runtime::ToolOutcome {
    let mut inv = flow.f.invocation("default", name, input);
    inv.session_id = session.into();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

/// Start `command` in the background of `session` and return the tool's handle.
async fn start(flow: &Flow, session: &str, mut input: Value) -> Value {
    input["background"] = json!(true);
    let output = support::ok(execute(flow, session, "bash", input).await);
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
    until("the job to settle", 20, || {
        let current = flow.runtime.job(id).unwrap();
        let settled = current.status != JobStatus::Running && current.notified;
        job = Some(current);
        settled
    })
    .await;
    job.unwrap()
}

fn log(handle: &Value) -> String {
    std::fs::read_to_string(handle["output_path"].as_str().unwrap()).unwrap_or_default()
}

/// A number no other process's command line contains, to find this test's processes
/// without PIDs (inside a Linux PID namespace they differ from the host's).
fn marker() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    1_000_000 + u64::from(nanos % 1_000_000)
}

/// Whether a process whose command line contains `pattern` is running.
fn running(pattern: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// The JSON of the completion notice admitted for `job`.
async fn notice(flow: &Flow, session: &str, job: &str) -> Option<Value> {
    let state = flow.runtime.state(session).await.unwrap();
    let row = state
        .inbox
        .iter()
        .find(|row| row.message_id == format!("msg_handback_{job}"))?;
    let parts = serde_json::to_value(&row.parts).unwrap();
    let text = parts[0]["text"].as_str().unwrap();
    let body = text.strip_prefix("Background job ended: ").expect(text);
    Some(serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn background_bash_returns_job_id_and_log_path_and_survives_turn_end() {
    let command = "echo started; while [ ! -f go ]; do sleep 0.05; done; echo finished";
    let flow = flow(vec![
        call(
            "bg",
            "bash",
            json!({"command": command, "background": true, "description": "wait for go"}),
        ),
        text("started it"),
        text("notice handled"),
    ]);
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "start the job").await;
    flow.settle(&session).await;
    let handle: Value = serde_json::from_str(&flow.output(&session, "bg").await).unwrap();
    let id = handle["job_id"].as_str().unwrap();
    assert!(id.starts_with("job_"), "{handle}");
    assert_eq!(handle["status"], "running");
    let path = Path::new(handle["output_path"].as_str().unwrap());
    let data = std::fs::canonicalize(flow.f.dir.path()).unwrap();
    assert_eq!(path, data.join("jobs").join(format!("{id}.log")));
    // The Turn ended; the Job did not.
    let job = flow.runtime.job(id).unwrap();
    assert_eq!(job.status, JobStatus::Running);
    assert_eq!(job.kind, "bash");
    assert_eq!(job.command.as_deref(), Some(command));
    assert_eq!(job.description, "wait for go");
    until("started in the log", 10, || {
        log(&handle).contains("started")
    })
    .await;
    std::fs::write(flow.f.repo.join("go"), "").unwrap();
    let job = terminal(&flow, id).await;
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(job.exit_code, Some(0));
    assert_eq!(log(&handle), "started\nfinished\n");
    let mut streamed = String::new();
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::JobOutput {
            session_id,
            job_id,
            text,
        } = event
        {
            assert_eq!(
                (session_id.as_str(), job_id.as_str()),
                (session.as_str(), id)
            );
            streamed.push_str(&text);
        }
    }
    assert_eq!(streamed, "started\nfinished\n");
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 3, "the notice starts a Turn");
}

#[tokio::test]
async fn background_bash_notice_has_exit_code_duration_and_last_20_lines() {
    let flow = flow(vec![text("notice handled")]);
    let session = flow.session("default").await;
    let handle = start(
        &flow,
        &session,
        json!({"command": "for i in $(seq 1 30); do echo line$i; done; sleep 0.2; exit 3"}),
    )
    .await;
    let id = handle["job_id"].as_str().unwrap();
    let job = terminal(&flow, id).await;
    assert_eq!(job.status, JobStatus::Error);
    assert_eq!(job.exit_code, Some(3));
    let notice = notice(&flow, &session, id).await.expect("notice admitted");
    assert_eq!(notice["job"]["id"], id);
    assert_eq!(notice["job"]["status"], "error");
    assert_eq!(notice["job"]["exit_code"], 3);
    assert!(notice["duration_ms"].as_i64().unwrap() >= 200, "{notice}");
    let expected: Vec<String> = (11..=30).map(|i| format!("line{i}")).collect();
    assert_eq!(notice["output_tail"], expected.join("\n"));
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn notify_false_suppresses_the_completion_notice() {
    let flow = flow(vec![text("unexpected")]);
    let session = flow.session("default").await;
    let handle = start(
        &flow,
        &session,
        json!({"command": "echo quiet", "notify": false}),
    )
    .await;
    let id = handle["job_id"].as_str().unwrap();
    let job = terminal(&flow, id).await;
    assert_eq!(job.status, JobStatus::Completed);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(notice(&flow, &session, id).await.is_none());
    flow.runtime.recover_jobs().await.unwrap();
    assert!(notice(&flow, &session, id).await.is_none());
    assert!(flow.main.requests().is_empty());
    let refused = support::failed(
        execute(
            &flow,
            &session,
            "bash",
            json!({"command": "true", "notify": false}),
        )
        .await,
    );
    assert!(
        refused.contains("notify applies only with background"),
        "{refused}"
    );
}

#[tokio::test]
async fn read_pages_a_running_job_log_without_stopping_it() {
    let flow = flow(Vec::new());
    let session = flow.session("default").await;
    let handle = start(
        &flow,
        &session,
        json!({"command": "seq 1 300 | sed 's/^/line/'; while [ ! -f go ]; do sleep 0.05; done", "notify": false}),
    )
    .await;
    until("300 lines", 10, || log(&handle).lines().count() == 300).await;
    let page = support::ok(
        execute(
            &flow,
            &session,
            "read",
            json!({"path": handle["output_path"], "offset": 101, "limit": 200}),
        )
        .await,
    );
    assert!(page.starts_with("101: line101\n"), "{page}");
    assert!(page.contains("300: line300"), "{page}");
    assert!(!page.contains("100: line100"), "{page}");
    let id = handle["job_id"].as_str().unwrap();
    assert_eq!(flow.runtime.job(id).unwrap().status, JobStatus::Running);
    std::fs::write(flow.f.repo.join("go"), "").unwrap();
    assert_eq!(terminal(&flow, id).await.status, JobStatus::Completed);
}

#[tokio::test]
async fn job_log_cap_drops_oldest_bytes_with_marker() {
    let flow = flow(vec![text("notice handled")]);
    flow.f.set_config(json!({
        "permissions": {"bash": "allow"},
        "background": {"max_output_mb": 1}
    }));
    let session = flow.session("default").await;
    let handle = start(&flow, &session, json!({"command": "seq 1 300000"})).await;
    let job = terminal(&flow, handle["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    let content = log(&handle);
    assert!(content.len() <= 1024 * 1024, "{}", content.len());
    let (marker, kept) = content.split_once('\n').unwrap();
    assert!(marker.starts_with("[output truncated: "), "{marker}");
    assert!(marker.contains("background.max_output_mb is 1"), "{marker}");
    let dropped: usize = marker["[output truncated: ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let full: usize = (1..=300_000).map(|n: u32| n.to_string().len() + 1).sum();
    assert_eq!(dropped + kept.len(), full);
    // The kept part starts on a line boundary and ends with the newest output.
    let first: u32 = kept.lines().next().unwrap().parse().unwrap();
    assert!(first > 1);
    assert!(kept.ends_with("299999\n300000\n"));
    let notice = notice(&flow, &session, &job.id).await.unwrap();
    assert!(notice["output_tail"].as_str().unwrap().ends_with("300000"));
}

#[tokio::test]
async fn task_stop_terminates_grandchildren_and_records_cancelled() {
    let flow = flow(vec![text("cancel notice handled")]);
    let session = flow.session("default").await;
    let (polite, stubborn) = (marker(), marker() + 1);
    // One grandchild exits on SIGTERM; the other ignores it and needs SIGKILL.
    flow.f.write(
        "stubborn.sh",
        &format!("trap '' TERM\ntouch stubborn.ready\nwhile :; do sleep 1; done\n: {stubborn}\n"),
    );
    let command = format!(
        "sleep {polite} & sh stubborn.sh {stubborn} & \
         while [ ! -f stubborn.ready ]; do sleep 0.05; done; echo ready; wait"
    );
    let handle = start(&flow, &session, json!({"command": command})).await;
    until("ready", 10, || log(&handle).contains("ready")).await;
    let (polite, stubborn) = (format!("sleep {polite}"), format!("stubborn.sh {stubborn}"));
    assert!(running(&polite) && running(&stubborn));
    let stopped = Instant::now();
    let output = support::ok(
        execute(
            &flow,
            &session,
            "task_stop",
            json!({"job_id": handle["job_id"]}),
        )
        .await,
    );
    let elapsed = stopped.elapsed();
    let output: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output["status"], "cancelled");
    // SIGTERM first; what survives it is killed within the 5 s grace.
    assert!(elapsed < Duration::from_secs(9), "{elapsed:?}");
    until("grandchildren to exit", 5, || {
        !running(&polite) && !running(&stubborn)
    })
    .await;
    let job = terminal(&flow, handle["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Cancelled);
    assert_eq!(job.exit_code, None);
}

#[tokio::test]
async fn task_stop_kills_after_the_grace_when_sigterm_is_ignored() {
    let flow = flow(vec![text("cancel notice handled")]);
    let session = flow.session("default").await;
    let tag = marker();
    flow.f.write(
        "deaf.sh",
        "trap '' TERM\ntouch deaf.ready\nwhile :; do sleep 1; done\n",
    );
    let handle = start(
        &flow,
        &session,
        json!({"command": format!("exec sh deaf.sh {tag}")}),
    )
    .await;
    until("ready", 10, || flow.f.repo.join("deaf.ready").exists()).await;
    let tag = format!("deaf.sh {tag}");
    let stopped = Instant::now();
    let job = flow
        .runtime
        .cancel_job(handle["job_id"].as_str().unwrap())
        .await
        .unwrap();
    let elapsed = stopped.elapsed();
    assert_eq!(job.status, JobStatus::Cancelled);
    assert!(
        elapsed >= Duration::from_millis(4500),
        "SIGKILL came before the grace: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(9), "{elapsed:?}");
    until("the process to exit", 5, || !running(&tag)).await;
}

#[tokio::test]
async fn twenty_first_job_fails_with_limit_message() {
    let flow = flow(Vec::new());
    let session = flow.session("default").await;
    for _ in 0..20 {
        start(
            &flow,
            &session,
            json!({"command": "sleep 60", "notify": false}),
        )
        .await;
    }
    let refused = support::failed(
        execute(
            &flow,
            &session,
            "bash",
            json!({"command": "sleep 60", "background": true}),
        )
        .await,
    );
    assert_eq!(refused, "Background job limit reached (20)");
    // Another Session has its own limit.
    let other = flow.session("default").await;
    start(&flow, &other, json!({"command": "true", "notify": false})).await;
    flow.runtime.delete(&session).await.unwrap();
    flow.runtime.delete(&other).await.unwrap();
    assert!(flow.runtime.jobs(None).unwrap().is_empty());
}

#[tokio::test]
async fn interrupt_keeps_background_dev_server_running() {
    let tag = marker();
    let server = format!("while :; do echo tick; sleep 0.05; done; : {tag}");
    let flow = flow(vec![
        call(
            "dev",
            "bash",
            json!({"command": server, "background": true, "notify": false}),
        ),
        call(
            "slow",
            "bash",
            json!({"command": "touch slow.started; sleep 30"}),
        ),
        text("after interrupt"),
    ]);
    let session = flow.session("default").await;
    flow.prompt(&session, "start the dev server, then wait")
        .await;
    until("the foreground command", 10, || {
        flow.f.repo.join("slow.started").exists()
    })
    .await;
    flow.runtime.interrupt(&session).await.unwrap();
    flow.settle(&session).await;
    let handle: Value = serde_json::from_str(&flow.output(&session, "dev").await).unwrap();
    let id = handle["job_id"].as_str().unwrap();
    assert_eq!(flow.runtime.job(id).unwrap().status, JobStatus::Running);
    let tag = format!(": {tag}");
    assert!(running(&tag));
    let ticks = log(&handle).lines().count();
    until("more output", 10, || log(&handle).lines().count() > ticks).await;
    flow.runtime.cancel_job(id).await.unwrap();
    until("the dev server to exit", 5, || !running(&tag)).await;
}

#[tokio::test]
async fn deleting_session_cancels_bash_jobs() {
    let flow = flow(Vec::new());
    let session = flow.session("default").await;
    let tag = marker();
    let handle = start(&flow, &session, json!({"command": format!("sleep {tag}")})).await;
    let tag = format!("sleep {tag}");
    until("the job's process", 10, || running(&tag)).await;
    flow.runtime.delete(&session).await.unwrap();
    until("the job's process to exit", 5, || !running(&tag)).await;
    assert!(
        flow.runtime
            .find_job(handle["job_id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    assert!(flow.main.requests().is_empty(), "no notice after deletion");
}
