//! Ordinary exec observes durable own/descendant deltas without rebilling history.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let end = bytes.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
    while bytes.len() < end + length {
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let mut parts = header.lines().next()?.split_whitespace();
    Some((parts.next()?.into(), parts.next()?.into()))
}

fn send_idle(
    stream: &mut std::net::TcpStream,
    stop: &AtomicBool,
    cancelled: &AtomicBool,
    acknowledge: bool,
) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();
    let started = Instant::now();
    while !stop.load(Ordering::SeqCst)
        && (!acknowledge
            || (!cancelled.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(2)))
    {
        thread::sleep(Duration::from_millis(5));
    }
    let event = json!({"type":"session.idle","data":{"session_id":"ses_ordinary"}});
    let _ = write!(stream, "data: {event}\n\n");
}

fn run(legacy: bool, budget: bool) -> (std::process::Output, bool) {
    run_with_ack(legacy, budget, true)
}

fn run_with_ack(legacy: bool, budget: bool, acknowledge: bool) -> (std::process::Output, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let submitted = Arc::new(AtomicBool::new(false));
    let interrupted = Arc::new(AtomicBool::new(false));
    let (stop, sent, cancelled) = (done.clone(), submitted.clone(), interrupted.clone());
    let server = thread::spawn(move || {
        let mut workers = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            let (sent, cancelled, stop) = (sent.clone(), cancelled.clone(), stop.clone());
            workers.push(thread::spawn(move || {
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let Some((method, path)) = read_request(&mut stream) else { return; };
                if path.starts_with("/api/v1/event") {
                    send_idle(&mut stream, &stop, &cancelled, acknowledge);
                    return;
                }
                let body = if path == "/api/v1/agents" { json!({"data":[]}) }
                else if path == "/api/v1/sessions/ses_ordinary/prompt" {
                    sent.store(true, Ordering::SeqCst); json!({"data":{}})
                } else if path == "/api/v1/sessions/ses_ordinary/interrupt" {
                    cancelled.store(true, Ordering::SeqCst); json!({"data":{}})
                } else if method == "GET" && path == "/api/v1/sessions/ses_ordinary" {
                    let current = sent.load(Ordering::SeqCst);
                    let mut data = json!({"id":"ses_ordinary","directory":"/workspace","totals":{"usage":{"input":if current {11} else {10},"output":if current {12} else {10},"reasoning":if current {13} else {10},"cache_read":if current {14} else {10},"cache_write":if current {15} else {10}},"cost":if current {12.0} else {10.0},"steps":if current {3} else {2},"unpriced_steps":0}});
                    if !legacy {
                        data["children_cost"] = json!(if current {24.0} else {20.0});
                        data["children_tokens"] = json!(if current {115} else {100});
                        data["children_unpriced_steps"] = json!(0);
                        data["children_usage_complete"] = json!(true);
                    }
                    json!({"data":data})
                } else { panic!("Unexpected request {method} {path}") };
                let body = body.to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command.args([
        "exec",
        "--attach",
        &url,
        "--session",
        "ses_ordinary",
        "--quiet",
        "--format",
        "json",
        "inspect",
    ]);
    if budget {
        command.args(["--max-cost", "3"]);
    }
    let output = command.stdin(Stdio::null()).output().unwrap();
    done.store(true, Ordering::SeqCst);
    server.join().unwrap();
    (output, submitted.load(Ordering::SeqCst))
}

#[test]
fn ordinary_exec_reports_current_subtree_spending_without_prior_history() {
    let (output, submitted) = run(false, false);
    assert!(submitted);
    assert!(
        output.status.success(),
        "{}",
        format_args!(
            "stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["cost_usd"], 6.0);
    assert_eq!(result["own_cost_usd"], 2.0);
    assert_eq!(result["total_tokens"], 30);
    assert_eq!(result["usage"]["cache_write"], 5);
}

#[test]
fn ordinary_exec_budget_observes_descendants_without_parent_step_events() {
    let (output, _) = run(false, true);
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        format_args!(
            "stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    );
}

#[test]
fn ordinary_budget_refuses_unknown_descendant_billing_before_prompt() {
    let (output, submitted) = run(true, true);
    assert!(!submitted);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn unbudgeted_legacy_exec_discloses_unknown_descendants() {
    let (output, submitted) = run(true, false);
    assert!(submitted);
    assert!(
        output.status.success(),
        "{}",
        format_args!(
            "stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["cost_usd"], 2.0);
    assert_eq!(result["children_usage_complete"], false);
    assert_eq!(result["cost_unpriced"], true);
}

#[test]
fn unacknowledged_ordinary_budget_stop_is_bounded_and_reports_uncertainty() {
    let started = Instant::now();
    let (output, _) = run_with_ack(false, true, false);
    assert!(started.elapsed() < Duration::from_secs(10));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 4);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("not acknowledged")
    );
}
