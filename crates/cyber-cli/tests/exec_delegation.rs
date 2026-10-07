//! Exercise the binary against the public child Job/history contract.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
enum AdmissionMode {
    Immediate,
    Queued,
    Unknown,
    LostResponse,
    Foreign,
    Legacy,
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(budget: bool, acknowledge: bool, supported: bool) -> Self {
        Self::with_admission(budget, acknowledge, supported, AdmissionMode::Immediate)
    }
    fn with_admission(
        budget: bool,
        acknowledge: bool,
        supported: bool,
        mode: AdmissionMode,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let log = requests.clone();
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let mut polls = 0;
            let mut stopped = false;
            let mut submitted = false;
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let Some((method, path, body)) = read_request(&mut stream) else {
                    continue;
                };
                log.lock().unwrap().push((path.clone(), body));
                let job = |status| json!({"id":"job_named", "session_id":"ses_parent", "child_id":"ses_child", "status":status, "result":{"preview":"truncated"}});
                let response = match path.strip_prefix("/api/v1").unwrap() {
                    "/agents" => json!({"data":[{"name":"explore", "mode":"subagent"}]}),
                    "/openapi.json" => {
                        json!({"paths":if supported && mode!=AdmissionMode::Legacy {json!({"/api/v1/sessions/{sessionID}/delegations/{requestID}":{"post":{},"get":{}},"/api/v1/sessions/{sessionID}/delegations/{requestID}/stop":{"post":{}}})} else {json!({})},"components":{"schemas":{"SubtaskBody":{"properties": if supported {json!({"agent":{}, "attachments":{}, "max_steps":{}})} else {json!({"prompt":{}})}}}}})
                    }
                    "/sessions/ses_parent" => {
                        json!({"data":{"id":"ses_parent", "directory":"/workspace"}})
                    }
                    path if path.starts_with("/sessions/ses_parent/delegations/") => {
                        admission_response(mode, &method, path, &mut submitted, &mut stopped)
                    }
                    "/sessions/ses_child" => {
                        json!({"data":{"id":"ses_child", "parent_id":"ses_parent", "status":"idle", "agent":"explore", "mode":"dont-ask", "totals":{"usage":{"input":2,"output":3,"reasoning":20,"cache_read":4,"cache_write":5},"steps":1,"cost":0.25,"unpriced_steps":1}}})
                    }
                    "/jobs/job_named" => {
                        polls += 1;
                        json!({"data":job(if budget || polls <= 2 {"running"} else {"completed"})})
                    }
                    "/jobs/job_named/stop" => {
                        json!({"data":job(if acknowledge {"cancelled"} else {"running"})})
                    }
                    "/sessions/ses_child/history?after=-1&limit=500" => {
                        json!({"data":[event(0,"session.text.ended.1",json!({"text":"x".repeat(20_000)}))], "hasMore":true})
                    }
                    "/sessions/ses_child/history?after=0&limit=500" => {
                        json!({"data":[event(1,"session.tool.called.1",json!({"call_id":"call_1", "name":"write", "input":{"path":"private"}})),event(2,"session.tool.settled.1",json!({"call_id":"call_1", "status":"error", "output":"Permission denied"}))],"hasMore":false})
                    }
                    "/sessions/ses_child/history?after=2&limit=500" => {
                        json!({"data":[],"hasMore":false})
                    }
                    unexpected => panic!("Unexpected request: {unexpected}"),
                };
                let response = if mode == AdmissionMode::LostResponse
                    && method == "POST"
                    && path.contains("/delegations/")
                    && !path.ends_with("/stop")
                {
                    "{".into()
                } else {
                    response.to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn command(&self, extra: &[&str]) -> (tempfile::TempDir, Command) {
        let dir = tempfile::tempdir().unwrap();
        let attachment = dir.path().join("note.txt");
        std::fs::write(&attachment, "attached context").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args([
                "exec",
                "--attach",
                &self.url,
                "--session",
                "ses_parent",
                "--quiet",
                "--format",
                "json",
                "--file",
                attachment.to_str().unwrap(),
                "--max-turns",
                "2",
            ])
            .args(extra)
            .arg("@explore find retry logic")
            .env("HOME", dir.path())
            .env("CYBER_HOME", dir.path().join("cyber"))
            .current_dir(dir.path())
            .stdin(Stdio::null());
        (dir, command)
    }
    fn run(&self, extra: &[&str]) -> (std::process::Output, Value) {
        let (_dir, mut command) = self.command(extra);
        let output = command.output().unwrap();
        let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        (output, value)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Err(panic) = self.thread.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}
fn event(seq: i64, kind: &str, data: Value) -> Value {
    json!({"type":kind,"data":data,"durable":{"aggregateID":"ses_child","seq":seq}})
}

#[test]
fn fixture_waits_for_request_bytes_delivered_after_connection_acceptance() {
    let server = Server::new(false, true, true);
    let mut stream =
        std::net::TcpStream::connect(server.url.trim_start_matches("http://")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    stream
        .write_all(b"GET /api/v1/agents HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("explore"));
}

#[test]
fn exec_reads_full_paginated_child_output_and_waits_for_job_settlement() {
    let server = Server::new(false, true, true);
    let (output, result) = server.run(&["--fail-on-deny"]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(result["result"], "x".repeat(20_000));
    assert_eq!(result["session_id"], "ses_child");
    assert_eq!(result["parent_session_id"], "ses_parent");
    assert_eq!(result["job_id"], "job_named");
    assert_eq!(result["usage"]["reasoning"], 20);
    assert_eq!(result["cost_unpriced"], true);
    assert_eq!(result["denials"].as_array().unwrap().len(), 1);
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(path, _)| path == "/api/v1/jobs/job_named")
            .count(),
        3
    );
    let body = &requests
        .iter()
        .find(|(_, body)| body["prompt"] == "find retry logic")
        .unwrap()
        .1;
    assert_eq!(body["agent"], "explore");
    assert_eq!(body["prompt"], "find retry logic");
    assert_eq!(body["max_steps"], 2);
    assert!(
        body["attachments"][0]
            .to_string()
            .contains("attached context")
    );
    assert!(!requests.iter().any(|(path, _)| path.ends_with("/stop")));
}

#[test]
fn hidden_usage_budget_stops_only_owned_child_job() {
    let server = Server::new(true, true, true);
    let (output, result) = server.run(&["--max-tokens", "10"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(result["stop_reason"], "budget_exceeded");
    assert!(result["error"].is_null());
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(path, _)| path.ends_with("/stop"))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .any(|(path, _)| path == "/api/v1/jobs/job_named/stop")
    );
}

#[test]
fn unacknowledged_cancellation_is_reported_with_owned_identity() {
    let server = Server::new(true, false, true);
    let (output, result) = server.run(&["--max-tokens", "10"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("cancellation was not acknowledged; inspect job_named")
    );
}

#[test]
fn older_server_cannot_silently_drop_delegation_fields() {
    let server = Server::new(false, true, false);
    let (output, _) = server.run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path.contains("/delegations/"))
    );
}

#[test]
fn timeout_cancels_the_returned_job_without_interrupting_parent() {
    let server = Server::new(true, true, true);
    let (output, result) = server.run(&["--timeout", "100ms"]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(result["stop_reason"], "timeout");
    assert!(result["error"].is_null());
    let requests = server.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|(path, _)| path == "/api/v1/jobs/job_named/stop")
    );
    assert!(!requests.iter().any(|(path, _)| path.contains("interrupt")));
}

#[test]
fn queued_admission_timeout_stops_the_same_request_without_a_child_job() {
    let server = Server::with_admission(false, true, true, AdmissionMode::Queued);
    let (output, result) = server.run(&["--timeout", "100ms"]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(result["stop_reason"], "timeout");
    assert_eq!(result["session_id"], "ses_parent");
    assert_eq!(result["parent_session_id"], "ses_parent");
    assert!(result["job_id"].is_null());
    assert!(result["error"].is_null());
    let id = result["delegation_id"].as_str().unwrap();
    assert!(id.starts_with("op_"));
    let requests = server.requests.lock().unwrap();
    let path = format!("/api/v1/sessions/ses_parent/delegations/{id}");
    assert!(
        requests
            .iter()
            .any(|(url, body)| url == &path && body["prompt"] == "find retry logic")
    );
    assert!(
        requests
            .iter()
            .any(|(url, _)| url == &format!("{path}/stop"))
    );
    assert!(
        requests
            .iter()
            .filter(|(url, _)| url.contains("/delegations/"))
            .all(|(url, _)| url == &path || url == &format!("{path}/stop"))
    );
    assert!(
        !requests
            .iter()
            .any(|(url, _)| url.starts_with("/api/v1/jobs/")
                || url.contains("interrupt")
                || url.contains("ses_child"))
    );
}

#[test]
fn a_lost_post_response_recovers_the_recorded_job_without_redispatch() {
    let server = Server::with_admission(false, true, true, AdmissionMode::LostResponse);
    let (output, result) = server.run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(result["job_id"], "job_named");
    assert_eq!(result["result"], "x".repeat(20_000));
    let id = result["delegation_id"].as_str().unwrap();
    let requests = server.requests.lock().unwrap();
    let path = format!("/api/v1/sessions/ses_parent/delegations/{id}");
    assert_eq!(
        requests
            .iter()
            .filter(|(url, body)| url == &path && body["prompt"].is_string())
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .any(|(url, body)| url == &path && body.is_null())
    );
}

#[test]
fn unknown_admission_reports_its_identity_and_never_adopts_another_job() {
    let server = Server::with_admission(false, true, true, AdmissionMode::Unknown);
    let (output, result) = server.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    let id = result["delegation_id"].as_str().unwrap();
    let error = result["error"].as_str().unwrap();
    assert!(error.contains(id));
    assert!(error.contains("cancellation was not acknowledged"));
    assert!(result["job_id"].is_null());
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(url, _)| url.starts_with("/api/v1/jobs/") || url.contains("interrupt"))
    );
}

#[test]
fn mismatched_admission_identity_cannot_redirect_job_following_or_cancellation() {
    let server = Server::with_admission(false, true, true, AdmissionMode::Foreign);
    let (output, result) = server.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("identity changed")
    );
    assert_ne!(result["delegation_id"], "op_foreign");
    let requests = server.requests.lock().unwrap();
    assert!(
        !requests
            .iter()
            .any(|(url, _)| url.starts_with("/api/v1/jobs/") || url.contains("op_foreign"))
    );
}

#[test]
fn an_older_typed_subtask_endpoint_cannot_replace_durable_admission() {
    let server = Server::with_admission(false, true, true, AdmissionMode::Legacy);
    let (output, _) = server.run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(url, _)| url.contains("/delegations/") || url.ends_with("/subtask"))
    );
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, String, Value)> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let header_end = loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => n,
        };
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(index) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let method = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => n,
        };
        bytes.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&bytes[header_end..]).unwrap_or(Value::Null);
    Some((method, path, body))
}

fn admission_response(
    mode: AdmissionMode,
    method: &str,
    path: &str,
    submitted: &mut bool,
    stopped: &mut bool,
) -> Value {
    let stopping = path.ends_with("/stop");
    let id = path.trim_end_matches("/stop").rsplit('/').next().unwrap();
    let initial = method == "POST" && !stopping;
    if initial {
        *submitted = true;
    }
    let pending_ack = stopping && *submitted && mode == AdmissionMode::Queued && !*stopped;
    if stopping {
        *stopped = true;
    }
    let (status, phase, job) = match mode {
        AdmissionMode::Queued if pending_ack => ("cancelling", "reserved", None),
        AdmissionMode::Queued if *stopped => ("cancelled", "reserved", None),
        AdmissionMode::Queued => ("pending", "reserved", None),
        AdmissionMode::Unknown => ("unknown", "launching", None),
        _ if *stopped && !*submitted => ("cancelled", "reserved", None),
        _ => ("admitted", "launching", Some("job_named")),
    };
    json!({"data":{"id":if mode==AdmissionMode::Foreign {"op_foreign"} else {id},"session_id":"ses_parent","status":status,"phase":phase,"job_id":job,"error":if mode==AdmissionMode::Unknown {Some("Lost admission owner")} else {None}}})
}

#[cfg(unix)]
#[test]
fn interrupt_cancels_queued_admission_without_interrupting_parent() {
    let server = Server::with_admission(false, true, true, AdmissionMode::Queued);
    let (_dir, mut command) = server.command(&[]);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|(path, body)| path.contains("/delegations/") && body.is_null())
    {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Admission did not enter its queue");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let signal = Command::new("kill")
        .args(["-s", "INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Interrupted exec did not settle");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["stop_reason"], "interrupted");
    assert!(result["error"].is_null());
    assert!(result["job_id"].is_null());
    let path = format!(
        "/api/v1/sessions/ses_parent/delegations/{}/stop",
        result["delegation_id"].as_str().unwrap()
    );
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().any(|(url, _)| url == &path));
    assert!(
        !requests
            .iter()
            .any(|(url, _)| url.starts_with("/api/v1/jobs/")
                || url.contains("interrupt")
                || url.contains("ses_child"))
    );
}
