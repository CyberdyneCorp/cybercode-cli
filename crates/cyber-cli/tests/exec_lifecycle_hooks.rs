//! Runtime lifecycle hooks (SessionStart, UserPromptSubmit, Stop) run through the
//! real app host: `cyber exec --embedded` against a fake provider.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Reads one HTTP request and returns its JSON body.
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

fn reply(stream: &mut TcpStream, text: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A fake OpenAI-compatible provider: the Stop continuation gets "second answer",
/// anything else "first answer".
struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let (seen, stop) = (requests.clone(), done.clone());
        let server = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let Some(body) = read_body(&mut stream) else {
                    continue;
                };
                let continued = body["messages"].to_string().contains("keep going");
                seen.lock().unwrap().push(body);
                reply(
                    &mut stream,
                    if continued {
                        "second answer"
                    } else {
                        "first answer"
                    },
                );
            }
        });
        Self {
            url,
            requests,
            done,
            server: Some(server),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
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

fn helper() -> PathBuf {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec");
    assert!(
        helper.is_file(),
        "build the helper first: cargo build -p cyber-sandbox --bin cyber-sandbox-exec"
    );
    helper
}

/// A command hook that returns `additional_context`.
fn context(id: &str, text: &str) -> Value {
    json!({"type":"command","id":id,"command":format!(
        "cat >/dev/null; printf '%s' '{}'",
        json!({"additional_context":text})
    )})
}

/// A Stop hook that blocks once, recording that it ran in `flag`.
fn block_once(flag: &Path) -> Value {
    let flag = flag.display();
    json!({"type":"command","id":"guard","command":format!(
        "cat >/dev/null; if [ ! -e '{flag}' ]; then : > '{flag}'; printf '%s' '{}'; fi",
        json!({"decision":"block","reason":"keep going"})
    )})
}

/// Runs exec, killing it after 60 s so a hung exec fails instead of blocking.
fn exec(root: &Path) -> std::process::Output {
    let child = Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["--cwd", root.join("repo").to_str().unwrap(), "exec"])
        .args(["--embedded", "--model", "corp/main", "--format", "json"])
        .arg("hello")
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("CYBER_SANDBOX_HELPER", helper())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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

#[test]
fn exec_runs_session_start_prompt_and_stop_hooks_through_the_app_host() {
    let provider = Provider::start();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    let stopped = root.join("stop-ran");
    std::fs::write(
        root.join("cyber/config/cyber.jsonc"),
        json!({
            "permissions":"allow",
            "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}},
            "hooks":{
                "SessionStart":[{"hooks":[context("start","SESSION START MARKER")]}],
                "UserPromptSubmit":[{"hooks":[context("prompt","PROMPT CONTEXT MARKER")]}],
                "Stop":[{"hooks":[block_once(&stopped)]}]
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();

    let output = exec(&root);
    // Conversation Turns only; the title request runs beside them.
    let requests: Vec<_> = provider
        .requests()
        .into_iter()
        .filter(|body| !body["messages"].to_string().contains("Write a short title"))
        .collect();
    let messages: Vec<_> = requests.iter().map(|body| &body["messages"]).collect();
    let detail = format!(
        "status={:?}\nstdout={}\nstderr={}\nmessages={messages:#?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let first = requests
        .first()
        .unwrap_or_else(|| panic!("no provider request\n{detail}"))["messages"]
        .to_string();
    assert!(first.contains("SESSION START MARKER"), "{detail}");
    assert!(first.contains("PROMPT CONTEXT MARKER"), "{detail}");
    assert!(stopped.exists(), "the Stop hook never ran\n{detail}");
    assert_eq!(requests.len(), 2, "one Stop continuation\n{detail}");
    assert!(output.status.success(), "{detail}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"], "second answer", "{detail}");
}
