//! `cyber exec --output-schema` through the real binary (`exec-mode` → Structured final
//! output) against a scripted OpenAI-compatible provider: the validated object is the
//! result, three invalid submissions exit 1 with `StructuredOutputError`, and a bad
//! schema file exits 2 before any request.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// Answers each Turn request with the next scripted `structured_output` arguments (or
/// text), and title requests (without tools) with a title.
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

    fn turns(&self) -> usize {
        self.turns.lock().unwrap().len()
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
        turns.lock().unwrap().push(body);
        let next = script.lock().unwrap().pop_front().unwrap_or_default();
        match next.strip_prefix("text:") {
            Some(text) => {
                json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]})
            }
            None => {
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call_{}", turns.lock().unwrap().len()),"type":"function","function":{"name":"structured_output","arguments":next}}]},"finish_reason":"tool_calls"}]})
            }
        }
    } else {
        json!({"choices":[{"index":0,"delta":{"content":"Audit"},"finish_reason":"stop"}]})
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

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new(provider: &Provider) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        let config = json!({"providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}});
        std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let schema = json!({"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]});
        std::fs::write(root.join("schema.json"), schema.to_string()).unwrap();
        Self { _dir: dir, root }
    }

    fn exec(&self, flags: &[&str]) -> Output {
        let helper =
            PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec");
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["--cwd", self.root.join("repo").to_str().unwrap()])
            .args(["exec", "--embedded", "--model", "corp/main"])
            .args(flags)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_SANDBOX_HELPER", helper)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn schema(&self) -> String {
        self.root.join("schema.json").display().to_string()
    }

    fn write(&self, name: &str, text: &str) -> String {
        let path: &Path = &self.root.join(name);
        std::fs::write(path, text).unwrap();
        path.display().to_string()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_validated_object_is_the_result() {
    let provider = Provider::start(&[r#"{"summary":"no issues"}"#, r#"{"summary":"again"}"#]);
    let env = Env::new(&provider);
    let schema = env.schema();

    let text = env.exec(&["--output-schema", &schema, "audit deps"]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let printed: Value = serde_json::from_str(&stdout(&text)).unwrap();
    assert_eq!(printed, json!({"summary": "no issues"}));

    let json_run = env.exec(&[
        "--output-schema",
        &schema,
        "--format",
        "json",
        "audit again",
    ]);
    assert_eq!(json_run.status.code(), Some(0), "{json_run:?}");
    let result: Value = serde_json::from_str(stdout(&json_run).trim()).unwrap();
    assert_eq!(result["result"], json!({"summary": "again"}));
    assert_eq!(result["error"], Value::Null);
    assert_eq!(provider.turns(), 2);
}

#[test]
fn three_invalid_submissions_exit_1_without_a_fourth() {
    let provider = Provider::start(&[
        r#"{"other":1}"#,
        "text:prose",
        r#"{"summary":5}"#,
        r#"{"summary":"late"}"#,
    ]);
    let env = Env::new(&provider);
    let run = env.exec(&[
        "--output-schema",
        &env.schema(),
        "--format",
        "json",
        "audit",
    ]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");
    let result: Value = serde_json::from_str(stdout(&run).trim()).unwrap();
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("StructuredOutputError")),
        "{result}"
    );
    assert_eq!(result["result"], Value::Null);
    assert_eq!(provider.turns(), 3, "no fourth submission");
}

#[test]
fn a_bad_schema_exits_2_before_any_request() {
    let provider = Provider::start(&[]);
    let env = Env::new(&provider);
    let not_object = env.write("string.json", r#"{"type":"string"}"#);
    let run = env.exec(&["--output-schema", &not_object, "audit"]);
    assert_eq!(run.status.code(), Some(2), "{run:?}");
    assert!(String::from_utf8_lossy(&run.stderr).contains("object schema"));
    let conflict = env.exec(&["--output-schema", &env.schema(), "--command", "review", "x"]);
    assert_eq!(conflict.status.code(), Some(2), "{conflict:?}");
    assert_eq!(provider.turns(), 0);
}
