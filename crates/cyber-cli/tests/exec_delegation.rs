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

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(budget: bool, acknowledge: bool, supported: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let log = requests.clone();
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let mut polls = 0;
            'connections: while !stopping.load(Ordering::Relaxed) {
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
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                let header_end = loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => continue 'connections,
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
                        Ok(0) | Err(_) => continue 'connections,
                        Ok(n) => n,
                    };
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let body = serde_json::from_slice(&bytes[header_end..]).unwrap_or(Value::Null);
                log.lock().unwrap().push((path.clone(), body));
                let job = |status| json!({"id":"job_named", "session_id":"ses_parent", "child_id":"ses_child", "status":status, "result":{"preview":"truncated"}});
                let response = match path.strip_prefix("/api/v1").unwrap() {
                    "/agents" => json!({"data":[{"name":"explore", "mode":"subagent"}]}),
                    "/openapi.json" => {
                        json!({"components":{"schemas":{"SubtaskBody":{"properties": if supported {json!({"agent":{}, "attachments":{}, "max_steps":{}})} else {json!({"prompt":{}})}}}}})
                    }
                    "/sessions/ses_parent" => {
                        json!({"data":{"id":"ses_parent", "directory":"/workspace"}})
                    }
                    "/sessions/ses_parent/subtask" => json!({"data":job("running")}),
                    "/sessions/ses_child" => {
                        json!({"data":{"id":"ses_child", "parent_id":"ses_parent", "status":"idle", "agent":"explore", "mode":"dont-ask", "totals":{"usage":{"input":2,"output":3,"reasoning":20,"cache_read":4,"cache_write":5},"steps":1,"cost":0.25,"unpriced_steps":1}}})
                    }
                    "/jobs/job_named" => {
                        polls += 1;
                        json!({"data":job(if budget || polls == 1 {"running"} else {"completed"})})
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
                let response = response.to_string();
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
    fn run(&self, extra: &[&str]) -> (std::process::Output, Value) {
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
        2
    );
    let body = &requests
        .iter()
        .find(|(path, _)| path.ends_with("/subtask"))
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
            .any(|(path, _)| path.ends_with("/subtask"))
    );
}

#[test]
fn timeout_cancels_the_returned_job_without_interrupting_parent() {
    let server = Server::new(true, true, true);
    let (output, result) = server.run(&["--timeout", "1ms"]);
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
