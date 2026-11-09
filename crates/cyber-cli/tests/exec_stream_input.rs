//! `cyber exec --input-format stream-json` through the real binary (`exec-mode` →
//! Streaming input and partial output, Bounded interactive JSONL driver), driven over
//! stdin against a scripted OpenAI-compatible provider.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// Answers each Turn request with the next scripted reply: `text:<answer>` or a tool
/// call `<tool> <json arguments>`. Title requests (no tools) get a title.
struct Provider {
    url: String,
    turns: Arc<Mutex<Vec<Value>>>,
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
        let turns = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let (seen, stop) = (turns.clone(), done.clone());
        let server = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, &seen, &script),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            turns,
            done,
            server: Some(server),
        }
    }

    fn turns(&self) -> Vec<Value> {
        self.turns.lock().unwrap().clone()
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

fn answer(mut stream: TcpStream, turns: &Mutex<Vec<Value>>, script: &Mutex<VecDeque<String>>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Some(body) = read_body(&mut stream) else {
        return;
    };
    let chunk = if body["tools"].is_array() {
        let n = {
            let mut turns = turns.lock().unwrap();
            turns.push(body);
            turns.len()
        };
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
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call_{n}"),"type":"function","function":{"name":tool,"arguments":arguments}}]},"finish_reason":"tool_calls"}]})
            }
        }
    } else {
        json!({"choices":[{"index":0,"delta":{"content":"Driven"},"finish_reason":"stop"}]})
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

/// A running `cyber exec --input-format stream-json --format stream-json`.
struct Driven {
    _dir: tempfile::TempDir,
    _root: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<Value>,
}

impl Driven {
    fn start(provider: &Provider, flags: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        let config = json!({"providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}});
        std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let helper =
            PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec");
        let mut child = Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["--cwd", root.join("repo").to_str().unwrap()])
            .args([
                "exec",
                "--embedded",
                "--model",
                "corp/main",
                "--format",
                "stream-json",
            ])
            .args(["--input-format", "stream-json"])
            .args(flags)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("CYBER_HOME", root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MODELS_PATH", root.join("models.json"))
            .env("CYBER_SANDBOX_HELPER", helper)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    let _ = send.send(value);
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            _dir: dir,
            _root: root,
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn close(&mut self) {
        self.stdin.take();
    }

    /// The next line of `kind`, collecting the ones before it.
    fn until(&self, kind: &str, seen: &mut Vec<Value>) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("no {kind} line; saw {seen:#?}"));
            seen.push(line.clone());
            if line["type"] == kind {
                return line;
            }
        }
    }

    fn exit(mut self) -> Option<i32> {
        self.close();
        for _ in 0..600 {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        panic!("cyber exec did not exit");
    }
}

#[test]
fn unavailable_snapshots_do_not_fail_a_successful_jsonl_input() {
    let provider = Provider::start(&["text:completed without snapshots"]);
    // Driven creates an invalid Git marker: capture warns, but inference succeeds.
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"warning"}"#);
    let result = run.until("result", &mut seen);
    assert_eq!(result["id"], "warning");
    assert_eq!(result["result"], "completed without snapshots");
    assert_eq!(result["error"], Value::Null, "{seen:#?}");
    assert_eq!(result["exit_code"], 0, "{seen:#?}");
    run.send(r#"{"type":"end"}"#);
    assert_eq!(run.until("end", &mut seen)["exit_code"], 0);
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn last_message_publication_failure_fails_the_run_without_losing_the_answer() {
    let provider = Provider::start(&["text:completed answer"]);
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().display().to_string();
    let mut run = Driven::start(&provider, &["--output-last-message", &path]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one"}"#);
    run.send(r#"{"type":"end"}"#);
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], "completed answer");
    assert_eq!(result["exit_code"], 0);
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 1);
    assert!(
        end["error"]
            .as_str()
            .unwrap()
            .contains("--output-last-message")
    );
    assert_eq!(run.exit(), Some(1));
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
}

#[test]
fn two_prompts_give_two_results_in_order_then_end() {
    let provider = Provider::start(&["text:first answer", "text:second answer"]);
    let last = tempfile::NamedTempFile::new().unwrap();
    let last_path = last.path().display().to_string();
    let mut run = Driven::start(&provider, &["--output-last-message", &last_path]);
    let mut seen = Vec::new();
    let init = run.until("system", &mut seen);
    assert_eq!(init["schema_version"], 2);
    run.send(r#"{"type":"user","text":"one","id":"a"}"#);
    let first = run.until("result", &mut seen);
    assert_eq!(
        (first["id"].as_str(), first["result"].as_str()),
        (Some("a"), Some("first answer"))
    );
    assert_eq!(first["exit_code"], 0, "{seen:#?}");
    run.send(r#"{"type":"user","text":"two","id":"b"}"#);
    run.send(r#"{"type":"end"}"#);
    let second = run.until("result", &mut seen);
    assert_eq!(
        (second["id"].as_str(), second["result"].as_str()),
        (Some("b"), Some("second answer"))
    );
    let end = run.until("end", &mut seen);
    assert_eq!(
        (end["exit_code"].as_i64(), end["inputs"].as_i64()),
        (Some(0), Some(2))
    );
    assert_eq!(run.exit(), Some(0));
    assert_eq!(
        std::fs::read_to_string(last.path()).unwrap(),
        "second answer"
    );
    let admitted: Vec<&Value> = seen.iter().filter(|l| l["type"] == "input").collect();
    assert_eq!(admitted.len(), 2);
}

#[test]
fn a_permission_is_answered_over_stdin_and_others_are_refused() {
    let provider = Provider::start(&[r#"bash {"command":"echo driven"}"#, "text:ran it"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"run it"}"#);
    let asked = run.until("permission_request", &mut seen);
    let request = asked["id"].as_str().unwrap().to_owned();
    // A reply to a request this run does not have is an error, and the run goes on.
    run.send(r#"{"type":"permission_reply","request_id":"per_unknown","reply":"once"}"#);
    let refused = run.until("error", &mut seen);
    assert_eq!(refused["code"], "protocol", "{refused}");
    run.send(&json!({"type":"permission_reply","request_id":request,"reply":"once"}).to_string());
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], "ran it");
    assert!(
        !seen.iter().any(|l| l["type"] == "permission_denied"),
        "{seen:#?}"
    );
    let output = provider.turns()[1].to_string();
    assert!(output.contains("driven"), "the command ran: {output}");
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn eof_while_a_request_waits_declines_it_instead_of_hanging() {
    let provider = Provider::start(&[r#"bash {"command":"echo never"}"#, "text:declined"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"run it"}"#);
    run.until("permission_request", &mut seen);
    run.close();
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], "declined");
    let second = provider.turns()[1].to_string();
    assert!(second.contains("no interactive approver"), "{second}");
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn a_malformed_line_is_a_protocol_error_exit_2() {
    let provider = Provider::start(&[]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send("not json at all");
    let error = run.until("error", &mut seen);
    assert_eq!(error["code"], "protocol");
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 2);
    assert_eq!(run.exit(), Some(2));
    assert!(provider.turns().is_empty(), "nothing was sent to the model");
}

#[test]
fn unsupported_combinations_exit_2_before_any_session() {
    let provider = Provider::start(&[]);
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["--cwd", dir.path().to_str().unwrap(), "exec", "--embedded"])
            .args(args)
            .env_clear()
            .env("HOME", dir.path())
            .env("CYBER_HOME", dir.path().join("cyber"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    for args in [
        &["--input-format", "stream-json", "hi"][..],
        &[
            "--input-format",
            "stream-json",
            "--format",
            "stream-json",
            "hi",
        ],
        &[
            "--input-format",
            "stream-json",
            "--format",
            "stream-json",
            "--timeout",
            "5m",
        ],
        &["--include-partial-messages", "hi"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
    }
    assert!(provider.turns().is_empty());
}

#[test]
fn each_input_gets_its_structured_result() {
    let provider = Provider::start(&[
        r#"structured_output {"summary":"first"}"#,
        r#"structured_output {"summary":"second"}"#,
    ]);
    let schema = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        schema.path(),
        json!({"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]})
            .to_string(),
    )
    .unwrap();
    let path = schema.path().display().to_string();
    let mut run = Driven::start(&provider, &["--output-schema", &path]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"a"}"#);
    run.send(r#"{"type":"user","text":"two","id":"b"}"#);
    run.send(r#"{"type":"end"}"#);
    let first = run.until("result", &mut seen);
    let second = run.until("result", &mut seen);
    assert_eq!(
        (first["id"].as_str(), &first["result"]),
        (Some("a"), &json!({"summary":"first"}))
    );
    assert_eq!(
        (second["id"].as_str(), &second["result"]),
        (Some("b"), &json!({"summary":"second"}))
    );
    assert_eq!(run.exit(), Some(0));
}
