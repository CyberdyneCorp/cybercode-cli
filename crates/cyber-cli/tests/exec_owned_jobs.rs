//! Exec waits for and cancels only the background work its own run started.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const SESSION: &str = "/api/v1/sessions/ses_owned";

fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
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

/// What the mocked server does for one test.
#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    /// A Job that was already running before the run sent its prompt.
    Baseline,
    /// The forked command's admission outcome is unknown.
    UnknownFork,
    /// A budget stop races a Job the Drain starts before acknowledging the interrupt.
    LateJobOnBudgetStop,
    /// A Job ended but its handback admission failed.
    FailedHandback,
}

#[derive(Default)]
struct State {
    sent: bool,
    interrupted: bool,
    late_job_stopped: bool,
    requests: Vec<String>,
}

struct Mock {
    scenario: Scenario,
    state: Mutex<State>,
    done: AtomicBool,
}

impl Mock {
    fn jobs(&self, state: &State) -> Value {
        let jobs = match self.scenario {
            Scenario::Baseline => json!([{"id":"job_old","status":"running","notified":false}]),
            Scenario::LateJobOnBudgetStop if state.interrupted => {
                let status = if state.late_job_stopped {
                    "cancelled"
                } else {
                    "running"
                };
                json!([{"id":"job_late","status":status,"notified":false}])
            }
            Scenario::FailedHandback if state.sent => {
                json!([{"id":"job_done","status":"completed","notified":false}])
            }
            _ => json!([]),
        };
        json!({"data":jobs,"cursor":{}})
    }

    /// Prior totals cost 10 (+20 for descendants); the prompt adds 2 (+4).
    fn snapshot(state: &State) -> Value {
        let (cost, children) = if state.sent {
            (12.0, 24.0)
        } else {
            (10.0, 20.0)
        };
        json!({"data":{"id":"ses_owned","directory":"/workspace","status":"idle",
            "totals":{"usage":{"input":10,"output":10,"reasoning":0,"cache_read":0,"cache_write":0},"cost":cost,"steps":2,"unpriced_steps":0},
            "children_cost":children,"children_tokens":100,"children_unpriced_steps":0,"children_usage_complete":true}})
    }

    fn respond(&self, method: &str, path: &str) -> Value {
        let mut state = self.state.lock().unwrap();
        state.requests.push(format!("{method} {path}"));
        let delegation = format!("{SESSION}/delegations/");
        if method == "GET" && path.starts_with("/api/v1/jobs?session_id=ses_owned&") {
            self.jobs(&state)
        } else if path == format!("{SESSION}/prompt") || path == format!("{SESSION}/command") {
            state.sent = true;
            json!({"data":{"message_id":"msg_command"}})
        } else if path == format!("{SESSION}/interrupt") {
            state.interrupted = true;
            json!({"data":{}})
        } else if method == "POST" && path == "/api/v1/jobs/job_late/stop" {
            state.late_job_stopped = true;
            json!({"data":{"id":"job_late","status":"cancelled"}})
        } else if method == "GET" && path.starts_with(&delegation) {
            let id = &path[delegation.len()..];
            json!({"data":{"id":id,"session_id":"ses_owned","status":"unknown","phase":"reserved","job_id":null,"error":"admission lost"}})
        } else if method == "GET" && path == SESSION {
            Self::snapshot(&state)
        } else {
            panic!("Unexpected request {method} {path}")
        }
    }

    /// The live stream: events once the run reached the point the scenario needs,
    /// then held open so only exec decides when the run is over.
    fn stream(&self, stream: &mut TcpStream) {
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.flush();
        let ready = |state: &State| match self.scenario {
            Scenario::LateJobOnBudgetStop => state.interrupted,
            _ => state.sent,
        };
        while !self.done.load(Ordering::SeqCst) && !ready(&self.state.lock().unwrap()) {
            thread::sleep(Duration::from_millis(5));
        }
        if self.scenario == Scenario::FailedHandback {
            let error = json!({"type":"session.error","data":{"session_id":"ses_owned","kind":"background_settlement","message":"admission fenced"}});
            let _ = write!(stream, "data: {error}\n\n");
        }
        let idle = json!({"type":"session.idle","data":{"session_id":"ses_owned"}});
        let _ = write!(stream, "data: {idle}\n\n");
        let _ = stream.flush();
        while !self.done.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn serve(self: Arc<Self>, listener: TcpListener) {
        let mut workers = Vec::new();
        while !self.done.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            let mock = self.clone();
            workers.push(thread::spawn(move || {
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let Some((method, path)) = read_request(&mut stream) else {
                    return;
                };
                if path.starts_with("/api/v1/event") {
                    mock.stream(&mut stream);
                    return;
                }
                let body = mock.respond(&method, &path).to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }
}

struct Outcome {
    output: std::process::Output,
    elapsed: Duration,
    requests: Vec<String>,
}

impl Outcome {
    fn detail(&self) -> String {
        format!(
            "status={:?}\nstdout={}\nstderr={}\nrequests={:#?}",
            self.output.status.code(),
            String::from_utf8_lossy(&self.output.stdout),
            String::from_utf8_lossy(&self.output.stderr),
            self.requests
        )
    }

    fn result(&self) -> Value {
        serde_json::from_slice(&self.output.stdout)
            .unwrap_or_else(|error| panic!("{error}\n{}", self.detail()))
    }
}

/// Runs exec against the mocked server, killing it after 30 s so a hang fails.
fn run(scenario: Scenario, extra: &[&str]) -> Outcome {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let mock = Arc::new(Mock {
        scenario,
        state: Mutex::default(),
        done: AtomicBool::new(false),
    });
    let server = thread::spawn({
        let mock = mock.clone();
        move || mock.serve(listener)
    });
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["exec", "--attach", &url, "--session", "ses_owned"])
        .args(["--quiet", "--format", "json"])
        .args(extra)
        .arg("inspect")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let id = child.id();
    let (exited, deadline) = mpsc::channel::<()>();
    let killer = thread::spawn(move || {
        if deadline.recv_timeout(Duration::from_secs(30)) == Err(mpsc::RecvTimeoutError::Timeout) {
            let _ = Command::new("kill").args(["-9", &id.to_string()]).status();
        }
    });
    let output = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    let _ = exited.send(());
    killer.join().unwrap();
    mock.done.store(true, Ordering::SeqCst);
    server.join().unwrap();
    let requests = std::mem::take(&mut mock.state.lock().unwrap().requests);
    Outcome {
        output,
        elapsed,
        requests,
    }
}

#[test]
fn a_job_running_before_the_run_is_neither_awaited_nor_cancelled() {
    let outcome = run(Scenario::Baseline, &[]);
    let detail = outcome.detail();
    assert!(outcome.output.status.success(), "{detail}");
    assert!(outcome.elapsed < Duration::from_secs(10), "{detail}");
    assert!(
        !outcome.requests.iter().any(|r| r.contains("/stop")),
        "{detail}"
    );
}

#[test]
fn an_unknown_forked_admission_ends_the_run_naming_the_delegation() {
    let outcome = run(Scenario::UnknownFork, &["--command", "audit"]);
    let detail = outcome.detail();
    assert_eq!(outcome.output.status.code(), Some(1), "{detail}");
    let error = outcome.result()["error"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        error.contains("needs reconciliation") && error.contains("op_skill_"),
        "{detail}"
    );
}

#[test]
fn a_budget_stop_cancels_an_owned_job_started_before_the_interrupt_was_acknowledged() {
    let outcome = run(Scenario::LateJobOnBudgetStop, &["--max-cost", "3"]);
    let detail = outcome.detail();
    assert!(
        outcome
            .requests
            .iter()
            .any(|r| r == "POST /api/v1/jobs/job_late/stop"),
        "the late owned Job was never cancelled\n{detail}"
    );
    assert_eq!(outcome.output.status.code(), Some(4), "{detail}");
    let result = outcome.result();
    assert_eq!(result["stop_reason"], "budget_exceeded", "{detail}");
    assert!(result["error"].is_null(), "{detail}");
}

#[test]
fn a_failed_handback_admission_ends_the_run_naming_the_job() {
    let outcome = run(Scenario::FailedHandback, &[]);
    let detail = outcome.detail();
    assert!(outcome.elapsed < Duration::from_secs(20), "{detail}");
    assert_eq!(outcome.output.status.code(), Some(1), "{detail}");
    let error = outcome.result()["error"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        error.contains("handback was not admitted") && error.contains("job_done"),
        "{detail}"
    );
}
