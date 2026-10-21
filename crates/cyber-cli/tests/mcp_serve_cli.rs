//! End-to-end tests of `cyber mcp serve` (`mcp-server` → MCP serve transports): the real
//! binary as an MCP stdio bridge against the shared server it starts, driven over pipes.
//! Children end by closing stdin, never by signals, so the tests also run on Windows.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // An empty model catalog: nothing is fetched from the network.
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self { _dir: dir, root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.root.join("cyber-home")
    }

    fn write_global_config(&self, config: &Value) {
        let dir = self.home().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cyber.jsonc"), config.to_string()).unwrap();
    }

    fn password(&self) -> String {
        std::fs::read_to_string(self.home().join("state/password"))
            .unwrap()
            .trim()
            .to_owned()
    }

    fn registration(&self) -> Value {
        let text = std::fs::read_to_string(self.home().join("state/server.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// Every log file of the data directory, concatenated.
    fn data_logs(&self) -> String {
        let mut text = String::new();
        for entry in std::fs::read_dir(self.home().join("data/log")).unwrap() {
            text.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default());
        }
        text
    }
}

/// Stops the shared server the bridge started when a test ends.
struct Service<'a>(&'a Env);

impl Drop for Service<'_> {
    fn drop(&mut self) {
        let _ = self.0.cyber(&["service", "stop"]);
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn wait_exit(child: &mut Child, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the bridge did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The real `cyber mcp serve` process, spoken to as an MCP client over its pipes.
struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    /// Every stdout line received so far.
    seen: Vec<String>,
    stderr: PathBuf,
}

impl Bridge {
    fn spawn(env: &Env, args: &[&str]) -> Self {
        let stderr = env.root.join(format!("bridge-{}.stderr", unique()));
        let mut child = env
            .command(&[&["mcp", "serve"], args].concat())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            seen: Vec::new(),
            stderr,
        }
    }

    fn send(&mut self, frame: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{frame}").unwrap();
        stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    /// Send a request and return its response; other frames are kept in `seen`.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).unwrap_or_else(|_| {
                panic!(
                    "no response to {method} (id {id}); stderr: {}",
                    std::fs::read_to_string(&self.stderr).unwrap_or_default()
                )
            });
            self.seen.push(line.clone());
            let frame: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
            if frame["id"] == id {
                return frame;
            }
        }
    }

    fn initialize(&mut self, client: &str) -> Value {
        let reply = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": client, "version": "1.0"},
            }),
        );
        self.notify("notifications/initialized");
        reply
    }

    /// The next stdout frame, within `limit`.
    fn next_frame(&mut self, limit: Duration) -> Value {
        let line = self
            .lines
            .recv_timeout(limit)
            .unwrap_or_else(|_| panic!("no frame within {limit:?}"));
        self.seen.push(line.clone());
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, id: u64, tool: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
        )
    }

    /// Close stdin, wait for the exit and collect the rest of stdout.
    fn close(mut self) -> (ExitStatus, Vec<String>, String) {
        drop(self.stdin.take());
        let status = wait_exit(&mut self.child, Duration::from_secs(20));
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            self.seen.push(line);
        }
        let stderr = std::fs::read_to_string(&self.stderr).unwrap_or_default();
        (status, self.seen, stderr)
    }
}

fn unique() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn assert_jsonrpc_frames(lines: &[String]) {
    for line in lines {
        let frame: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
        assert_eq!(frame["jsonrpc"], "2.0", "{line}");
    }
}

fn initialize_results(lines: &[String]) -> usize {
    lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|frame| frame["result"]["serverInfo"].is_object())
        .count()
}

/// POST one JSON-RPC frame to the endpoint over TCP with the local password and return
/// the HTTP status.
fn post_status(env: &Env, mcp_session: &str, frame: &Value) -> u16 {
    use base64::Engine as _;
    let url = env.registration()["url"].as_str().unwrap().to_owned();
    let authority = url.trim_start_matches("http://").trim_end_matches('/');
    let mut stream = std::net::TcpStream::connect(authority).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = frame.to_string();
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("cyber:{}", env.password()));
    write!(
        stream,
        "POST /api/v1/mcp/serve HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Basic {credentials}\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Mcp-Session-Id: {mcp_session}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no HTTP status in {response:?}"))
}

/// The MCP session the bridge logged for its last `initialize`.
fn logged_mcp_session(env: &Env) -> String {
    let log = std::fs::read_to_string(env.home().join("data/log/mcp-serve.log"))
        .expect("the bridge writes <data>/log/mcp-serve.log");
    log.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|line| line["mcp_session"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("no MCP session in the bridge log: {log}"))
}

#[test]
fn an_invalid_client_label_and_a_port_are_usage_errors() {
    let env = Env::new();
    let bad = env
        .command(&["mcp", "serve", "--client", "Bad Label"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2), "{}", stderr(&bad));
    assert!(stderr(&bad).contains("--client"), "{}", stderr(&bad));
    assert!(bad.stdout.is_empty());

    let port = env
        .command(&["mcp", "serve", "--port", "1"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(port.status.code(), Some(2), "{}", stderr(&port));
    assert!(stderr(&port).contains("--port"), "{}", stderr(&port));
}

#[test]
fn disabled_in_the_global_config_exits_before_reading_stdin() {
    let env = Env::new();
    env.write_global_config(&json!({"mcp_serve": {"enabled": false}}));
    let mut child = env
        .command(&["mcp", "serve", "--client", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Stdin stays open: an exit proves the bridge never waited for a frame.
    let stdin = child.stdin.take();
    let status = wait_exit(&mut child, Duration::from_secs(20));
    drop(stdin);
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(status.code(), Some(1), "{err}");
    assert!(err.contains("MCP serve is disabled"), "{err}");
    assert!(out.is_empty(), "stdout carries MCP frames only: {out:?}");
    assert!(
        !env.home().join("state/server.json").exists(),
        "a disabled bridge starts no server"
    );
}

#[test]
fn http_transport_prints_the_endpoint_and_ignores_project_mcp_serve() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(
        repo.join("cyber.jsonc"),
        json!({"mcp_serve": {"enabled": false}}).to_string(),
    )
    .unwrap();
    let _service = Service(&env);
    let output = env
        .command(&["mcp", "serve", "--transport", "http"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("mcp_serve is ignored outside the global config"),
        "{}",
        stderr(&output)
    );
    let text = stdout(&output);
    let url = env.registration()["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    assert!(
        text.lines()
            .any(|line| line.trim() == format!("{url}/api/v1/mcp/serve")),
        "{text}"
    );
    assert!(text.contains("cyber service password"), "{text}");
    let password = env.password();
    assert!(password.len() >= 16);
    assert!(!text.contains(&password), "the password is never printed");
    assert!(!stderr(&output).contains(&password));
}

#[test]
fn the_stdio_bridge_serves_the_mcp_lifecycle_and_deletes_its_session_on_eof() {
    let env = Env::new();
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &["--client", "claude-code"]);
    let init = bridge.initialize("Claude Code");
    assert_eq!(init["result"]["serverInfo"]["name"], "cyber-code", "{init}");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let tools = bridge.request(2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{tools}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"cyber_session_list"), "{names:?}");

    let listed = bridge.call(3, "cyber_session_list", json!({}));
    assert_ne!(listed["result"]["isError"], true, "{listed}");
    assert!(
        listed["result"]["structuredContent"].is_object(),
        "{listed}"
    );

    let pong = bridge.request(4, "ping", json!({}));
    assert_eq!(pong["result"], json!({}), "{pong}");
    let tasks = bridge.request(5, "tasks/list", json!({}));
    assert_eq!(tasks["error"]["code"], -32601, "{tasks}");

    let mcp_session = logged_mcp_session(&env);
    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);
    assert_eq!(initialize_results(&lines), 1, "{lines:?}");
    assert_eq!(
        post_status(
            &env,
            &mcp_session,
            &json!({"jsonrpc": "2.0", "id": 9, "method": "ping"})
        ),
        404,
        "stdin EOF ended the MCP session"
    );
    let logs = env.data_logs();
    assert!(
        logs.lines()
            .any(|line| line.contains("\"DELETE\"") && line.contains("mcp/serve")),
        "the server logged the DELETE: {logs}"
    );
    assert!(env.home().join("data/log/mcp-serve.log").is_file());
}

#[test]
fn the_stdio_bridge_reinitializes_silently_after_a_server_restart() {
    let env = Env::new();
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &[]);
    let init = bridge.initialize("Restart Probe");
    assert_eq!(init["result"]["serverInfo"]["name"], "cyber-code", "{init}");
    let listed = bridge.call(2, "cyber_session_list", json!({}));
    assert_ne!(listed["result"]["isError"], true, "{listed}");

    let stopped = env.cyber(&["service", "stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    let down = bridge.call(3, "cyber_session_list", json!({}));
    assert_eq!(down["error"]["code"], -32010, "{down}");
    assert!(down["error"]["data"]["_tag"].is_string(), "{down}");

    let started = env.cyber(&["service", "start"]);
    assert!(started.status.success(), "{}", stderr(&started));
    let again = bridge.call(4, "cyber_session_list", json!({}));
    assert_ne!(again["result"]["isError"], true, "{again}");
    assert!(again["result"]["structuredContent"].is_object(), "{again}");

    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);
    assert_eq!(
        initialize_results(&lines),
        1,
        "the re-initialize is silent: {lines:?}"
    );
    let log = std::fs::read_to_string(env.home().join("data/log/mcp-serve.log")).unwrap();
    assert!(
        log.lines()
            .filter(|line| line.contains("\"mcp_session\""))
            .count()
            >= 2,
        "{log}"
    );
}

#[test]
fn a_session_created_through_the_bridge_is_an_ordinary_session() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &["--client", "claude-code"]);
    bridge.initialize("Claude Code");
    let created = bridge.call(
        2,
        "cyber_session_create",
        json!({"directory": repo.display().to_string(), "model": "openai/gpt-6-luna",
            "title": "Bridged", "idempotency_key": "bridge-1"}),
    );
    let result = &created["result"];
    assert_eq!(result["isError"], false, "{created}");
    let session = result["structuredContent"]["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(result["structuredContent"]["effective_mode"], "default");
    assert_eq!(
        result["structuredContent"]["session"]["mcp_client"],
        "claude-code"
    );
    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);

    // `cyber api` sees it in its Location like any other Session.
    let listed = env
        .command(&["api", "v1.session.list"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(listed.status.success(), "{}", stderr(&listed));
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let row = listed["data"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(session))
        .unwrap_or_else(|| panic!("{listed}"))
        .clone();
    assert_eq!(row["title"], "Bridged");
    assert_eq!(row["mode"], "default");
}

/// A loopback OpenAI-compatible model answering every request with `"{ANSWER}"`; a
/// request whose last message mentions `slowly` is answered after three seconds.
struct Model {
    url: String,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const ANSWER: &str = "the failing test passes now";

impl Model {
    fn start() -> Self {
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = std::sync::Arc::clone(&done);
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        std::thread::spawn(move || answer(stream));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            done,
            thread: Some(thread),
        }
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn answer(mut stream: std::net::TcpStream) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let body = loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            if bytes.len() >= end + 4 + length {
                break serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                    .unwrap_or_default();
            }
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    };
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(Value::to_string)
        .unwrap_or_default();
    if last.contains("slowly") {
        std::thread::sleep(Duration::from_secs(3));
    }
    let chunk = json!({"choices":[{"index":0,"delta":{"content":ANSWER},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":10}});
    let reply = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

/// `cyber api <operation> --param sessionID=<session>` from `dir`, as JSON.
fn api(env: &Env, dir: &std::path::Path, operation: &str, session: &str) -> Value {
    let output = env
        .command(&["api", operation, "--param", &format!("sessionID={session}")])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn progress_kind(frame: &Value) -> Option<&str> {
    (frame["method"] == "notifications/progress")
        .then(|| frame["params"]["_meta"]["dev.cyber-code/progress"]["kind"].as_str())
        .flatten()
}

fn frames(lines: &[String]) -> Vec<Value> {
    lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Read frames until the `admitted` progress of the call with `token`; its message.
fn admitted(bridge: &mut Bridge, token: &str) -> String {
    loop {
        let frame = bridge.next_frame(Duration::from_secs(30));
        if progress_kind(&frame) == Some("admitted") && frame["params"]["progressToken"] == token {
            let marker = &frame["params"]["_meta"]["dev.cyber-code/progress"];
            return marker["message_id"].as_str().unwrap().to_owned();
        }
    }
}

/// Whether the Session has the model's answer after the prompt `message`.
fn answered_after(env: &Env, repo: &std::path::Path, session: &str, message: &str) -> bool {
    let messages = api(env, repo, "v1.message.list", session);
    // Newest first: an answer above the prompt.
    messages["data"]
        .as_array()
        .into_iter()
        .flatten()
        .take_while(|m| m["id"] != json!(message))
        .any(|m| m["kind"] == "assistant" && m["text"] == ANSWER)
}

/// `cyber_prompt` on a new Session with progress: completed with the model's answer,
/// after an `admitted` notification naming the prompt. Returns its Session and message.
fn first_prompt(bridge: &mut Bridge, directory: &str) -> (String, Value) {
    let reply = bridge.request(
        2,
        "tools/call",
        json!({"name": "cyber_prompt", "_meta": {"progressToken": "p-1"},
            "arguments": {"directory": directory, "model": "test/main",
                "prompt": "fix the failing test", "idempotency_key": "drive-1"}}),
    );
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "{reply}");
    let first = &result["structuredContent"];
    assert_eq!(first["status"], "completed", "{first}");
    assert_eq!(first["final_text"], ANSWER);
    let session = first["session_id"].as_str().unwrap().to_owned();
    let message = first["message_id"].clone();
    let progress: Vec<Value> = frames(&bridge.seen)
        .into_iter()
        .filter(|frame| progress_kind(frame).is_some())
        .collect();
    assert_eq!(
        progress_kind(&progress[0]),
        Some("admitted"),
        "{progress:?}"
    );
    assert_eq!(
        progress[0]["params"]["_meta"]["dev.cyber-code/progress"]["message_id"],
        message
    );
    assert_eq!(progress[0]["params"]["progressToken"], "p-1");
    (session, message)
}

#[test]
fn another_agent_drives_cyber_code_through_the_bridge() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let model = Model::start();
    env.write_global_config(&json!({
        "providers":{"test":{"api":{"type":"openai-compatible","url":model.url,
            "settings":{"auth":"none"}},"models":{"main":{}}}},
    }));
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &["--client", "claude-code"]);
    bridge.initialize("Claude Code");
    let directory = repo.display().to_string();

    let (session, message) = first_prompt(&mut bridge, &directory);
    let read = bridge.call(3, "cyber_session_read", json!({"session_id": session}));
    let read = &read["result"]["structuredContent"];
    assert_eq!(read["session"]["mcp_client"], "claude-code", "{read}");
    assert_eq!(read["recent_prompts"][0]["message_id"], message, "{read}");
    assert_eq!(read["recent_prompts"][0]["status"], "completed");

    // Stdin closes while a second prompt waits: only the wait ends.
    bridge.send(
        &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
        "name":"cyber_prompt","_meta":{"progressToken":"p-2"},
        "arguments":{"session_id": session, "prompt": "now slowly check the lint",
            "idempotency_key": "drive-2"}}}),
    );
    let second = admitted(&mut bridge, "p-2");
    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);
    let answered = frames(&lines).iter().any(|frame| frame["id"] == 4);
    assert!(!answered, "no answer after stdin EOF: {lines:?}");

    // The Turn runs to completion, as `cyber api` shows. The answer is stored before the
    // Session goes idle, so wait for both.
    let deadline = Instant::now() + Duration::from_secs(30);
    let session_row = loop {
        let row = api(&env, &repo, "v1.session.get", &session);
        if row["data"]["status"] == "idle" && answered_after(&env, &repo, &session, &second) {
            break row;
        }
        assert!(
            Instant::now() < deadline,
            "{row}\n{}",
            api(&env, &repo, "v1.message.list", &session)
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(session_row["data"]["mode"], "default", "{session_row}");
    drop(model);
}

/// End a process abruptly.
fn kill(pid: u64) {
    let pid = pid.to_string();
    let status = if cfg!(windows) {
        Command::new("taskkill").args(["/F", "/PID", &pid]).status()
    } else {
        Command::new("kill").args(["-9", &pid]).status()
    };
    assert!(status.unwrap().success(), "could not kill {pid}");
}

/// The receipt of the newest prompt admitted through MCP serve, asked through the
/// bridge while another call waits: `(session_id, message_id)`.
fn admitted_prompt(bridge: &mut Bridge, id: u64) -> Option<(Value, Value)> {
    let listed = bridge.call(id, "cyber_session_list", json!({}));
    let session = listed["result"]["structuredContent"]["sessions"][0]["session_id"].clone();
    let session_id = session.as_str()?;
    let read = bridge.call(
        id + 1,
        "cyber_session_read",
        json!({"session_id": session_id}),
    );
    let message = &read["result"]["structuredContent"]["recent_prompts"][0]["message_id"];
    message
        .is_string()
        .then(|| (session.clone(), message.clone()))
}

#[test]
fn a_wait_dropped_by_a_restart_fails_with_the_prompt_receipt() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let model = Model::start();
    env.write_global_config(&json!({
        "providers":{"test":{"api":{"type":"openai-compatible","url":model.url,
            "settings":{"auth":"none"}},"models":{"main":{}}}},
    }));
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &[]);
    bridge.initialize("Restart Probe");

    // No idempotency key and no progress token: only the receipt can name the prompt.
    bridge.send(
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"cyber_prompt","arguments":{"directory": repo.display().to_string(),
            "model": "test/main", "prompt": "slowly fix the failing test"}}}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut probe = 10;
    let (session, message) = loop {
        if let Some(receipt) = admitted_prompt(&mut bridge, probe) {
            break receipt;
        }
        probe += 2;
        assert!(Instant::now() < deadline, "the prompt was never admitted");
        std::thread::sleep(Duration::from_millis(100));
    };
    // The receipt event follows the durable admission within the same call.
    std::thread::sleep(Duration::from_millis(300));

    // A crash, not a graceful stop (which answers the wait `interrupted` first).
    kill(env.registration()["pid"].as_u64().unwrap());
    let down = loop {
        let frame = bridge.next_frame(Duration::from_secs(30));
        if frame["id"] == 2 {
            break frame;
        }
    };
    let data = &down["error"]["data"];
    assert_eq!(down["error"]["code"], -32010, "{down}");
    assert_eq!(data["reason"], "unreachable", "{down}");
    assert_eq!(data["session_id"], session, "{down}");
    assert_eq!(data["message_id"], message, "{down}");

    // The receipt re-attaches after the restart instead of admitting the prompt again.
    let started = env.cyber(&["service", "start"]);
    assert!(started.status.success(), "{}", stderr(&started));
    let again = bridge.call(
        3,
        "cyber_prompt_wait",
        json!({"session_id": session, "message_id": message, "wait_seconds": 0}),
    );
    assert_eq!(again["result"]["isError"], false, "{again}");
    assert_eq!(again["result"]["structuredContent"]["message_id"], message);
    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);
    drop(model);
}

/// `mcp-server` → Messaging over MCP through the real stdio bridge: list, send (origin
/// `client` with the bridge's label), replay, an oversized message and history.
#[test]
fn the_bridge_sends_and_reads_messages() {
    let env = Env::new();
    let repo = env.root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let model = Model::start();
    env.write_global_config(&json!({
        "providers":{"test":{"api":{"type":"openai-compatible","url":model.url,
            "settings":{"auth":"none"}},"models":{"main":{}}}},
    }));
    let _service = Service(&env);
    let mut bridge = Bridge::spawn(&env, &["--client", "claude-code"]);
    bridge.initialize("Claude Code");
    let created = bridge.call(
        2,
        "cyber_session_create",
        json!({"directory": repo.display().to_string(), "model": "test/main",
            "title": "migration"}),
    );
    let session = created["result"]["structuredContent"]["session"]["session_id"].clone();
    assert!(session.is_string(), "{created}");

    let listed = bridge.call(3, "cyber_message_list", json!({}));
    let sessions = &listed["result"]["structuredContent"]["sessions"];
    assert_eq!(sessions[0]["address"], "migration", "{listed}");
    assert_eq!(sessions[0]["canReceive"], true, "{listed}");

    let arguments =
        json!({"to": "migration", "text": "schema v2 is merged", "idempotency_key": "bridge-m1"});
    let sent = bridge.call(4, "cyber_message_send", arguments.clone());
    let sent = &sent["result"]["structuredContent"];
    assert_eq!(sent["outcome"], "delivered", "{sent}");
    let again = bridge.call(5, "cyber_message_send", arguments);
    let again = &again["result"]["structuredContent"];
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["message_id"], sent["message_id"]);
    let large = bridge.call(
        6,
        "cyber_message_send",
        json!({"to": "migration", "text": "x".repeat(20 * 1024)}),
    );
    assert_eq!(large["result"]["isError"], true, "{large}");
    let text = large["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MessageTooLargeError"), "{text}");

    let history = bridge.call(7, "cyber_message_history", json!({"session_id": session}));
    assert_one_client_message(&history, &sent["message_id"]);
    let (status, lines, err) = bridge.close();
    assert_eq!(status.code(), Some(0), "{err}");
    assert_jsonrpc_frames(&lines);

    // The message woke the Session, whose Turn answered it, as `cyber api` shows.
    let session = session.as_str().unwrap();
    wait_answered(&env, &repo, session, sent["message_id"].as_str().unwrap());
    let records = api(&env, &repo, "v1.session.messaging", session);
    assert_eq!(records["data"][0]["origin_kind"], "client", "{records}");
    drop(model);
}

/// Wait until the Session answered `message`.
fn wait_answered(env: &Env, repo: &std::path::Path, session: &str, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !answered_after(env, repo, session, message) {
        assert!(
            Instant::now() < deadline,
            "{}",
            api(env, repo, "v1.message.list", session)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A `cyber_message_history` reply holding only the bridge client's message `id`.
fn assert_one_client_message(history: &Value, id: &Value) {
    let messages = &history["result"]["structuredContent"]["messages"];
    assert_eq!(messages.as_array().map(Vec::len), Some(1), "{history}");
    assert_eq!(&messages[0]["message_id"], id);
    assert_eq!(messages[0]["from"], "mcp:claude-code");
    assert_eq!(messages[0]["text"], "schema v2 is merged");
}
