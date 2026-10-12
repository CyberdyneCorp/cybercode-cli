//! `cyber exec --embedded` calls a remote Streamable HTTP MCP tool through the real app
//! host: a fake OpenAI-compatible provider asks for `mcp__remote__echo` and receives the
//! fixture's result.
#[path = "../../cyber-tools/tests/support/mcp_http.rs"]
mod mcp_http;

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

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

fn stream(stream: &mut TcpStream, delta: Value, finish: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":100,"completion_tokens":10}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The first conversation request calls the remote tool; once a tool result is present the
/// model answers with plain text.
fn answer(out: &mut TcpStream, body: &Value) {
    let messages = &body["messages"];
    if messages.to_string().contains("Write a short title") {
        return stream(out, json!({"content":"Remote title"}), "stop");
    }
    let called = messages
        .as_array()
        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
    if called {
        return stream(out, json!({"content":"remote call finished"}), "stop");
    }
    let call = json!({"tool_calls":[{"index":0,"id":"call_remote","type":"function",
        "function":{"name":"mcp__remote__echo","arguments":json!({"text":"REMOTE-ECHO-RESULT"}).to_string()}}]});
    stream(out, call, "tool_calls");
}

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
                answer(&mut stream, &body);
                seen.lock().unwrap().push(body);
            }
        });
        Self {
            url,
            requests,
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

/// Runs exec with a 60 s ceiling so a hung run fails instead of blocking the suite.
fn exec(root: &Path) -> (Option<i32>, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["--cwd", root.join("repo").to_str().unwrap(), "exec"])
        .args(["--embedded", "--model", "corp/main", "--format", "json"])
        .arg("call the remote tool")
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    for name in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    (status, out.join().unwrap(), err.join().unwrap())
}

#[test]
fn exec_calls_a_remote_streamable_http_tool_through_the_app_host() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
    let provider = Provider::start();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(
        root.join("cyber/config/cyber.jsonc"),
        json!({
            "permissions":"allow",
            "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}},
            "mcp":{"remote":{"type":"remote","url":http.url,"required":true}}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();

    let (status, stdout, stderr) = exec(&root);
    let requests = provider.requests.lock().unwrap().clone();
    let detail = format!(
        "status={status:?}\nstdout={stdout}\nstderr={stderr}\nrequests={requests:#?}\nfixture={:#?}",
        http.requests()
    );
    let calls = http.posts("tools/call");
    assert_eq!(calls.len(), 1, "{detail}");
    assert_eq!(calls[0].body["params"]["name"], "echo", "{detail}");
    let delivered = requests.iter().any(|body| {
        body["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["role"] == "tool" && message.to_string().contains("REMOTE-ECHO-RESULT")
            })
        })
    });
    assert!(
        delivered,
        "the model never received the tool result\n{detail}"
    );
    assert_eq!(status, Some(0), "{detail}");
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(result["result"], "remote call finished", "{detail}");
    // Embedded exec drops the App without a runtime shutdown, so session DELETE is
    // covered through the app host's Location close instead (cyber-app tests/mcp.rs).
    drop(http);
    drop(runtime);
}
