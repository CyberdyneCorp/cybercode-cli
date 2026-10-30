//! Fixed-fixture audit handshake; never used by a production plugin host.
use cyber_sandbox::plugin_policy::LinuxPolicy;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

enum Message {
    Stdout(String),
    Stderr(String),
    Fault(String),
}

fn reader(
    input: impl Read + Send + 'static,
    tx: Sender<Message>,
    stdout: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut input = BufReader::new(input);
        for _ in 0..128 {
            let mut bytes = Vec::new();
            match (&mut input).take(2049).read_until(b'\n', &mut bytes) {
                Ok(0) => return,
                Ok(_) if bytes.len() <= 2048 && bytes.ends_with(b"\n") => {}
                result => {
                    let _ = tx.send(Message::Fault(format!(
                        "bounded fixture stream: {result:?}"
                    )));
                    return;
                }
            }
            let Ok(line) = String::from_utf8(bytes) else {
                let _ = tx.send(Message::Fault("non-UTF8 fixture stream".into()));
                return;
            };
            let message = if stdout {
                Message::Stdout(line)
            } else {
                Message::Stderr(line)
            };
            if tx.send(message).is_err() {
                return;
            }
        }
        let _ = tx.send(Message::Fault("fixture stream exceeded 128 lines".into()));
    })
}

#[derive(Default)]
struct Trace {
    ready: Option<Value>,
    finished: Option<Value>,
    cancelled: Option<u32>,
    started: BTreeSet<u32>,
    reaped: BTreeSet<u32>,
    audit: Vec<String>,
}
impl Trace {
    fn record(&mut self, message: Message) -> Result<(), String> {
        match message {
            Message::Stdout(line) => self.output(&line),
            Message::Stderr(line) => self.event(&line),
            Message::Fault(error) => Err(error),
        }
    }
    fn output(&mut self, line: &str) -> Result<(), String> {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let slot = match value["phase"].as_str() {
            Some("cancel-ready") => &mut self.ready,
            Some("cancel-finished") => &mut self.finished,
            _ => return Err(format!("unexpected fixture phase: {value}")),
        };
        if slot.is_some() {
            return Err("duplicate fixture phase".into());
        }
        *slot = Some(value);
        Ok(())
    }
    fn event(&mut self, line: &str) -> Result<(), String> {
        self.audit.push(line.trim_end().into());
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        if words.len() != 2 {
            return Err(format!("invalid audit event: {line}"));
        }
        let pid: u32 = words[1].parse().map_err(|_| "invalid audit PID")?;
        if pid == 0 {
            return Err("zero audit PID".into());
        }
        match words[0] {
            "BSTART" => {
                self.reaped.remove(&pid);
                self.started.insert(pid);
            }
            "BCANCEL" => {
                self.cancelled = Some(pid);
            }
            "BREAP" => {
                self.reaped.insert(pid);
            }
            _ => return Err(format!("unknown audit event: {line}")),
        }
        Ok(())
    }
    fn can_release(&self) -> bool {
        let Some(pid) = self.cancelled else {
            return false;
        };
        self.ready
            .as_ref()
            .is_some_and(|v| v["client_syn_sent"] == true && v["cancelled"] == true)
            && self.started.contains(&pid)
            && self.reaped.contains(&pid)
    }
    fn report(&self) -> Value {
        json!({"ready":self.ready,"finished":self.finished,"cancelled_pid":self.cancelled,
            "started":self.started,"reaped":self.reaped,"audit":self.audit})
    }
}

fn drive(
    child: &mut Child,
    input: &mut Option<ChildStdin>,
    rx: &Receiver<Message>,
    trace: &mut Trace,
    released: &mut bool,
) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(message) = rx.recv_timeout(Duration::from_millis(30)) {
            trace.record(message)?;
        }
        if !*released && trace.can_release() {
            input
                .take()
                .ok_or("fixture input already closed")?
                .write_all(b"R")
                .map_err(|e| e.to_string())?;
            *released = true;
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(None)
}

pub fn observe(policy: &LinuxPolicy<'_>) -> Value {
    let args = [
        policy.target_args[0].clone(),
        policy.target_args[1].clone(),
        "cancel".into(),
    ];
    let policy = LinuxPolicy {
        target_args: &args,
        ..*policy
    };
    let plan = policy.compose().unwrap();
    std::fs::write(policy.profile, plan.landlock).unwrap();
    let mut child = Command::new("/usr/bin/bwrap")
        .args(plan.arguments)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take();
    let (tx, rx) = mpsc::channel();
    let stdout = reader(child.stdout.take().unwrap(), tx.clone(), true);
    let stderr = reader(child.stderr.take().unwrap(), tx.clone(), false);
    drop(tx);
    let mut trace = Trace::default();
    let mut released = false;
    let outcome = drive(&mut child, &mut input, &rx, &mut trace, &mut released);
    // This single owner is the only waiter. An unreaped bwrap child retains its
    // identity; terminating its private PID namespace settles the owned fixture
    // before any assertion, including the intentionally missing-cancel control.
    let status = child.try_wait().unwrap().unwrap_or_else(|| {
        let _ = child.kill();
        child.wait().unwrap()
    });
    drop(input);
    stdout.join().unwrap();
    stderr.join().unwrap();
    let mut errors = Vec::new();
    for message in rx {
        if let Err(error) = trace.record(message) {
            errors.push(error);
        }
    }
    assert!(
        outcome.is_ok() && errors.is_empty(),
        "fixture protocol failed: {outcome:?} {errors:?}: {}",
        trace.report()
    );
    let ready = trace
        .ready
        .as_ref()
        .expect("missing cancellation readiness");
    assert!(
        ready["client_syn_sent"] == true && ready["cancelled"] == true,
        "missing actual blocked-connect interruption: {}",
        trace.report()
    );
    assert!(
        released,
        "cancelled socket worker was not reaped: {}",
        trace.report()
    );
    assert!(
        status.success(),
        "cancel fixture failed: {status}: {}",
        trace.report()
    );
    assert_eq!(
        trace.finished.as_ref().unwrap()["tcp_after_cancel"],
        true,
        "broker did not serve after cancellation"
    );
    trace.report()
}
