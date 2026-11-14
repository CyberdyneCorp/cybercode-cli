//! `cyber exec --include-hook-events` through the real binary (`exec-mode` → Hook events
//! in exec): `hook_event` lines for the run's hooks, in order before what they gate, with
//! no hook IO, against a scripted OpenAI-compatible provider.
#![cfg(unix)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// A value the hooks print and see in their environment; it must never reach stdout.
const SECRET: &str = "hook-secret-7f3a9c1e5b";

/// Answers each Turn request with the next scripted reply: `text:<answer>` or a tool
/// call `<tool> <json arguments>`. Title requests (no tools) get a title.
struct Provider {
    url: String,
    done: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn start(script: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let script: Arc<Mutex<VecDeque<String>>> =
            Arc::new(Mutex::new(script.iter().map(|s| (*s).to_owned()).collect()));
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let server = thread::spawn(move || {
            let mut turn = 0;
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, &script, &mut turn),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            done,
            server: Some(server),
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn answer(mut stream: TcpStream, script: &Mutex<VecDeque<String>>, turn: &mut u32) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Some(body) = read_body(&mut stream) else {
        return;
    };
    let chunk = if body["tools"].is_array() {
        *turn += 1;
        let next = script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "text:extra".into());
        match next.strip_prefix("text:") {
            Some(text) => {
                json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]})
            }
            None => {
                let (tool, arguments) = next.split_once(' ').unwrap();
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call_{turn}"),"type":"function","function":{"name":tool,"arguments":arguments}}]},"finish_reason":"tool_calls"}]})
            }
        }
    } else {
        json!({"choices":[{"index":0,"delta":{"content":"Title"},"finish_reason":"stop"}]})
    };
    let usage = json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":1}});
    let body = format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn read_body(stream: &mut TcpStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).ok();
            }
        }
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
}

/// A command hook `id` that leaks [`SECRET`] (from its environment) to stderr and into
/// its decision reason, then prints `decision`, or exits 2 to block when it is `None`.
fn leaky(id: &str, decision: Option<Value>) -> Value {
    let end = match decision {
        Some(decision) => format!("printf '%s' '{decision}'"),
        None => "echo 'bash is not allowed here' >&2; exit 2".into(),
    };
    json!({"type":"command","id":id,"command":format!(
        "cat >/dev/null; echo \"$HOOK_SECRET\" >&2; {end}"
    )})
}

/// A fresh root whose config routes `corp/main` to `provider`, records hook IO (so the
/// receipts hold the secret) and configures: a SessionStart hook, a PreToolUse hook that
/// allows `read` and one that blocks `bash`.
fn root(provider: &Provider) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(root.join("repo/a.txt"), "file body").unwrap();
    let allow = json!({"decision":"allow","reason":format!("reason {SECRET}")});
    let config = json!({
        "permissions": "allow",
        "telemetry": {"log_hook_io": true},
        "providers": {"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}},
        "hooks": {
            "SessionStart": [{"hooks":[leaky("start", Some(json!({})))]}],
            "PreToolUse": [
                {"matcher":"read","hooks":[leaky("allow-read", Some(allow))]},
                {"matcher":"bash","hooks":[leaky("block-bash", None)]}
            ]
        }
    });
    std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();
    (dir, root)
}

/// `cyber exec` with a private in-process server.
fn cyber(root: &Path) -> Command {
    let mut command = base(root);
    command.args(["exec", "--embedded", "--model", "corp/main"]);
    command
}

/// The isolated `cyber` command of `root`, with no arguments yet.
fn base(root: &Path) -> Command {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec");
    assert!(
        helper.is_file(),
        "build the helper first: cargo build -p cyber-sandbox --bin cyber-sandbox-exec"
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["--cwd", root.join("repo").to_str().unwrap()])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("CYBER_SANDBOX_HELPER", helper)
        .env("HOOK_SECRET", SECRET)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs `command` with `stdin`, killed after 60 s so a hung exec fails.
fn run(mut command: Command, stdin: &str) -> std::process::Output {
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).unwrap();
    drop(input);
    let id = child.id();
    let (exited, deadline) = mpsc::channel::<()>();
    let killer = thread::spawn(move || {
        if deadline.recv_timeout(Duration::from_secs(60)) == Err(mpsc::RecvTimeoutError::Timeout) {
            let _ = Command::new("kill").args(["-9", &id.to_string()]).status();
        }
    });
    let output = child.wait_with_output().unwrap();
    let _ = exited.send(());
    killer.join().unwrap();
    output
}

/// Every stdout line, parsed.
fn lines(output: &std::process::Output) -> Vec<Value> {
    BufReader::new(output.stdout.as_slice())
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect()
}

fn detail(output: &std::process::Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// The index of the first line matching `test`.
fn find(lines: &[Value], test: impl Fn(&Value) -> bool, detail: &str) -> usize {
    lines
        .iter()
        .position(test)
        .unwrap_or_else(|| panic!("no such line\n{detail}"))
}

const TOOLS: &[&str] = &[
    r#"read {"path":"a.txt"}"#,
    r#"bash {"command":"echo hi"}"#,
    "text:done",
];

/// The line of guard `hook_id` for `tool`, which must come before that call's
/// `tool_result`; returns its index.
fn guard_line(lines: &[Value], hook_id: &str, tool: &str, detail: &str) -> usize {
    let at = find(lines, |l| l["hook_id"] == hook_id, detail);
    let line = &lines[at];
    assert_eq!(
        (&line["event"], &line["tool_name"]),
        (&json!("PreToolUse"), &json!(tool)),
        "{detail}"
    );
    let call = line["call_id"].clone();
    let result = find(
        lines,
        |l| l["type"] == "tool_result" && l["call_id"] == call,
        detail,
    );
    assert!(
        at < result,
        "the hook line precedes its call's result\n{detail}"
    );
    at
}

/// Every `hook_event` line belongs to the run's Session and has a settled receipt.
fn assert_receipt_lines(lines: &[Value], count: usize, detail: &str) {
    let hooks: Vec<&Value> = lines.iter().filter(|l| l["type"] == "hook_event").collect();
    assert_eq!(hooks.len(), count, "one line per execution\n{detail}");
    for hook in hooks {
        assert_eq!(hook["session_id"], lines[0]["session_id"], "{detail}");
        assert!(hook["receipt_id"].as_str().unwrap().starts_with("hke_"));
        assert!(hook["duration_ms"].is_u64(), "{detail}");
        assert_eq!(hook["scope"], "global", "{detail}");
    }
}

#[test]
fn stream_json_reports_allowing_and_blocking_hooks_before_what_they_gate() {
    let provider = Provider::start(TOOLS);
    let (_dir, root) = root(&provider);
    let mut command = cyber(&root);
    command.args(["--format", "stream-json", "--include-hook-events", "go"]);
    let output = run(command, "");
    let detail = detail(&output);
    assert!(output.status.success(), "{detail}");
    let lines = lines(&output);
    assert_receipt_lines(&lines, 3, &detail);
    let start = find(&lines, |l| l["event"] == "SessionStart", &detail);
    assert_eq!(lines[start]["outcome"], "ok", "{detail}");

    let allowed = guard_line(&lines, "allow-read", "read", &detail);
    assert_eq!(
        (&lines[allowed]["outcome"], &lines[allowed]["decision"]),
        (&json!("ok"), &json!("allow")),
        "{detail}"
    );
    let blocked = guard_line(&lines, "block-bash", "bash", &detail);
    assert_eq!(lines[blocked]["outcome"], "blocked", "{detail}");
    let end = find(&lines, |l| l["type"] == "result", &detail);
    assert!(start < end && blocked < end, "{detail}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(SECRET), "hook IO leaked\n{detail}");
}

/// Stops the background server a test started.
struct Service(PathBuf);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = base(&self.0).args(["service", "stop"]).output();
    }
}

/// `cyber exec` against the background server, which keeps Sessions between runs.
fn shared(root: &Path) -> Command {
    let mut command = base(root);
    command.args(["exec", "--model", "corp/main"]);
    command
}

#[test]
fn a_reused_session_reports_only_this_runs_hooks() {
    let script = [TOOLS, &[r#"read {"path":"a.txt"}"#, "text:again"]].concat();
    let provider = Provider::start(&script);
    let (_dir, root) = root(&provider);
    let _service = Service(root.clone());
    let mut command = shared(&root);
    command.args(["--format", "stream-json", "--include-hook-events", "go"]);
    let first = run(command, "");
    assert!(first.status.success(), "{}", detail(&first));
    let earlier: Vec<Value> = lines(&first)
        .into_iter()
        .filter(|l| l["type"] == "hook_event")
        .map(|l| l["receipt_id"].clone())
        .collect();
    let session = lines(&first)[0]["session_id"].as_str().unwrap().to_owned();
    assert_eq!(earlier.len(), 3, "{}", detail(&first));
    let mut command = shared(&root);
    command.args(["--format", "stream-json", "--include-hook-events"]);
    command.args(["--session", &session, "again"]);
    let second = run(command, "");
    let detail = detail(&second);
    assert!(second.status.success(), "{detail}");
    let hooks: Vec<Value> = lines(&second)
        .into_iter()
        .filter(|l| l["type"] == "hook_event")
        .collect();
    // Only the new `read` guard: no line of the first run's receipts.
    assert!(
        hooks.iter().all(|l| !earlier.contains(&l["receipt_id"])),
        "{detail}"
    );
    assert!(
        hooks.iter().any(|l| l["hook_id"] == "allow-read"),
        "{detail}"
    );
}

#[test]
fn without_the_flag_no_hook_lines_appear() {
    let provider = Provider::start(TOOLS);
    let (_dir, root) = root(&provider);
    let mut command = cyber(&root);
    command.args(["--format", "stream-json", "go"]);
    let output = run(command, "");
    let detail = detail(&output);
    assert!(output.status.success(), "{detail}");
    let lines = lines(&output);
    assert!(lines.iter().all(|l| l["type"] != "hook_event"), "{detail}");
    // The hooks did run: the blocked call's result says so.
    find(&lines, |l| l["type"] == "tool_result", &detail);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
}

#[test]
fn the_flag_needs_stream_json() {
    let provider = Provider::start(&[]);
    let (_dir, root) = root(&provider);
    for format in ["json", "text"] {
        let mut command = cyber(&root);
        command.args(["--format", format, "--include-hook-events", "go"]);
        let output = run(command, "");
        let detail = detail(&output);
        assert_eq!(output.status.code(), Some(2), "{detail}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("--include-hook-events needs --format stream-json"),
            "{detail}"
        );
    }
}

#[test]
fn driven_runs_report_hook_lines_before_each_result() {
    let provider = Provider::start(&[r#"read {"path":"a.txt"}"#, "text:first", "text:second"]);
    let (_dir, root) = root(&provider);
    let mut command = cyber(&root);
    command.args([
        "--format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--include-hook-events",
    ]);
    let stdin = [
        r#"{"type":"user","text":"one","id":"a"}"#,
        r#"{"type":"end"}"#,
    ]
    .join("\n");
    let output = run(command, &format!("{stdin}\n"));
    let detail = detail(&output);
    assert_eq!(output.status.code(), Some(0), "{detail}");
    let lines = lines(&output);
    assert_eq!(lines[0]["schema_version"], 2, "{detail}");
    let start = find(&lines, |l| l["event"] == "SessionStart", &detail);
    let read = find(&lines, |l| l["hook_id"] == "allow-read", &detail);
    let result = find(&lines, |l| l["type"] == "result", &detail);
    assert!(start < result && read < result, "{detail}");
    assert_eq!(
        lines.iter().filter(|l| l["type"] == "hook_event").count(),
        2,
        "{detail}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
}
