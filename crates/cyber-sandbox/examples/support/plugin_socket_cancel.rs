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
    Eof(bool),
}
#[derive(Clone, Copy)]
enum Case {
    Cancellation,
    Mutation,
    Escape,
    Saturation,
}
impl Case {
    fn argument(self) -> &'static str {
        match self {
            Self::Cancellation => "cancel",
            Self::Mutation => "mutation",
            Self::Escape => "escape",
            Self::Saturation => "saturation",
        }
    }
    fn handshakes(self) -> u8 {
        match self {
            Self::Escape => 0,
            Self::Saturation => 2,
            _ => 1,
        }
    }
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
                Ok(0) => {
                    let _ = tx.send(Message::Eof(stdout));
                    return;
                }
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
    cancelled_workers: BTreeSet<u32>,
    pinned: Option<u32>,
    started: BTreeSet<u32>,
    reaped: BTreeSet<u32>,
    audit: Vec<String>,
    stdout_closed: bool,
    closed_before_cleanup: Option<bool>,
    owner_exited_before_cleanup: Option<bool>,
    pressure: Option<Value>,
    pressure_cancelled: Option<Value>,
}
impl Trace {
    fn record(&mut self, message: Message) -> Result<(), String> {
        match message {
            Message::Stdout(line) => self.output(&line),
            Message::Stderr(line) => self.event(&line),
            Message::Fault(error) => Err(error),
            Message::Eof(stdout) => {
                self.stdout_closed |= stdout;
                Ok(())
            }
        }
    }
    fn output(&mut self, line: &str) -> Result<(), String> {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let slot = match value["phase"].as_str() {
            Some("cancel-ready" | "mutation-ready" | "escape-ready" | "saturation-ready") => {
                &mut self.ready
            }
            Some("cancel-finished" | "mutation-finished" | "saturation-finished") => {
                &mut self.finished
            }
            Some("saturation-filled") => &mut self.pressure,
            Some("saturation-cancelled") => &mut self.pressure_cancelled,
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
                self.cancelled_workers.insert(pid);
            }
            "BPIN" => {
                self.pinned = Some(pid);
            }
            "BREAP" => {
                self.reaped.insert(pid);
            }
            _ => return Err(format!("unknown audit event: {line}")),
        }
        Ok(())
    }
    fn can_release(&self, case: Case, handshake: u8) -> bool {
        if matches!(case, Case::Saturation) {
            return self.release_pressure(handshake);
        }
        if matches!(case, Case::Escape) {
            return false;
        }
        if matches!(case, Case::Mutation) {
            let Some(tid) = self.pinned else {
                return false;
            };
            return self
                .ready
                .as_ref()
                .is_some_and(|v| v["request_tid"].as_u64() == Some(u64::from(tid)));
        }
        let Some(pid) = self.cancelled else {
            return false;
        };
        self.ready
            .as_ref()
            .is_some_and(|v| v["client_syn_sent"] == true && v["cancelled"] == true)
            && self.started.contains(&pid)
            && self.reaped.contains(&pid)
    }
    fn release_pressure(&self, handshake: u8) -> bool {
        match handshake {
            0 => {
                self.ready
                    .as_ref()
                    .is_some_and(|v| v["setup_complete"] == true)
                    && self.started.len() == 3
                    && self.started.is_subset(&self.reaped)
            }
            1 => {
                self.pressure_cancelled
                    .as_ref()
                    .is_some_and(|v| v["backpressure"] == true && v["interrupted_count"] == 16)
                    && self.started.len() == 19
                    && self.cancelled_workers.len() == 16
                    && self.cancelled_workers.is_subset(&self.reaped)
            }
            _ => false,
        }
    }
    fn report(&self) -> Value {
        json!({"ready":self.ready,"finished":self.finished,"cancelled_pid":self.cancelled,
            "pinned_tid":self.pinned,"started":self.started,"reaped":self.reaped,"audit":self.audit,
            "stdout_closed_before_cleanup":self.closed_before_cleanup,"owner_exited_before_cleanup":self.owner_exited_before_cleanup,
            "pressure":self.pressure,"pressure_cancelled":self.pressure_cancelled,"cancelled_workers":self.cancelled_workers})
    }
    fn verify(&self, case: Case, released: bool, status: ExitStatus) {
        match case {
            Case::Cancellation => self.verify_cancel(released, status),
            Case::Mutation => self.verify_mutation(released, status),
            Case::Escape => self.verify_escape(status),
            Case::Saturation => self.verify_pressure(released, status),
        }
    }
    fn verify_cancel(&self, released: bool, status: ExitStatus) {
        let ready = self.ready.as_ref().expect("missing cancellation readiness");
        assert!(
            ready["client_syn_sent"] == true && ready["cancelled"] == true,
            "missing actual blocked-connect interruption: {}",
            self.report()
        );
        assert!(
            released,
            "cancelled socket worker was not reaped: {}",
            self.report()
        );
        assert!(
            status.success(),
            "cancel fixture failed: {status}: {}",
            self.report()
        );
        assert_eq!(
            self.finished.as_ref().unwrap()["tcp_after_cancel"],
            true,
            "broker did not serve after cancellation"
        );
    }
    fn verify_mutation(&self, released: bool, status: ExitStatus) {
        assert!(
            released,
            "missing post-pin mutation handshake: {}",
            self.report()
        );
        let finished = self.finished.as_ref().expect("missing mutation result");
        let correct = [
            "bound_original",
            "fd_replaced",
            "address_changed",
            "result_ok",
        ]
        .iter()
        .all(|key| finished[key] == true);
        assert!(
            status.success() && correct,
            "socket/address mutation changed the admitted operation: {}",
            self.report()
        );
    }
    fn verify_escape(&self, status: ExitStatus) {
        assert_eq!(
            self.ready.as_ref().expect("missing descendant receipt")["controlled_stdin"],
            true,
            "fixture stdin was not the controlled blocking pipe"
        );
        assert_eq!(
            self.ready.as_ref().expect("missing descendant receipt")["escaped_session"],
            true,
            "fixture did not escape its original process group"
        );
        assert!(
            status.success() && self.owner_exited_before_cleanup == Some(true),
            "namespace owner did not finish after plugin leader exit: {}",
            self.report()
        );
        assert!(
            self.closed_before_cleanup == Some(true),
            "escaped descendant retained stdout after namespace owner exited: {}",
            self.report()
        );
    }
    fn verify_pressure(&self, released: bool, status: ExitStatus) {
        assert_eq!(
            self.ready.as_ref().expect("missing pressure setup")["setup_complete"],
            true
        );
        assert_eq!(
            self.pressure
                .as_ref()
                .expect("missing actual saturation wave")["blocked_count"],
            16,
            "fixture did not establish 16 real blocked connects"
        );
        assert!(
            self.pressure_cancelled
                .as_ref()
                .is_some_and(|v| v["backpressure"] == true),
            "socket-worker cap did not backpressure: {}",
            self.report()
        );
        assert!(
            released,
            "saturation cancellation wave was not reaped: {}",
            self.report()
        );
        assert!(
            status.success()
                && self
                    .finished
                    .as_ref()
                    .is_some_and(|v| v["tcp_after_saturation"] == true),
            "socket-worker capacity was not recovered: {}",
            self.report()
        );
    }
}

fn settled(
    child: &mut Child,
    trace: &Trace,
    case: Case,
    exit: &mut Option<(ExitStatus, Instant)>,
) -> Result<Option<ExitStatus>, String> {
    if exit.is_none()
        && let Some(status) = child.try_wait().map_err(|e| e.to_string())?
    {
        *exit = Some((status, Instant::now()));
    }
    let Some((status, at)) = exit.as_ref() else {
        return Ok(None);
    };
    let complete = match case {
        Case::Escape => trace.stdout_closed || at.elapsed() >= Duration::from_secs(10),
        _ => true,
    };
    Ok(complete.then_some(*status))
}

fn drive(
    child: &mut Child,
    input: &mut Option<ChildStdin>,
    rx: &Receiver<Message>,
    trace: &mut Trace,
    handshakes: &mut u8,
    case: Case,
) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut exit = None;
    while Instant::now() < deadline {
        if let Ok(message) = rx.recv_timeout(Duration::from_millis(30)) {
            trace.record(message)?;
        }
        if *handshakes < case.handshakes() && trace.can_release(case, *handshakes) {
            input
                .as_mut()
                .ok_or("fixture input already closed")?
                .write_all(b"R")
                .map_err(|e| e.to_string())?;
            *handshakes += 1;
            if *handshakes == case.handshakes() {
                drop(input.take());
            }
        }
        if let Some(status) = settled(child, trace, case, &mut exit)? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(None)
}

pub fn observe(policy: &LinuxPolicy<'_>) -> Value {
    observe_case(policy, Case::Cancellation, true)
}
pub fn observe_mutation(policy: &LinuxPolicy<'_>) -> Value {
    observe_case(policy, Case::Mutation, true)
}
pub fn observe_escape(policy: &LinuxPolicy<'_>, private_pid: bool) -> Value {
    observe_case(policy, Case::Escape, private_pid)
}
pub fn observe_saturation(policy: &LinuxPolicy<'_>) -> Value {
    observe_case(policy, Case::Saturation, true)
}
fn observe_case(policy: &LinuxPolicy<'_>, case: Case, private_pid: bool) -> Value {
    let args = [
        policy.target_args[0].clone(),
        policy.target_args[1].clone(),
        case.argument().into(),
    ];
    let policy = LinuxPolicy {
        target_args: &args,
        shell: matches!(case, Case::Escape) || policy.shell,
        ..*policy
    };
    let mut plan = policy.compose().unwrap();
    if !private_pid {
        assert_eq!(
            plan.arguments
                .iter()
                .filter(|arg| *arg == "--unshare-pid")
                .count(),
            1
        );
        plan.arguments.retain(|arg| arg != "--unshare-pid");
        let proc = plan
            .arguments
            .iter()
            .position(|arg| arg == "--proc")
            .unwrap();
        assert_eq!(plan.arguments[proc + 1], "/proc");
        plan.arguments.splice(
            proc..proc + 2,
            ["--ro-bind".into(), "/proc".into(), "/proc".into()],
        );
    }
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
    let mut handshakes = 0;
    let outcome = drive(
        &mut child,
        &mut input,
        &rx,
        &mut trace,
        &mut handshakes,
        case,
    );
    if matches!(case, Case::Escape) {
        trace.closed_before_cleanup = Some(trace.stdout_closed);
        trace.owner_exited_before_cleanup =
            Some(outcome.as_ref().is_ok_and(|status| status.is_some()));
    }
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
    trace.verify(case, handshakes == case.handshakes(), status);
    trace.report()
}
