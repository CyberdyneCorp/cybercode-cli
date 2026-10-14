//! Runtime lifecycle, PermissionDenied and async hooks run through the real app
//! host: `cyber exec --embedded` against a fake provider.
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

fn send(stream: &mut TcpStream, delta: Value, finish: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":100,"completion_tokens":10}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The model's reply to one request body: a streamed delta and its finish reason.
trait Answer: Fn(&Value) -> (Value, &'static str) + Send + 'static {}
impl<F: Fn(&Value) -> (Value, &'static str) + Send + 'static> Answer for F {}

/// The Stop continuation gets "second answer", anything else "first answer".
fn stop_answers(body: &Value) -> (Value, &'static str) {
    let continued = body["messages"].to_string().contains("keep going");
    let text = if continued {
        "second answer"
    } else {
        "first answer"
    };
    (json!({"content":text}), "stop")
}

/// The conversation's first request reads `a.txt`; once a tool result is present
/// the model answers with plain text.
fn read_then_answer(body: &Value) -> (Value, &'static str) {
    let messages = &body["messages"];
    let called = messages
        .as_array()
        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
    if messages.to_string().contains("Write a short title") || called {
        return (json!({"content":"after the tool"}), "stop");
    }
    let call = json!({"tool_calls":[{"index":0,"id":"call_read","type":"function",
        "function":{"name":"read","arguments":json!({"path":"a.txt"}).to_string()}}]});
    (call, "tool_calls")
}

/// A fake OpenAI-compatible provider answering each request through an [`Answer`].
struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn start(answer: impl Answer) -> Self {
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
                let (delta, finish) = answer(&body);
                seen.lock().unwrap().push(body);
                send(&mut stream, delta, finish);
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
    exec_with(root, &[], &[])
}

/// The isolated `cyber` command every run in this file uses, rooted at `root`.
fn cyber(root: &Path, envs: &[(&str, &Path)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["--cwd", root.join("repo").to_str().unwrap()])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("CYBER_SANDBOX_HELPER", helper())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(envs.iter().copied())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs exec with extra flags and variables, killed after 60 s.
fn exec_with(root: &Path, flags: &[&str], envs: &[(&str, &Path)]) -> std::process::Output {
    let child = cyber(root, envs)
        .arg("exec")
        .args(["--embedded", "--model", "corp/main", "--format", "json"])
        .args(flags)
        .arg("hello")
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
    let provider = Provider::start(stop_answers);
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

/// Conversation requests only; the title request runs beside them.
fn conversation(provider: &Provider) -> Vec<Value> {
    provider
        .requests()
        .into_iter()
        .filter(|body| !body["messages"].to_string().contains("Write a short title"))
        .collect()
}

/// A fresh root whose global config routes `corp/main` to `provider` with `hooks`.
fn root_with(provider: &Provider, hooks: Value) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(root.join("repo/a.txt"), "file body").unwrap();
    std::fs::write(
        root.join("cyber/config/cyber.jsonc"),
        json!({
            "permissions":"allow",
            "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}},
            "hooks":hooks
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();
    (dir, root)
}

/// A global PostToolUse command hook for `read` that prints `decision`.
fn post_read(decision: Value) -> Value {
    json!({"PostToolUse":[{"matcher":"read","hooks":[{"type":"command","id":"guard","command":format!(
        "cat >/dev/null; printf '%s' '{decision}'"
    )}]}]})
}

fn explain(output: &std::process::Output, requests: &[Value]) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}\nmessages={:#?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        requests
            .iter()
            .map(|body| &body["messages"])
            .collect::<Vec<_>>(),
    )
}

#[test]
fn exec_admits_tool_hook_context_and_honors_continue_false_through_the_app_host() {
    let provider = Provider::start(read_then_answer);
    let (_dir, root) = root_with(
        &provider,
        post_read(json!({"additional_context":"TOOL CONTEXT MARKER"})),
    );
    let output = exec(&root);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert_eq!(requests.len(), 2, "{detail}");
    assert!(
        !requests[0]["messages"]
            .to_string()
            .contains("TOOL CONTEXT MARKER"),
        "{detail}"
    );
    let second = requests[1]["messages"].to_string();
    assert_eq!(
        second.matches("[hook guard] TOOL CONTEXT MARKER").count(),
        1,
        "{detail}"
    );
    assert!(output.status.success(), "{detail}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"], "after the tool", "{detail}");
    drop(provider);

    let provider = Provider::start(read_then_answer);
    let (_dir, root) = root_with(
        &provider,
        post_read(json!({"continue":false,"stop_reason":"policy stop"})),
    );
    let output = exec(&root);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert_eq!(
        requests.len(),
        1,
        "no request after the stopped Turn\n{detail}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("hook guard: policy stop"),
        "{detail}"
    );
    assert!(output.status.success(), "{detail}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["error"].is_null(), "{detail}");
}

/// The tool result contents of a request, in order.
fn tool_results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every durable hook receipt in the embedded exec database under `root`.
fn receipts(root: &Path) -> Vec<Value> {
    fn find(path: &Path) -> Option<PathBuf> {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path) {
                    return Some(found);
                }
            } else if path.extension().is_some_and(|extension| extension == "db")
                && path
                    .parent()
                    .is_some_and(|parent| parent.ends_with("embedded"))
            {
                return Some(path);
            }
        }
        None
    }
    let database = find(root).expect("exec database");
    let connection = rusqlite::Connection::open(database).unwrap();
    let mut statement = connection
        .prepare("SELECT data FROM hook_execution ORDER BY started_ms")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}

#[test]
fn exec_retries_a_denied_call_once_through_a_permission_denied_hook() {
    // The model reads `secret.txt`, which a deny rule refuses; the hook re-issues
    // the call for `a.txt`, which the rules allow.
    let provider = Provider::start(|body: &Value| {
        if body["messages"].to_string().contains("Write a short title")
            || !tool_results(body).is_empty()
        {
            return (json!({"content":"after the retry"}), "stop");
        }
        let call = json!({"tool_calls":[{"index":0,"id":"call_read","type":"function",
            "function":{"name":"read","arguments":json!({"path":"secret.txt"}).to_string()}}]});
        (call, "tool_calls")
    });
    let retry = json!({"decision":"retry","updated_input":{"path":"a.txt"}});
    let (_dir, root) = root_with(
        &provider,
        json!({"PermissionDenied":[{"matcher":"read","hooks":[{"type":"command","id":"retry","command":format!(
            "cat >/dev/null; printf x >> '{}'; printf '%s' '{retry}'",
            "../denied-count"
        )}]}]}),
    );
    std::fs::write(root.join("repo/secret.txt"), "SECRET BODY").unwrap();
    let mut config: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("cyber/config/cyber.jsonc")).unwrap(),
    )
    .unwrap();
    config["permissions"] = json!({"read":{"*":"allow","secret.txt":"deny"}});
    std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();

    let output = exec(&root);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert!(output.status.success(), "{detail}");
    assert_eq!(requests.len(), 2, "{detail}");
    let results = tool_results(&requests[1]);
    assert_eq!(results.len(), 1, "the tool ran once\n{detail}");
    assert!(results[0].contains("file body"), "{detail}");
    assert!(!requests[1].to_string().contains("SECRET BODY"), "{detail}");
    assert_eq!(
        std::fs::read_to_string(root.join("denied-count")).unwrap(),
        "x",
        "the hook ran exactly once\n{detail}"
    );
    let receipts = receipts(&root);
    assert_eq!(receipts.len(), 1, "{receipts:#?}");
    assert_eq!(receipts[0]["event"], "PermissionDenied");
    assert_eq!(receipts[0]["status"], "completed");
}

#[test]
fn exec_runs_pre_tool_use_guards_on_a_permission_denied_retry_with_new_input() {
    // A deny rule refuses `secret.txt`; the PermissionDenied hook re-issues the call
    // for `guarded.txt`, which the rules allow but a managed PreToolUse guard denies.
    // The guard must see the retried input, so nothing from either file is read.
    let provider = Provider::start(|body: &Value| {
        if body["messages"].to_string().contains("Write a short title")
            || !tool_results(body).is_empty()
        {
            return (json!({"content":"after the retry"}), "stop");
        }
        let call = json!({"tool_calls":[{"index":0,"id":"call_read","type":"function",
            "function":{"name":"read","arguments":json!({"path":"secret.txt"}).to_string()}}]});
        (call, "tool_calls")
    });
    let retry = json!({"decision":"retry","updated_input":{"path":"guarded.txt"}});
    let (_dir, root) = root_with(
        &provider,
        json!({"PermissionDenied":[{"matcher":"read","hooks":[{"type":"command","id":"retry","command":format!(
            "cat >/dev/null; printf '%s' '{retry}'"
        )}]}]}),
    );
    std::fs::write(root.join("repo/secret.txt"), "SECRET BODY").unwrap();
    std::fs::write(root.join("repo/guarded.txt"), "GUARDED BODY").unwrap();
    let path = root.join("cyber/config/cyber.jsonc");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["permissions"] = json!({"read":{"*":"allow","secret.txt":"deny"}});
    std::fs::write(&path, config.to_string()).unwrap();
    let guard_runs = root.join("guard-runs");
    let deny = json!({"decision":"deny","reason":"managed guard"});
    std::fs::create_dir_all(root.join("policy")).unwrap();
    std::fs::write(
        root.join("policy/cyber-policy.jsonc"),
        json!({"hooks":{"PreToolUse":[{"matcher":"read","hooks":[{"type":"command","id":"guard","command":format!(
            "input=$(cat); printf x >> '{}'; case \"$input\" in *guarded.txt*) printf '%s' '{deny}';; esac",
            guard_runs.display()
        )}]}]}})
        .to_string(),
    )
    .unwrap();
    let policy = root.join("policy");

    let output = exec_with(&root, &[], &[("CYBER_MANAGED_POLICY_DIR", &policy)]);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert!(output.status.success(), "{detail}");
    assert_eq!(requests.len(), 2, "{detail}");
    let results = tool_results(&requests[1]);
    assert_eq!(results.len(), 1, "{detail}");
    assert!(
        results[0].contains("blocked by hook guard: managed guard"),
        "{detail}"
    );
    let sent = requests[1].to_string();
    assert!(
        !sent.contains("GUARDED BODY") && !sent.contains("SECRET BODY"),
        "{detail}"
    );
    assert_eq!(runs(&guard_runs), 2, "the guard saw both inputs\n{detail}");
}

/// A local MCP server whose `judge` tool appends its first argument to `calls` and
/// allows the event.
const GUARD_SERVER: &str = r#"
import json,sys
mode,calls=sys.argv[1],sys.argv[2]
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    if request['method']=='initialize': result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'guard','version':'1'}}
    elif request['method']=='tools/list': result={'tools':[{'name':'judge','inputSchema':{'type':'object'}}]}
    elif request['method']=='tools/call':
        open(calls,'a').write(mode)
        result={'content':[{'type':'text','text':'{"decision":"allow"}'}]}
    else: result={}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#;

#[test]
fn exec_fails_a_managed_mcp_guard_closed_when_another_layer_shadows_its_server() {
    let provider = Provider::start(read_then_answer);
    let (_dir, root) = root_with(&provider, json!({}));
    std::fs::write(root.join("guard.py"), GUARD_SERVER).unwrap();
    let calls = root.join("guard-calls");
    let server = |mode: &str| {
        json!({"type":"local","command":"/usr/bin/python3",
            "args":["-u",root.join("guard.py"),mode,calls]})
    };
    let path = root.join("cyber/config/cyber.jsonc");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"] = json!({"guard":server("G")});
    config["sandbox"] = json!({"policy":"full-access"});
    std::fs::write(&path, config.to_string()).unwrap();
    std::fs::create_dir_all(root.join("policy")).unwrap();
    std::fs::write(
        root.join("policy/cyber-policy.jsonc"),
        json!({"hooks":{"PreToolUse":[{"matcher":"read","hooks":[{"type":"mcp_tool",
            "id":"guard","server":"guard","tool":"judge","fail_closed":true}]}]}})
        .to_string(),
    )
    .unwrap();
    let policy = root.join("policy");

    // The global server serves the managed guard and the read runs.
    let output = exec_with(&root, &[], &[("CYBER_MANAGED_POLICY_DIR", &policy)]);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert!(output.status.success(), "{detail}");
    assert!(
        tool_results(&requests[1])[0].contains("file body"),
        "{detail}"
    );
    assert_eq!(std::fs::read_to_string(&calls).unwrap(), "G", "{detail}");

    // An invocation layer that redirects the server is never called by the managed
    // guard, which fails closed and refuses the read.
    let shadow =
        json!({"mcp":{"guard":{"args":["-u",root.join("guard.py"),"S",calls]}}}).to_string();
    let envs = [
        ("CYBER_MANAGED_POLICY_DIR", policy.as_path()),
        ("CYBER_CONFIG_CONTENT", Path::new(&shadow)),
    ];
    let output = exec_with(&root, &[], &envs);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert!(output.status.success(), "{detail}");
    let last = requests.last().unwrap();
    let results = tool_results(last);
    assert_eq!(results.len(), 1, "{detail}");
    assert!(!results[0].contains("file body"), "{detail}");
    assert!(results[0].contains("blocked by hook guard"), "{detail}");
    assert_eq!(std::fs::read_to_string(&calls).unwrap(), "G", "{detail}");
}

/// A local policy server that holds each hook request until `release` is set.
struct HeldHook {
    url: String,
    arrived: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    replied: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl HeldHook {
    fn start(reply: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let arrived = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let replied = Arc::new(AtomicBool::new(false));
        let (seen, gate, done) = (arrived.clone(), release.clone(), replied.clone());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let mut accepted = None;
            for _ in 0..6000 {
                match listener.accept() {
                    Ok((stream, _)) => {
                        accepted = Some(stream);
                        break;
                    }
                    Err(_) if gate.load(Ordering::SeqCst) => return,
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
            let Some(mut stream) = accepted else {
                return;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let _ = read_body(&mut stream);
            seen.store(true, Ordering::SeqCst);
            for _ in 0..3000 {
                if gate.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let body = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
            done.store(true, Ordering::SeqCst);
        });
        Self {
            url,
            arrived,
            release,
            replied,
            server: Some(server),
        }
    }
}

impl Drop for HeldHook {
    fn drop(&mut self) {
        self.release.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

#[test]
fn exec_runs_async_hooks_in_background_and_admits_context_at_next_boundary() {
    let hook = HeldHook::start(
        json!({"decision":"deny","reason":"ignored","additional_context":"ASYNC CONTEXT MARKER"}),
    );
    let (arrived, release, replied) = (
        hook.arrived.clone(),
        hook.release.clone(),
        hook.replied.clone(),
    );
    // (request index, whether the hook had replied when it arrived)
    let order = Arc::new(Mutex::new(Vec::<(usize, bool)>::new()));
    let seen = order.clone();
    let provider = Provider::start(move |body: &Value| {
        if body["messages"].to_string().contains("Write a short title") {
            return (json!({"content":"title"}), "stop");
        }
        let text = body["messages"].to_string();
        let mut order = seen.lock().unwrap();
        let index = order.len();
        order.push((index, replied.load(Ordering::SeqCst)));
        if text.contains("ASYNC CONTEXT MARKER") {
            return (json!({"content":"saw the context"}), "stop");
        }
        let results = tool_results(body).len();
        if results > 0 {
            // The tool settled and this request proceeds while the hook is held.
            release.store(true, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(100));
        }
        if results >= 20 {
            return (json!({"content":"context never arrived"}), "stop");
        }
        let call = json!({"tool_calls":[{"index":0,"id":format!("call_{results}"),"type":"function",
            "function":{"name":"read","arguments":json!({"path":"a.txt"}).to_string()}}]});
        (call, "tool_calls")
    });
    let (_dir, root) = root_with(
        &provider,
        json!({"PostToolUse":[{"matcher":"read","hooks":[{"type":"http","id":"async","async":true,"url":hook.url}]}]}),
    );
    let output = exec(&root);
    let requests = conversation(&provider);
    let detail = explain(&output, &requests);
    assert!(output.status.success(), "{detail}");
    assert!(arrived.load(Ordering::SeqCst), "{detail}");
    let order = order.lock().unwrap().clone();
    assert!(order.len() >= 3, "{order:?}\n{detail}");
    assert!(!order[1].1, "request 2 waited for the async hook\n{detail}");
    // The deny was ignored: the first call settled with the file body.
    assert!(
        tool_results(&requests[1])[0].contains("file body"),
        "{detail}"
    );
    let last = requests.last().unwrap().to_string();
    assert_eq!(
        last.matches("[hook async] ASYNC CONTEXT MARKER").count(),
        1,
        "{detail}"
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"], "saw the context", "{detail}");
    let receipts = receipts(&root);
    assert!(!receipts.is_empty());
    let first = &receipts[0];
    assert_eq!(first["event"], "PostToolUse");
    assert_eq!(first["status"], "completed", "{receipts:#?}");
    assert_eq!(first["decision"]["decision"], "deny");
    drop(hook);
}

/// A PreToolUse command hook for `read` that appends to `marker` each time it runs.
fn marking(id: &str, marker: &Path) -> Value {
    json!({"matcher":"read","hooks":[{"type":"command","id":id,"command":format!(
        "cat >/dev/null; printf x >> '{}'", marker.display()
    )}]})
}

fn runs(marker: &Path) -> usize {
    std::fs::read_to_string(marker).map_or(0, |text| text.len())
}

/// Runs a short `cyber` subcommand and returns its JSON output.
fn cyber_json(root: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Value {
    let output = cyber(root, envs)
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Runs a `cyber` subcommand that must succeed.
fn cyber_ok(root: &Path, args: &[&str]) {
    let output = cyber(root, &[]).args(args).output().unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

/// Markers of the global, project and managed PreToolUse hooks of one root.
struct Markers {
    global: PathBuf,
    project: PathBuf,
    managed: PathBuf,
}

/// Global and project hooks with an approved checkout configuration, and a
/// managed-only policy directory holding a managed hook.
fn managed_root(provider: &Provider) -> (tempfile::TempDir, PathBuf, Markers) {
    let (dir, root) = root_with(provider, json!({}));
    let markers = Markers {
        global: root.join("global-ran"),
        project: root.join("repo/project-ran"),
        managed: root.join("managed-ran"),
    };
    let path = root.join("cyber/config/cyber.jsonc");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["hooks"] = json!({"PreToolUse":[marking("global", &markers.global)]});
    std::fs::write(&path, config.to_string()).unwrap();
    std::fs::write(
        root.join("repo/cyber.jsonc"),
        json!({"hooks":{"PreToolUse":[marking("project", &markers.project)]},"sandbox":{"network":"off"}})
            .to_string(),
    )
    .unwrap();
    let inspected = cyber_json(&root, &["trust", "inspect"], &[]);
    cyber_ok(
        &root,
        &[
            "trust",
            "approve",
            "--digest",
            inspected["digest"].as_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(root.join("policy")).unwrap();
    std::fs::write(
        root.join("policy/cyber-policy.jsonc"),
        json!({
            "hooks":{"PreToolUse":[marking("managed", &markers.managed)]},
            "policy":{"hooks":{"managed_only":true}}
        })
        .to_string(),
    )
    .unwrap();
    (dir, root, markers)
}

/// `cyber hooks list` shows the managed hook first and the others as skipped;
/// returns the project hook digest.
fn assert_listed_managed_only(root: &Path, envs: &[(&str, &Path)]) -> String {
    let listed = cyber_json(root, &["hooks", "list"], envs);
    let scopes: Vec<_> = listed["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hook| (hook["scope"].clone(), hook["skipped"].clone()))
        .collect();
    assert_eq!(
        scopes,
        [
            (json!("managed"), Value::Null),
            (json!("global"), json!("managed-only policy")),
            (json!("project"), json!("managed-only policy")),
        ],
        "{listed:#}"
    );
    assert_eq!(listed["managed_policy"]["managed_only"], true);
    listed["hooks"][2]["digest"].as_str().unwrap().to_owned()
}

#[test]
fn exec_applies_managed_only_policy_and_invocation_trust() {
    let provider = Provider::start(read_then_answer);
    let (_dir, root, markers) = managed_root(&provider);
    let policy = root.join("policy");
    let managed_env = [("CYBER_MANAGED_POLICY_DIR", policy.as_path())];
    let project_digest = assert_listed_managed_only(&root, &managed_env);
    // Even an approved project hook is skipped under the policy.
    cyber_ok(&root, &["hooks", "trust", "--digest", &project_digest]);

    let output = exec_with(&root, &[], &managed_env);
    let detail = explain(&output, &conversation(&provider));
    assert!(output.status.success(), "{detail}");
    let ran = [&markers.managed, &markers.global, &markers.project].map(|marker| runs(marker));
    assert_eq!(ran, [1, 0, 0], "{detail}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("skipped by managed-only policy").count(),
        1,
        "{detail}"
    );
    assert!(
        stderr.contains("hook managed-policy: 2 hooks skipped by managed-only policy"),
        "{detail}"
    );
    let skipped = receipts(&root)
        .into_iter()
        .filter(|receipt| receipt["outcome"] == "skipped")
        .count();
    assert_eq!(skipped, 2, "{detail}");

    // Without the policy an unapproved project hook is skipped unless this run
    // approves it, and that approval is never kept.
    cyber_ok(&root, &["hooks", "untrust", "--digest", &project_digest]);
    for (flags, expected) in [
        (&[][..], 0),
        (&["--trust-project-hooks"][..], 1),
        (&[][..], 1),
    ] {
        let output = exec_with(&root, flags, &[]);
        let detail = explain(&output, &conversation(&provider));
        assert!(output.status.success(), "{detail}");
        assert_eq!(runs(&markers.project), expected, "{flags:?}\n{detail}");
        let noticed = String::from_utf8_lossy(&output.stderr)
            .contains("hook trust: 1 untrusted hook changed \u{2014} review with /hooks");
        assert_eq!(noticed, flags.is_empty(), "{flags:?}\n{detail}");
    }
    assert_eq!(runs(&markers.global), 3);
    let refused = cyber(&root, &[])
        .args(["exec", "--trust-project-hooks", "hello"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
}
