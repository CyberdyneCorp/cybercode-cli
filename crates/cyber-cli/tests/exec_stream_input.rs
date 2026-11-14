//! `cyber exec --input-format stream-json` through the real binary (`exec-mode` →
//! Streaming input and partial output, Bounded interactive JSONL driver), driven over
//! stdin against a scripted OpenAI-compatible provider.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Answers each Turn request with the next scripted reply: `text:<answer>`, a tool
/// call `<tool> <json arguments>` or `fail:<body>`, an HTTP 400 with that body. Title requests (no tools) get a title.
struct Provider {
    url: String,
    turns: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
    gate: Arc<Gate>,
}

#[derive(Default)]
struct Gate {
    reached: AtomicBool,
    released: AtomicBool,
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
        let gate = Arc::new(Gate::default());
        let held = gate.clone();
        let (seen, stop) = (turns.clone(), done.clone());
        let server = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, &seen, &script, &held, &stop),
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
            gate,
        }
    }

    fn turns(&self) -> Vec<Value> {
        self.turns.lock().unwrap().clone()
    }

    fn wait_for_gate(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.gate.reached.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "provider request did not reach its gate"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn release(&self) {
        self.gate.released.store(true, Ordering::SeqCst);
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

fn answer(
    mut stream: TcpStream,
    turns: &Mutex<Vec<Value>>,
    script: &Mutex<VecDeque<String>>,
    gate: &Gate,
    done: &AtomicBool,
) {
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
        let next = match next.strip_prefix("gate:") {
            Some(text) => {
                gate.reached.store(true, Ordering::SeqCst);
                while !gate.released.load(Ordering::SeqCst) && !done.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(5));
                }
                text
            }
            None => next.as_str(),
        };
        if let Some(body) = next.strip_prefix("fail:") {
            let _ = write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            return;
        }
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
        Self::start_reading(provider, flags, Arc::new(AtomicBool::new(true)), None)
    }

    /// Like `start`, but working in another driver's repository, so both share one
    /// Location.
    fn start_beside(provider: &Provider, flags: &[&str], other: &Driven) -> Self {
        let repo = other._root.join("repo");
        Self::start_reading(provider, flags, Arc::new(AtomicBool::new(true)), Some(repo))
    }

    /// Like `start`, but stdout is not read until `reading` becomes true, so the driver's
    /// writes block once the pipe fills.
    fn start_reading(
        provider: &Provider,
        flags: &[&str],
        reading: Arc<AtomicBool>,
        repo: Option<PathBuf>,
    ) -> Self {
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
            .arg("--cwd")
            .arg(repo.unwrap_or_else(|| root.join("repo")))
            .arg("exec")
            .args((!flags.contains(&"--attach")).then_some("--embedded"))
            .args(["--model", "corp/main", "--format", "stream-json"])
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
            .stderr(std::fs::File::create(root.join("exec.stderr")).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        thread::spawn(move || {
            while !reading.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
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
                .unwrap_or_else(|_| {
                    let stderr = std::fs::read_to_string(self._root.join("exec.stderr"));
                    panic!("no {kind} line; saw {seen:#?}\nstderr: {stderr:?}")
                });
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
fn steering_a_structured_input_reports_its_superseded_error_and_the_new_inputs_value() {
    let provider = Provider::start(&[
        "gate:structured_output {\"wrong\":1}",
        "structured_output {\"summary\":\"latest\"}",
    ]);
    let mut schema = tempfile::NamedTempFile::new().unwrap();
    schema.write_all(br#"{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}"#).unwrap();
    let path = schema.path().display().to_string();
    let mut run = Driven::start(&provider, &["--output-schema", &path]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    let first_input = run.until("input", &mut seen);
    provider.wait_for_gate();
    run.send(r#"{"type":"user","text":"replace it","id":"next","delivery":"steer"}"#);
    let next_input = run.until("input", &mut seen);
    assert_eq!(
        next_input["status"], "admitted",
        "stdin stays live during inference"
    );
    run.send(r#"{"type":"end"}"#);
    provider.release();
    let first = run.until("result", &mut seen);
    assert_eq!(first["id"], "first");
    assert_eq!(first["message_id"], first_input["message_id"]);
    assert_eq!(first["result"], Value::Null);
    assert_eq!(
        first["error"],
        format!(
            "StructuredOutputError: superseded by input {}",
            next_input["message_id"].as_str().unwrap()
        )
    );
    assert_eq!(first["exit_code"], 1);
    let next = run.until("result", &mut seen);
    assert_eq!(next["id"], "next");
    assert_eq!(next["message_id"], next_input["message_id"]);
    assert_eq!(next["result"], json!({"summary":"latest"}));
    assert_eq!(next["error"], Value::Null);
    assert_eq!(next["exit_code"], 0);
    let end = run.until("end", &mut seen);
    assert_eq!(
        (end["inputs"].as_u64(), end["exit_code"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(run.exit(), Some(1));
    assert_eq!(provider.turns().len(), 2);
}

#[test]
fn unstructured_steering_keeps_each_completed_answer_correlated() {
    let provider = Provider::start(&["gate:text:first answer", "text:steered answer"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    run.until("input", &mut seen);
    provider.wait_for_gate();
    run.send(r#"{"type":"user","text":"refine","id":"next","delivery":"steer"}"#);
    assert_eq!(run.until("input", &mut seen)["status"], "admitted");
    run.send(r#"{"type":"end"}"#);
    provider.release();
    let first = run.until("result", &mut seen);
    assert_eq!(
        (first["id"].as_str(), first["result"].as_str()),
        (Some("first"), Some("first answer"))
    );
    let next = run.until("result", &mut seen);
    assert_eq!(
        (next["id"].as_str(), next["result"].as_str()),
        (Some("next"), Some("steered answer"))
    );
    assert_eq!(run.until("end", &mut seen)["exit_code"], 0);
    assert_eq!(run.exit(), Some(0));
    assert_eq!(provider.turns().len(), 2);
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
fn completed_inputs_release_slots_and_keep_run_accounting_and_correlation() {
    const COUNT: usize = 65;
    let provider = Provider::start(&["text:completed"; COUNT]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    let mut messages = std::collections::BTreeSet::new();
    for index in 0..COUNT {
        let id = format!("input-{index}");
        run.send(&json!({"type":"user","text":"next","id":id}).to_string());
        let input = run.until("input", &mut seen);
        assert_eq!(input["id"], id);
        assert_eq!(input["status"], "admitted");
        let result = run.until("result", &mut seen);
        assert_eq!(result["id"], id);
        assert_eq!(result["message_id"], input["message_id"]);
        assert!(messages.insert(result["message_id"].as_str().unwrap().to_owned()));
        assert_eq!(result["result"], "completed");
        assert_eq!(result["exit_code"], 0);
        seen.clear();
    }
    run.send(r#"{"type":"end"}"#);
    let end = run.until("end", &mut seen);
    assert_eq!(end["inputs"], COUNT);
    assert_eq!(end["exit_code"], 0);
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

/// A secret-shaped value in a provider error body or in a refused stdin line never
/// reaches the `error`, `result` or `end` lines literally.
#[test]
fn jsonl_errors_do_not_echo_secrets() {
    const SECRET: &str = "wJalrXUtnFEMI0K7MDENGbPxRfiCYzzEXAMPLEKEY";
    const AWS: &str = "AKIAZZZZEXAMPLE7QXYZ";
    let body = json!({"error":{"message":format!("rejected request: TOKEN={SECRET} key {AWS}"),"type":"invalid_request_error"}});
    let script = format!("fail:{body}");
    let provider = Provider::start(&[&script]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"one","text":"go"}"#);
    let result = run.until("result", &mut seen);
    assert_eq!(result["id"], "one");
    assert_ne!(result["exit_code"], 0, "{seen:#?}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("[REDACTED:"), "{seen:#?}");
    run.send(&format!(r#"{{"type":"api_key={SECRET}"}}"#));
    let refused = run.until("error", &mut seen);
    let refused = seen
        .iter()
        .find(|line| line["code"] == "protocol")
        .unwrap_or(&refused)
        .clone();
    assert!(
        refused["message"].as_str().unwrap().contains("[REDACTED:"),
        "{refused}"
    );
    let end = run.until("end", &mut seen);
    assert!(
        end["error"].as_str().unwrap().contains("[REDACTED:"),
        "{end}"
    );
    assert_eq!(run.exit(), Some(2));
    let errors: Vec<&Value> = seen
        .iter()
        .filter(|line| ["error", "result", "end"].contains(&line["type"].as_str().unwrap_or("")))
        .collect();
    assert!(errors.len() >= 3, "{seen:#?}");
    for line in errors {
        let text = line.to_string();
        assert!(!text.contains(SECRET) && !text.contains(AWS), "{text}");
    }
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

#[cfg(unix)]
#[test]
fn background_input_retires_only_after_the_handback_answer_is_consumed() {
    let provider = Provider::start(&[
        r#"bash {"command":"while [ ! -f release-job ]; do sleep 0.02; done; echo delivered","background":true}"#,
        "text:initial answer",
        "gate:text:handback answer",
    ]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"background","text":"start a background job"}"#);
    let input = run.until("input", &mut seen);
    let request = run.until("permission_request", &mut seen);
    run.send(
        &json!({"type":"permission_reply","request_id":request["id"],"reply":"once"}).to_string(),
    );
    run.until("assistant_text", &mut seen);
    // A result must not be emitted while the process remains gated.
    let mut early = false;
    while let Ok(line) = run.lines.recv_timeout(Duration::from_millis(300)) {
        early |= line["type"] == "result";
        seen.push(line);
    }
    std::fs::write(run._root.join("repo/release-job"), "").unwrap();
    provider.wait_for_gate();
    while let Ok(line) = run.lines.try_recv() {
        early |= line["type"] == "result";
        seen.push(line);
    }
    run.send(r#"{"type":"end"}"#);
    provider.release();
    let result = run.until("result", &mut seen);
    assert_eq!(result["message_id"], input["message_id"]);
    assert_eq!(result["result"], "handback answer");
    assert_eq!(run.until("end", &mut seen)["exit_code"], 0, "{seen:#?}");
    assert_eq!(run.exit(), Some(0));
    assert!(
        !early,
        "input retired before its background work and handback settled: {seen:#?}"
    );
}

#[cfg(unix)]
#[test]
fn eof_declines_a_handback_question_and_settles_the_original_input() {
    let provider = Provider::start(&[
        r#"bash {"command":"while [ ! -f release-job ]; do sleep 0.02; done","background":true}"#,
        "text:initial answer",
        r#"question {"questions":[{"question":"Choose","header":"Choice","options":[{"label":"yes"},{"label":"no"}]}]}"#,
        "text:question declined",
    ]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"background","text":"start a background job"}"#);
    let input = run.until("input", &mut seen);
    let permission = run.until("permission_request", &mut seen);
    run.send(
        &json!({"type":"permission_reply","request_id":permission["id"],"reply":"once"})
            .to_string(),
    );
    run.until("assistant_text", &mut seen);
    std::fs::write(run._root.join("repo/release-job"), "").unwrap();
    let question = run.until("question", &mut seen);
    assert_eq!(question["input_message_ids"], json!([input["message_id"]]));
    run.close();
    let result = run.until("result", &mut seen);
    assert_eq!(result["message_id"], input["message_id"]);
    assert_eq!(result["result"], "question declined", "{seen:#?}");
    assert_eq!(result["exit_code"], 0, "{seen:#?}");
    assert_eq!(run.until("end", &mut seen)["exit_code"], 0);
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn a_permission_is_answered_over_stdin_and_others_are_refused() {
    let provider = Provider::start(&[r#"bash {"command":"echo driven"}"#, "text:ran it"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"run it"}"#);
    let input = run.until("input", &mut seen);
    let asked = run.until("permission_request", &mut seen);
    assert_eq!(asked["input_message_ids"], json!([input["message_id"]]));
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
fn a_second_reply_to_an_answered_request_is_a_protocol_error() {
    let provider = Provider::start(&[r#"bash {"command":"echo driven"}"#, "text:ran it"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"run it"}"#);
    let asked = run.until("permission_request", &mut seen);
    let reply = json!({"type":"permission_reply","request_id":asked["id"],"reply":"once"});
    run.send(&reply.to_string());
    run.send(&reply.to_string());
    let refused = run.until("error", &mut seen);
    assert_eq!(refused["code"], "protocol", "{refused}");
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(message.contains(asked["id"].as_str().unwrap()), "{refused}");
    run.send(r#"{"type":"end"}"#);
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 0, "{seen:#?}");
    let results: Vec<_> = seen.iter().filter(|l| l["type"] == "result").collect();
    assert_eq!(results.len(), 1, "{seen:#?}");
    assert_eq!(results[0]["result"], "ran it");
    assert_eq!(provider.turns().len(), 2, "the tool ran once");
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn a_question_reply_naming_a_permission_does_not_consume_it() {
    let provider = Provider::start(&[r#"bash {"command":"echo driven"}"#, "text:ran it"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"run it"}"#);
    let asked = run.until("permission_request", &mut seen);
    run.send(
        &json!({"type":"question_reply","request_id":asked["id"],"answers":[["yes"]]}).to_string(),
    );
    let refused = run.until("error", &mut seen);
    assert_eq!(refused["code"], "protocol", "{refused}");
    run.send(
        &json!({"type":"permission_reply","request_id":asked["id"],"reply":"once"}).to_string(),
    );
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], "ran it", "{seen:#?}");
    assert!(
        !seen.iter().any(|l| l["type"] == "permission_denied"),
        "{seen:#?}"
    );
    let turns = provider.turns();
    assert_eq!(turns.len(), 2, "the tool ran once: {turns:#?}");
    assert!(turns[1].to_string().contains("driven"), "{:#}", turns[1]);
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
fn a_question_is_bound_to_its_input_and_answered_over_stdin() {
    let provider = Provider::start(&[
        r#"question {"questions":[{"question":"Choose","header":"Choice","options":[{"label":"yes"},{"label":"no"}]}]}"#,
        "text:choice received",
    ]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"choose","id":"choice"}"#);
    let input = run.until("input", &mut seen);
    let asked = run.until("question", &mut seen);
    assert_eq!(asked["input_message_ids"], json!([input["message_id"]]));
    run.send(
        &json!({"type":"question_reply","request_id":asked["id"],"answers":[["yes"]]}).to_string(),
    );
    let result = run.until("result", &mut seen);
    assert_eq!(result["id"], "choice");
    assert_eq!(result["result"], "choice received");
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
        &["--include-hook-events", "hi"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
    }
    assert!(provider.turns().is_empty());
}

#[test]
fn include_hook_events_needs_stream_json_input() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["--cwd", dir.path().to_str().unwrap(), "exec", "--embedded"])
        .args(["--include-hook-events", "hi"])
        .env_clear()
        .env("HOME", dir.path())
        .env("CYBER_HOME", dir.path().join("cyber"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--include-hook-events"),
        "{out:?}"
    );
    assert!(!dir.path().join("cyber").join("embedded").exists());
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

#[test]
fn a_nullable_structured_object_is_a_terminal_answer() {
    let provider = Provider::start(&[r#"structured_output {"answer":null}"#]);
    let schema = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        schema.path(),
        r#"{"type":"object","properties":{"answer":{"type":"null"}},"required":["answer"]}"#,
    )
    .unwrap();
    let path = schema.path().display().to_string();
    let mut run = Driven::start(&provider, &["--output-schema", &path]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"null-answer","text":"return null"}"#);
    run.send(r#"{"type":"end"}"#);
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], json!({"answer":null}));
    assert_eq!(result["error"], Value::Null, "{seen:#?}");
    assert_eq!(result["exit_code"], 0, "{seen:#?}");
    assert_eq!(run.until("end", &mut seen)["exit_code"], 0);
    assert_eq!(run.exit(), Some(0));
}

#[test]
fn interrupt_during_inference_keeps_stdin_live_for_a_later_input() {
    let provider = Provider::start(&["gate:text:cancelled answer", "text:later answer"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"first","text":"start"}"#);
    let first = run.until("input", &mut seen);
    provider.wait_for_gate();
    run.send(r#"{"type":"interrupt"}"#);
    run.send(r#"{"type":"user","id":"later","text":"continue"}"#);
    let later = run.until("input", &mut seen);
    assert_eq!(later["id"], "later");
    assert_eq!(
        later["status"], "admitted",
        "reader blocked behind interrupted inference"
    );
    run.send(r#"{"type":"end"}"#);
    provider.release();
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 1, "{seen:#?}");
    let results: Vec<_> = seen
        .iter()
        .filter(|line| line["type"] == "result")
        .collect();
    assert_eq!(results.len(), 2, "{seen:#?}");
    let cancelled = results.iter().find(|line| line["id"] == "first").unwrap();
    assert_eq!(cancelled["message_id"], first["message_id"]);
    assert_eq!(cancelled["exit_code"], 1);
    let completed = results.iter().find(|line| line["id"] == "later").unwrap();
    assert_eq!(completed["message_id"], later["message_id"]);
    assert_eq!(completed["exit_code"], 0);
    assert_eq!(completed["result"], "later answer");
    assert_eq!(run.exit(), Some(1));
}
#[test]
fn malformed_input_during_inference_settles_owned_work_and_exits_2() {
    let provider = Provider::start(&["gate:text:cancelled answer"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"first","text":"start"}"#);
    let input = run.until("input", &mut seen);
    provider.wait_for_gate();
    run.send("invalid json");
    let end = run.until("end", &mut seen);
    provider.release();
    assert_eq!(end["exit_code"], 2, "{seen:#?}");
    let result = seen.iter().find(|line| line["type"] == "result").unwrap();
    assert_eq!(result["message_id"], input["message_id"]);
    assert_eq!(result["exit_code"], 1);
    assert_eq!(run.exit(), Some(2));
}
#[cfg(unix)]
#[test]
fn sigint_stops_owned_background_process_before_exit_130() {
    let provider = Provider::start(&[
        r#"bash {"command":"i=0; while [ $i -lt 1000 ]; do echo tick >> ticks; i=$((i+1)); sleep 0.02; done","background":true}"#,
        "text:background started",
    ]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","id":"background","text":"start"}"#);
    let input = run.until("input", &mut seen);
    let request = run.until("permission_request", &mut seen);
    run.send(
        &json!({"type":"permission_reply","request_id":request["id"],"reply":"once"}).to_string(),
    );
    run.until("assistant_text", &mut seen);
    let ticks = run._root.join("repo/ticks");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ticks.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-INT", &run.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 130, "{seen:#?}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "waited for natural background exit instead of stopping it"
    );
    let result = seen.iter().find(|line| line["type"] == "result").unwrap();
    assert_eq!(result["message_id"], input["message_id"]);
    assert_eq!(result["exit_code"], 1);
    let stopped = std::fs::read(&ticks).unwrap();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        std::fs::read(&ticks).unwrap(),
        stopped,
        "process kept writing after acknowledged interruption"
    );
    assert_eq!(run.exit(), Some(130));
    assert_eq!(
        provider.turns().len(),
        2,
        "cancelled Job dispatched a handback"
    );
}

#[test]
fn input_beyond_the_pending_capacity_is_a_protocol_error_that_settles_admitted_inputs() {
    const CAPACITY: usize = 32;
    let provider = Provider::start(&["gate:text:held answer"]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    let mut admitted = Vec::new();
    for index in 0..CAPACITY {
        let id = format!("input-{index}");
        run.send(&json!({"type":"user","text":"next","id":id}).to_string());
        let input = run.until("input", &mut seen);
        assert_eq!(
            (&input["id"], &input["status"]),
            (&json!(id), &json!("admitted"))
        );
        admitted.push(input["message_id"].as_str().unwrap().to_owned());
        if index == 0 {
            provider.wait_for_gate();
        }
    }
    run.send(r#"{"type":"user","text":"one too many","id":"overflow"}"#);
    let error = run.until("error", &mut seen);
    assert_eq!(error["code"], "protocol", "{seen:#?}");
    assert!(
        error["message"].as_str().unwrap().contains("32 inputs"),
        "{error}"
    );
    provider.release();
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 2, "{seen:#?}");
    assert_eq!(run.exit(), Some(2));
    // The refused input was never admitted or sent; every admitted one is settled.
    assert!(
        !seen
            .iter()
            .any(|line| line["id"] == "overflow" && line["type"] == "input")
    );
    let settled: std::collections::BTreeSet<&str> = seen
        .iter()
        .filter(|line| line["type"] == "result")
        .filter_map(|line| line["message_id"].as_str())
        .collect();
    for message in &admitted {
        assert!(
            settled.contains(message.as_str()),
            "{message} unsettled: {seen:#?}"
        );
    }
    assert!(
        provider
            .turns()
            .iter()
            .all(|turn| !turn.to_string().contains("one too many"))
    );
}

#[test]
fn an_oversized_line_is_a_protocol_error_exit_2() {
    let provider = Provider::start(&[]);
    let mut run = Driven::start(&provider, &[]);
    let mut seen = Vec::new();
    let text = "x".repeat(1024 * 1024);
    run.send(&json!({"type":"user","text":text}).to_string());
    let error = run.until("error", &mut seen);
    assert_eq!(error["code"], "protocol");
    assert_eq!(run.until("end", &mut seen)["exit_code"], 2);
    assert_eq!(run.exit(), Some(2));
    assert!(provider.turns().is_empty(), "nothing was sent to the model");
}

#[test]
fn a_stalled_stdout_reader_delays_but_never_loses_or_reorders_results() {
    // Each answer is larger than a pipe buffer, so the driver blocks writing it.
    let answers: Vec<String> = (0..3)
        .map(|i| format!("answer {i} ").repeat(30_000))
        .collect();
    let script: Vec<String> = answers.iter().map(|a| format!("text:{a}")).collect();
    let script: Vec<&str> = script.iter().map(String::as_str).collect();
    let provider = Provider::start(&script);
    let reading = Arc::new(AtomicBool::new(false));
    let mut run = Driven::start_reading(&provider, &[], reading.clone(), None);
    for index in 0..answers.len() {
        run.send(&json!({"type":"user","text":"next","id":format!("input-{index}")}).to_string());
    }
    run.send(r#"{"type":"end"}"#);
    // Let the driver fill the pipe and block before anyone reads it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while provider.turns().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(Duration::from_millis(500));
    assert!(
        run.child.try_wait().unwrap().is_none(),
        "exited while stdout was full"
    );
    reading.store(true, Ordering::SeqCst);
    let mut seen = Vec::new();
    let end = run.until("end", &mut seen);
    assert_eq!(
        (end["exit_code"].as_i64(), end["inputs"].as_i64()),
        (Some(0), Some(3))
    );
    let results: Vec<_> = seen
        .iter()
        .filter(|line| line["type"] == "result")
        .collect();
    assert_eq!(results.len(), answers.len());
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result["id"], format!("input-{index}"));
        assert_eq!(result["result"].as_str(), Some(answers[index].as_str()));
    }
    assert_eq!(run.exit(), Some(0));
}

/// `cyber serve` on its own Cyber home, configured with the scripted provider.
struct Server {
    _dir: tempfile::TempDir,
    root: PathBuf,
    child: Child,
    port: u16,
    password: String,
}

impl Server {
    fn start(provider: &Provider) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        let config = json!({"providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}});
        std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let (child, port) = Self::serve(&root);
        let password = Self::password(&root);
        Self {
            _dir: dir,
            root,
            child,
            port,
            password,
        }
    }

    fn cyber(root: &std::path::Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("CYBER_HOME", root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MODELS_PATH", root.join("models.json"))
            .env(
                "CYBER_SANDBOX_HELPER",
                PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec"),
            )
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(root);
        command
    }

    /// Port 0 and the announced URL: a port probed free here could be taken by a
    /// parallel test's listener before the server binds it.
    fn serve(root: &std::path::Path) -> (Child, u16) {
        let mut serve = Self::cyber(root, &["serve", "--port", "0"]);
        #[cfg(unix)]
        serve.arg("--socket").arg(root.join("cyber/state/s.sock"));
        let mut child = serve
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(root.join("serve.stderr")).unwrap())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut announced = String::new();
        stdout.read_line(&mut announced).unwrap();
        // Keep draining, so a later line never meets a closed pipe.
        thread::spawn(move || std::io::copy(&mut stdout, &mut std::io::sink()));
        let port = announced
            .trim()
            .rsplit(':')
            .next()
            .and_then(|port| port.parse().ok())
            .unwrap_or_else(|| {
                let stderr = std::fs::read_to_string(root.join("serve.stderr"));
                panic!("server did not listen: {announced:?} {stderr:?}")
            });
        (child, port)
    }

    fn password(root: &std::path::Path) -> String {
        let output = Self::cyber(root, &["service", "password"])
            .output()
            .unwrap();
        let password = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert!(!password.is_empty(), "no server password");
        password
    }

    /// Kill the server without a graceful stop.
    fn crash(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Start the server again on the same home and database, on a new port.
    fn restart(&mut self) {
        self.crash();
        (self.child, self.port) = Self::serve(&self.root);
        self.password = Self::password(&self.root);
    }

    /// `GET /api/v1{path}` on the server, directly.
    fn get(&self, path: &str) -> Value {
        let client = cyber_client::Client::http(
            &format!("http://127.0.0.1:{}", self.port),
            Some(self.password.clone()),
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(client.get(path))
            .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the lossy proxy drops.
#[derive(Clone, Copy)]
struct Loss {
    /// The first this many `POST …/prompt` requests reach the server but their responses
    /// are dropped: the admission is committed and its acknowledgement lost.
    prompts: usize,
    /// Every `GET …/inbox` connection is closed before it reaches the server.
    inbox: bool,
}

/// Forwards each request on its own upstream connection, dropping what `loss` names.
fn lossy_proxy(upstream: Arc<AtomicU16>, loss: Loss, lost: Arc<Mutex<usize>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for client in listener.incoming().map_while(Result::ok) {
            let lost = lost.clone();
            let upstream = upstream.load(Ordering::SeqCst);
            thread::spawn(move || relay(client, upstream, loss, &lost));
        }
    });
    url
}

fn relay(mut client: TcpStream, upstream: u16, loss: Loss, lost: &Mutex<usize>) {
    let Some((head, body)) = read_request(&mut client) else {
        return;
    };
    let line = head.lines().next().unwrap_or_default();
    if loss.inbox && line.starts_with("GET ") && line.contains("/inbox ") {
        let _ = client.shutdown(std::net::Shutdown::Both);
        return;
    }
    let lose = line.starts_with("POST ") && line.contains("/prompt ") && {
        let mut lost = lost.lock().unwrap();
        let again = *lost < loss.prompts;
        *lost += usize::from(again);
        again
    };
    let Ok(mut server) = TcpStream::connect(("127.0.0.1", upstream)) else {
        return;
    };
    let head: String = head
        .lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    server
        .write_all(format!("{head}connection: close\r\n\r\n").as_bytes())
        .unwrap();
    server.write_all(&body).unwrap();
    let mut response = Vec::new();
    if lose {
        let _ = server.read_to_end(&mut response);
        let _ = client.shutdown(std::net::Shutdown::Both);
        return;
    }
    let _ = std::io::copy(&mut server, &mut client);
}

/// One request's head (without the blank line) and its `Content-Length` body.
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = bytes[end + 4..].to_vec();
            while body.len() < length {
                let n = stream.read(&mut buffer).ok()?;
                if n == 0 {
                    return None;
                }
                body.extend_from_slice(&buffer[..n]);
            }
            return Some((head, body));
        }
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
}

#[test]
fn a_lost_admission_acknowledgement_is_retried_without_a_second_prompt() {
    let provider = Provider::start(&["text:answered once", "text:second answer"]);
    let server = Server::start(&provider);
    // The client retries a mutation three times with one Idempotency-Key; losing all
    // four acknowledgements makes the driver resend the same message identity.
    let lost = Arc::new(Mutex::new(0));
    let loss = Loss {
        prompts: 4,
        inbox: false,
    };
    let url = lossy_proxy(Arc::new(AtomicU16::new(server.port)), loss, lost.clone());
    let mut run = Driven::start(
        &provider,
        &["--attach", &url, "--password", &server.password],
    );
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    let input = run.until("input", &mut seen);
    assert_eq!(input["status"], "admitted", "{seen:#?}");
    let result = run.until("result", &mut seen);
    assert_eq!(
        (result["id"].as_str(), result["result"].as_str()),
        (Some("first"), Some("answered once"))
    );
    assert_eq!(result["message_id"], input["message_id"]);
    run.send(r#"{"type":"user","text":"two","id":"second"}"#);
    run.send(r#"{"type":"end"}"#);
    assert_eq!(run.until("result", &mut seen)["result"], "second answer");
    let end = run.until("end", &mut seen);
    assert_eq!(
        (end["exit_code"].as_i64(), end["inputs"].as_i64()),
        (Some(0), Some(2))
    );
    assert_eq!(run.exit(), Some(0));
    assert_eq!(*lost.lock().unwrap(), 4, "acknowledgements lost");
    // The retried admission did not run the prompt twice.
    let turns = provider.turns();
    assert_eq!(turns.len(), 2, "{turns:#?}");
    let users = turns[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .count();
    assert_eq!(users, 1, "{:#}", turns[0]);
}

/// Both admission attempts lose every acknowledgement (four client tries each); the
/// committed prompt is found in the inbox and reported admitted, not refused.
#[test]
fn an_admission_whose_retry_is_also_lost_is_confirmed_from_the_inbox() {
    let provider = Provider::start(&["text:answered once"]);
    let server = Server::start(&provider);
    let lost = Arc::new(Mutex::new(0));
    let loss = Loss {
        prompts: 8,
        inbox: false,
    };
    let url = lossy_proxy(Arc::new(AtomicU16::new(server.port)), loss, lost.clone());
    let mut run = Driven::start(
        &provider,
        &["--attach", &url, "--password", &server.password],
    );
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    run.send(r#"{"type":"end"}"#);
    let input = run.until("input", &mut seen);
    assert_eq!(input["status"], "admitted", "{seen:#?}");
    let result = run.until("result", &mut seen);
    assert_eq!(result["result"], "answered once", "{seen:#?}");
    assert_eq!(result["message_id"], input["message_id"]);
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 0, "{seen:#?}");
    assert_eq!(run.exit(), Some(0));
    assert_eq!(*lost.lock().unwrap(), 8, "acknowledgements lost");
    assert_eq!(provider.turns().len(), 1);
}

/// When the inbox cannot be read either, the driver cannot tell whether the prompt runs:
/// the run fails instead of reporting it refused.
#[test]
fn an_unconfirmable_admission_fails_the_run() {
    let provider = Provider::start(&["text:unseen"]);
    let server = Server::start(&provider);
    let loss = Loss {
        prompts: 8,
        inbox: true,
    };
    let url = lossy_proxy(
        Arc::new(AtomicU16::new(server.port)),
        loss,
        Arc::new(Mutex::new(0)),
    );
    let mut run = Driven::start(
        &provider,
        &["--attach", &url, "--password", &server.password],
    );
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 2, "{seen:#?}");
    let error = end["error"].as_str().unwrap_or_default();
    assert!(error.contains("cannot confirm admission"), "{seen:#?}");
    assert!(
        seen.iter().all(|line| line["status"] != "refused"),
        "{seen:#?}"
    );
    let result = seen.iter().find(|line| line["type"] == "result");
    assert!(
        result.is_some_and(|result| result["id"] == "first"
            && result["exit_code"] != 0
            && result["error"]
                .as_str()
                .is_some_and(|e| e.contains("cannot confirm"))),
        "{seen:#?}"
    );
    assert_eq!(run.exit(), Some(2));
}

/// A server that dies under an attached driver ends the run explicitly instead of
/// hanging; the admitted input stays in the Session for the restarted server.
#[test]
fn a_server_crash_ends_the_run_explicitly_and_the_restarted_server_keeps_the_input() {
    let provider = Provider::start(&["gate:text:never delivered", "gate:text:after restart"]);
    let mut server = Server::start(&provider);
    let upstream = Arc::new(AtomicU16::new(server.port));
    let loss = Loss {
        prompts: 0,
        inbox: false,
    };
    let url = lossy_proxy(upstream.clone(), loss, Arc::new(Mutex::new(0)));
    let mut run = Driven::start(
        &provider,
        &["--attach", &url, "--password", &server.password],
    );
    let mut seen = Vec::new();
    run.send(r#"{"type":"user","text":"one","id":"first"}"#);
    let input = run.until("input", &mut seen);
    assert_eq!(input["status"], "admitted", "{seen:#?}");
    provider.wait_for_gate();
    server.crash();

    let end = run.until("end", &mut seen);
    assert_eq!(end["exit_code"], 2, "{seen:#?}");
    assert_eq!(
        end["error"], "the server closed the event stream",
        "{seen:#?}"
    );
    let result = seen.iter().find(|line| line["type"] == "result");
    assert!(
        result.is_some_and(|result| result["message_id"] == input["message_id"]
            && result["error"] == "not answered before the run ended"
            && result["exit_code"] != 0),
        "{seen:#?}"
    );
    assert_eq!(run.exit(), Some(2));

    server.restart();
    upstream.store(server.port, Ordering::SeqCst);
    let session = input["session_id"].as_str().unwrap();
    let inbox = server.get(&format!("/sessions/{session}/inbox"));
    let row = inbox["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["message_id"] == input["message_id"]);
    assert!(row.is_some(), "{inbox:#}");
    provider.release();
}

/// Two drivers attached to one Session: a request bound to the first driver's input is
/// never shown to the second, and the second cannot answer it.
#[test]
fn a_second_driver_cannot_reply_to_the_first_drivers_request() {
    let provider = Provider::start(&[r#"gate:bash {"command":"echo driven"}"#, "text:ran it"]);
    let server = Server::start(&provider);
    let url = format!("http://127.0.0.1:{}", server.port);
    let attach = [
        "--attach",
        url.as_str(),
        "--password",
        server.password.as_str(),
    ];
    let mut first = Driven::start(&provider, &attach);
    let mut seen = Vec::new();
    first.send(r#"{"type":"user","text":"run it","id":"first"}"#);
    let input = first.until("input", &mut seen);
    assert_eq!(input["status"], "admitted", "{seen:#?}");
    let session = input["session_id"].as_str().unwrap().to_owned();
    provider.wait_for_gate();

    // The second driver takes its baseline before the request is asked, so the
    // request event reaches it too.
    let mut flags = attach.to_vec();
    flags.extend(["--session", session.as_str()]);
    let mut second = Driven::start_beside(&provider, &flags, &first);
    let mut other = Vec::new();
    second.until("system", &mut other);
    provider.release();

    let asked = first.until("permission_request", &mut seen);
    assert_eq!(asked["input_message_ids"], json!([input["message_id"]]));
    second.send(
        &json!({"type":"permission_reply","request_id":asked["id"],"reply":"once"}).to_string(),
    );
    let refused = second.until("error", &mut other);
    assert_eq!(refused["code"], "protocol", "{refused}");
    first.send(
        &json!({"type":"permission_reply","request_id":asked["id"],"reply":"once"}).to_string(),
    );
    let result = first.until("result", &mut seen);
    assert_eq!(
        (result["id"].as_str(), result["result"].as_str()),
        (Some("first"), Some("ran it")),
        "{seen:#?}"
    );
    assert!(
        !seen.iter().any(|l| l["type"] == "permission_denied"),
        "{seen:#?}"
    );
    first.send(r#"{"type":"end"}"#);
    assert_eq!(first.until("end", &mut seen)["exit_code"], 0, "{seen:#?}");
    assert_eq!(first.exit(), Some(0));

    second.send(r#"{"type":"end"}"#);
    let end = second.until("end", &mut other);
    assert_eq!(end["exit_code"], 0, "{other:#?}");
    assert!(
        other
            .iter()
            .all(|l| l["type"] != "permission_request" && l["type"] != "result"),
        "{other:#?}"
    );
    assert_eq!(second.exit(), Some(0));
    let turns = provider.turns();
    assert_eq!(turns.len(), 2, "the tool ran once: {turns:#?}");
    assert!(turns[1].to_string().contains("driven"), "{:#}", turns[1]);
}
