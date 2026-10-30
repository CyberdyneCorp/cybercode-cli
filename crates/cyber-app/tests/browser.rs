//! Browser verification through the production App (`browser-verification` → Supported
//! optional browser integration, Verification artifacts and lifecycle). Sessions run over
//! the App's HTTP router against a scripted local model; the browser is a fake browser MCP
//! server (`support/browser_mcp.py`) speaking the pinned server's tool names, and dev
//! servers are real `python3 -m http.server` processes started as owned `bash` Jobs.
//!
//! The fixtures run `python3` and POSIX shells, so these cases are Unix-only.
#![cfg(unix)]

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const PASSWORD: &str = "browser-password";

struct Fixture {
    _temp: tempfile::TempDir,
    work: PathBuf,
    paths: Paths,
    log: PathBuf,
    app: App,
    base: String,
    http: reqwest::Client,
    model: scripted_model::ScriptedModel,
}

/// The fake browser server's definition, logging to `log`.
fn browser_server(log: &Path) -> Value {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/browser_mcp.py");
    json!({"type": "local", "command": "python3", "args": ["-u", fixture, log]})
}

impl Fixture {
    /// The App over HTTP. `global` is merged into the global configuration; with
    /// `browser`, it also enables the fake browser there.
    async fn new(global: Value, browser: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let log = root.join("browser.jsonl");
        let model = scripted_model::ScriptedModel::start(reply);
        let mut config = model.config();
        config["sandbox"] = json!({"policy": "full-access"});
        config["permissions"] = json!({"*": "allow", "network": "allow"});
        if browser {
            config["browser"] = json!({"enabled": true});
            config["mcp"] = json!({"browser": browser_server(&log)});
        }
        for (key, value) in global.as_object().unwrap() {
            config[key] = value.clone();
        }
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let work = root.join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        std::fs::create_dir_all(work.join("site")).unwrap();
        std::fs::write(
            work.join("site/index.html"),
            r#"<title>Shop</title><button data-console="cart.js failed: client_secret=abc123">Buy</button>"#,
        )
        .unwrap();
        let serve = work.join("serve.sh");
        std::fs::write(
            &serve,
            // Not `python3 -m http.server`: its bind resolves the host name
            // (getfqdn), which hangs past the readiness timeout with some Pythons
            // (Homebrew 3.14+ on macOS).
            "#!/bin/sh\nexec python3 -c '\nimport functools, http.server, socketserver, sys\nclass Server(socketserver.ThreadingTCPServer):\n    allow_reuse_address = True\n    daemon_threads = True\nhandler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[2])\nServer((\"127.0.0.1\", int(sys.argv[1])), handler).serve_forever()\n' \"$1\" \"$(dirname \"$0\")/site\"\n",
        )
        .unwrap();
        let mut mode = std::fs::metadata(&serve).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&serve, mode).unwrap();
        let app = App::build(AppOptions {
            paths: paths.clone(),
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: work.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        Self {
            _temp: temp,
            work,
            paths,
            log,
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            model,
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", self.work.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        response.json().await.unwrap_or(Value::Null)
    }

    async fn session(&self) -> String {
        let created = self
            .send(
                reqwest::Method::POST,
                "/sessions",
                Some(json!({"model": "test/main"})),
            )
            .await;
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    /// Run `steps` (tool calls, one per model request) as one Turn, then `Done.`.
    async fn run(&self, id: &str, steps: Value) {
        let before = self.model.requests().len();
        let text = format!("SCRIPT:{steps}");
        self.send(
            reqwest::Method::POST,
            &format!("/sessions/{id}/prompt"),
            Some(json!({"parts": [{"type": "text", "text": text}]})),
        )
        .await;
        for _ in 0..3000 {
            let answered = self.model.requests().len() > before;
            if answered && !self.app.runtime.is_running(id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pending = self
            .send(reqwest::Method::GET, "/permissions/requests", None)
            .await;
        let messages = self
            .send(
                reqwest::Method::GET,
                &format!("/sessions/{id}/messages?order=asc&limit=100"),
                None,
            )
            .await;
        panic!("{id} did not finish (pending {pending}): {messages}");
    }

    /// Every tool result the model received in this Session, oldest first.
    fn results(&self) -> Vec<String> {
        chats(&self.model)
            .last()
            .map(tool_results)
            .unwrap_or_default()
    }

    async fn evidence(&self, id: &str) -> Value {
        self.send(
            reqwest::Method::GET,
            &format!("/sessions/{id}/browser/evidence"),
            None,
        )
        .await["data"]
            .clone()
    }

    /// The fake browser's log.
    fn browser_log(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

/// Answers `SCRIPT:[[name, arguments], …]`: one call per request, then `Done.`.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({"content": "Browser"});
    }
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let Some(script) = messages.iter().rev().find_map(|message| {
        let text = message_text(message);
        let start = text.find("SCRIPT:")?;
        serde_json::from_str::<Vec<Value>>(&text[start + 7..]).ok()
    }) else {
        return json!({"content": "Done."});
    };
    let done = messages
        .iter()
        .rev()
        .take_while(|message| !message_text(message).contains("SCRIPT:"))
        .filter(|message| message["role"] == "tool")
        .count();
    match script.get(done) {
        Some(step) => json!({"tool_calls": [{"index": 0, "id": format!("call_{}", messages.len()),
            "type": "function", "function": {"name": step[0], "arguments": step[1].to_string()}}]}),
        None => json!({"content": "Done."}),
    }
}

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn chats(model: &scripted_model::ScriptedModel) -> Vec<Value> {
    model
        .requests()
        .into_iter()
        .filter(|body| !body.to_string().contains("Write a short title"))
        .collect()
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

fn tool_results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .map(message_text)
        .collect()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A `bash` call starting the fixture site (`serve.sh`) as a dev server on `port`.
fn dev_server(port: u16) -> Value {
    json!(["bash", {"command": format!("./serve.sh {port}"), "background": true,
        "ready_url": format!("http://127.0.0.1:{port}/"), "ready_timeout_seconds": 30}])
}

fn sha256_of(path: &str) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

async fn reachable(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_ok()
}

#[tokio::test]
async fn dev_server_exiting_before_readiness_records_startup_failure_with_logs() {
    let f = Fixture::new(json!({}), false).await;
    let id = f.session().await;
    let port = free_port();
    let command =
        "echo 'vite starting'; echo 'Error: listen EADDRINUSE client_secret=hunter2' >&2; exit 3";
    let steps = json!([["bash", {"command": command, "background": true,
        "ready_url": format!("http://127.0.0.1:{port}/"), "ready_timeout_seconds": 30}]]);
    f.run(&id, steps).await;

    let result = &f.results()[0];
    assert!(result.contains("Dev server startup failed"), "{result}");
    assert!(result.contains("exited with code 3"), "{result}");
    assert!(result.contains("was not tested"), "{result}");
    assert!(result.contains("Error: listen EADDRINUSE"), "{result}");
    assert!(
        !result.contains("hunter2"),
        "the log tail is redacted: {result}"
    );

    let report = f.evidence(&id).await;
    let records = report["evidence"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{report}");
    let record = &records[0];
    assert_eq!(record["status"], "startup_failed");
    assert_eq!(record["session_id"], id);
    assert_eq!(record["url"], format!("http://127.0.0.1:{port}/"));
    assert!(
        record["observed"][0]
            .as_str()
            .unwrap()
            .contains("exited with code 3")
    );
    assert!(
        record["not_run"][0]
            .as_str()
            .unwrap()
            .contains("not tested")
    );
    assert!(
        record["workspace_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let log = &record["artifacts"][0];
    assert_eq!(log["kind"], "log");
    let path = log["path"].as_str().unwrap();
    assert_eq!(log["sha256"], sha256_of(path));
    let kept = std::fs::read_to_string(path).unwrap();
    assert!(
        kept.contains("EADDRINUSE") && !kept.contains("hunter2"),
        "{kept}"
    );
    // The Job is the failed process, not left running.
    let jobs = f.send(reqwest::Method::GET, "/jobs", None).await;
    assert!(jobs.to_string().contains("\"exit_code\":3"), "{jobs}");

    // A server that never answers is stopped when its readiness check times out.
    let port = free_port();
    let steps = json!([["bash", {"command": "echo waiting; sleep 30", "background": true,
        "ready_url": format!("http://127.0.0.1:{port}/"), "ready_timeout_seconds": 1}]]);
    f.run(&id, steps).await;
    let result = f.results().last().unwrap().clone();
    assert!(result.contains("timed out after 1 s"), "{result}");
    assert!(result.contains("was not tested"), "{result}");
    let report = f.evidence(&id).await;
    let timeout = &report["evidence"][1];
    assert_eq!(timeout["status"], "startup_failed", "{report}");
    assert!(
        timeout["observed"][0]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    let jobs = f.send(reqwest::Method::GET, "/jobs", None).await;
    let stopped = jobs["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["command"] == "echo waiting; sleep 30")
        .unwrap();
    assert_eq!(stopped["status"], "cancelled", "{stopped}");
    f.app.runtime.shutdown().await;
}

/// The Job of `command`, once it exists.
async fn job_of(f: &Fixture, command: &str) -> Option<Value> {
    let jobs = f.send(reqwest::Method::GET, "/jobs", None).await;
    jobs["data"]
        .as_array()?
        .iter()
        .find(|job| job["command"] == command)
        .cloned()
}

/// Regression: a call cancelled while its readiness check was pending returned
/// without stopping the dev server Job, which nobody owned and kept running.
#[tokio::test]
async fn cancelling_a_pending_readiness_check_stops_the_dev_server() {
    let f = Fixture::new(json!({}), false).await;
    let id = f.session().await;
    let port = free_port();
    let command = "echo waiting; sleep 30";
    let steps = json!([["bash", {"command": command, "background": true,
        "ready_url": format!("http://127.0.0.1:{port}/"), "ready_timeout_seconds": 30}]]);
    f.send(
        reqwest::Method::POST,
        &format!("/sessions/{id}/prompt"),
        Some(json!({"parts": [{"type": "text", "text": format!("SCRIPT:{steps}")}]})),
    )
    .await;
    let mut running = false;
    for _ in 0..1000 {
        if job_of(&f, command)
            .await
            .is_some_and(|job| job["status"] == "running")
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running, "the dev server Job never started");
    f.send(
        reqwest::Method::POST,
        &format!("/sessions/{id}/interrupt"),
        None,
    )
    .await;
    let mut status = Value::Null;
    for _ in 0..500 {
        status = job_of(&f, command).await.unwrap()["status"].clone();
        if status != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        status, "cancelled",
        "the interrupted call left its dev server running"
    );
    f.app.runtime.shutdown().await;
}

/// Regression: the readiness probe followed redirects from the core process, outside
/// the sandbox, so a dev server could make it fetch any host. A redirect answer counts
/// as ready and its target is never contacted.
#[tokio::test]
async fn readiness_probe_does_not_follow_redirects() {
    let f = Fixture::new(json!({}), false).await;
    let id = f.session().await;
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_port = target.local_addr().unwrap().port();
    let contacted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = contacted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = target.accept().await {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio::io::AsyncWriteExt::write_all(
                &mut stream,
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        }
    });
    let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = redirect.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = redirect.accept().await {
            let mut buffer = [0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await;
            let answer = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/?secret=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, answer.as_bytes()).await;
        }
    });
    let steps = json!([["bash", {"command": "echo up; sleep 30", "background": true,
        "ready_url": format!("http://127.0.0.1:{port}/"), "ready_timeout_seconds": 10}]]);
    f.run(&id, steps).await;
    let result = &f.results()[0];
    assert!(result.contains("\"ready_url\""), "{result}");
    assert!(
        !contacted.load(std::sync::atomic::Ordering::SeqCst),
        "the probe followed the redirect"
    );
    f.app.runtime.shutdown().await;
}

/// Regression: every tool the browser server published was offered, so
/// `browser_evaluate` (`location.href = …`) or `browser_file_upload` reached hosts and
/// files the per-navigation network check and `deny_read` keep out. Only vetted tools
/// are offered, and an unvetted one never reaches the server.
/// Regression: below the default sandbox (network `proxy`) a dev server could never get
/// ready: macOS denied it listening (`Operation not permitted`) and Linux ran it in its
/// own network namespace, so the call started the command and then failed or timed out.
/// It is refused before anything starts now, and with `sandbox.network: on` the sandboxed
/// dev server becomes ready.
#[tokio::test]
async fn a_sandboxed_dev_server_needs_the_sandbox_network_on() {
    let sandbox =
        |network: &str| json!({"sandbox": {"policy": "workspace-write", "network": network}});
    let f = Fixture::new(sandbox("proxy"), false).await;
    let id = f.session().await;
    let port = free_port();
    f.run(&id, json!([dev_server(port)])).await;
    let result = &f.results()[0];
    assert!(
        result.contains("ready_url is not available for this sandboxed command")
            && result.contains("sandbox.network"),
        "{result}"
    );
    let command = format!("./serve.sh {port}");
    assert!(job_of(&f, &command).await.is_none(), "nothing started");
    f.app.runtime.shutdown().await;

    let f = Fixture::new(sandbox("on"), false).await;
    let id = f.session().await;
    f.run(&id, json!([dev_server(port)])).await;
    let result = &f.results()[0];
    assert!(result.contains("The dev server is ready at"), "{result}");
    assert!(reachable(port).await);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn unvetted_browser_tools_are_neither_offered_nor_called() {
    let f = Fixture::new(json!({}), true).await;
    let id = f.session().await;
    let evaluate = json!({"function": "() => { location.href = 'https://denied.example/' }"});
    f.run(
        &id,
        json!([
            ["mcp__browser__browser_snapshot", {}],
            ["mcp__browser__browser_evaluate", evaluate],
            ["mcp__browser__browser_file_upload", {"paths": ["/etc/hosts"]}]
        ]),
    )
    .await;
    let requests = chats(&f.model);
    let offered = tool_names(requests.last().unwrap());
    assert!(
        offered
            .iter()
            .any(|name| name == "mcp__browser__browser_snapshot"),
        "{offered:?}"
    );
    for refused in [
        "browser_evaluate",
        "browser_file_upload",
        "browser_tabs",
        "browser_navigate_back",
    ] {
        assert!(
            !offered.iter().any(|name| name.ends_with(refused)),
            "{refused} offered: {offered:?}"
        );
    }
    let called: Vec<Value> = f
        .browser_log()
        .into_iter()
        .filter(|event| event["event"] == "call")
        .map(|event| event["name"].clone())
        .collect();
    assert_eq!(called, [json!("browser_snapshot")], "{:?}", f.results());
    let results = f.results();
    // Scenario "Browser page framed as browser": the page reaches the model as data.
    assert!(
        results[0].starts_with(
            "<untrusted_content source=\"browser\" origin=\"mcp:browser/browser_snapshot\">\n"
        ) && results[0].ends_with("\n</untrusted_content>"),
        "{results:?}"
    );
    assert!(results[1].contains("Unknown tool"), "{results:?}");
    assert!(results[2].contains("Unknown tool"), "{results:?}");
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn session_close_stops_owned_dev_server_and_browser() {
    let f = Fixture::new(json!({}), true).await;
    let first = f.session().await;
    let second = f.session().await;
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}/");
    f.run(
        &first,
        json!([dev_server(port), ["mcp__browser__browser_navigate", {"url": url}]]),
    )
    .await;
    let results = f.results();
    assert!(results[0].contains("\"ready_url\""), "{results:?}");
    assert!(results[1].contains("Page Title: Shop"), "{results:?}");
    assert!(reachable(port).await);
    f.run(&second, json!([["mcp__browser__browser_snapshot", {}]]))
        .await;

    // One browser process per Session, each with its own fresh profile.
    let starts: Vec<Value> = f
        .browser_log()
        .into_iter()
        .filter(|event| event["event"] == "start")
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?} {:?}", f.results());
    let profiles = f.paths.data.join("browser/profiles");
    let profile = |session: &str| profiles.join(session);
    let started = |session: &str| {
        starts
            .iter()
            .find(|start| {
                Path::new(start["tmpdir"].as_str().unwrap()).starts_with(profile(session))
            })
            .unwrap_or_else(|| panic!("no browser for {session}: {starts:?}"))
            .clone()
    };
    let (first_start, second_start) = (started(&first), started(&second));
    assert_ne!(first_start["pid"], second_start["pid"]);
    assert!(profile(&first).exists());

    f.send(
        reqwest::Method::PATCH,
        &format!("/sessions/{first}"),
        Some(json!({"archived": true})),
    )
    .await;
    let first_pid = first_start["pid"].clone();
    let mut stopped = false;
    for _ in 0..500 {
        let exited = f
            .browser_log()
            .iter()
            .any(|event| event["event"] == "exit" && event["pid"] == first_pid);
        if exited && !reachable(port).await && !profile(&first).exists() {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        stopped,
        "archiving the Session stops its browser, profile and dev server: {:?} port {} profile {}",
        f.browser_log(),
        reachable(port).await,
        profile(&first).exists()
    );
    let jobs = f.send(reqwest::Method::GET, "/jobs", None).await;
    let job = jobs["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["session_id"] == first)
        .unwrap();
    assert_eq!(job["status"], "cancelled", "{job}");
    // Regression: the stopped dev server's completion notice started a Turn in the
    // archived Session, which started a new browser for it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let restarted = f.browser_log().iter().any(|event| {
        event["event"] == "start"
            && event["pid"] != first_pid
            && Path::new(event["tmpdir"].as_str().unwrap()).starts_with(profile(&first))
    });
    assert!(!restarted, "{:?}", f.browser_log());
    assert!(!f.app.runtime.is_running(&first));
    // The other Session's browser keeps running.
    assert!(profile(&second).exists());
    assert!(
        !f.browser_log()
            .iter()
            .any(|event| event["event"] == "exit" && event["pid"] == second_start["pid"])
    );
    // Deleting a Session stops its browser too.
    f.send(
        reqwest::Method::DELETE,
        &format!("/sessions/{second}"),
        None,
    )
    .await;
    assert!(!profile(&second).exists());
    assert!(
        f.browser_log()
            .iter()
            .any(|event| event["event"] == "exit" && event["pid"] == second_start["pid"])
    );
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn evidence_records_hashes_and_marks_stale_after_edit() {
    let f = Fixture::new(
        json!({"permissions": {"*": "allow", "network": {"*": "allow", "example.invalid": "deny"},
            "mcp__browser__browser_click": "deny"}}),
        true,
    )
    .await;
    let id = f.session().await;
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}/");
    f.run(
        &id,
        json!([
            dev_server(port),
            ["mcp__browser__browser_navigate", {"url": "http://example.invalid/"}],
            ["mcp__browser__browser_navigate", {"url": url}],
            ["mcp__browser__browser_resize", {"width": 1280, "height": 720}],
            ["mcp__browser__browser_take_screenshot", {}],
            ["mcp__browser__browser_console_messages", {}],
            ["mcp__browser__browser_type", {"element": "Search", "ref": "e2", "text": "shoes"}],
            ["mcp__browser__browser_network_requests", {}],
        ]),
    )
    .await;
    let requests = chats(&f.model);
    let tools = tool_names(&requests[0]);
    assert!(
        tools.contains(&"mcp__browser__browser_navigate".to_string()),
        "{tools:?}"
    );
    assert!(
        !tools.contains(&"mcp__browser__browser_click".to_string()),
        "a denied browser tool is not offered: {tools:?}"
    );
    let results = f.results();
    assert!(
        results[1].contains("Network access to example.invalid was not allowed"),
        "{results:?}"
    );
    assert!(results[4].contains("[screenshot saved to "), "{results:?}");
    let navigations: Vec<Value> = f
        .browser_log()
        .into_iter()
        .filter(|event| event["name"] == "browser_navigate")
        .collect();
    assert_eq!(
        navigations.len(),
        1,
        "the denied navigation never reached the browser: {results:?}"
    );

    let report = f.evidence(&id).await;
    let hash = report["workspace_hash"].as_str().unwrap().to_owned();
    assert!(hash.starts_with("sha256:"));
    let records = report["evidence"].as_array().unwrap().clone();
    assert_eq!(records.len(), 3, "{report}");
    let (shot, console, network) = (&records[0], &records[1], &records[2]);
    for record in [shot, console, network] {
        assert_eq!(record["status"], "observed");
        assert_eq!(record["session_id"], id);
        assert_eq!(record["url"], url);
        assert_eq!(record["viewport"], json!({"width": 1280, "height": 720}));
        assert_eq!(record["workspace_hash"], hash);
        assert_eq!(record["stale"], false);
        assert!(record["time_ms"].as_i64().unwrap() > 0);
        for artifact in record["artifacts"].as_array().unwrap() {
            assert_eq!(
                artifact["sha256"],
                sha256_of(artifact["path"].as_str().unwrap())
            );
        }
    }
    let actions: Vec<&str> = shot["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            format!("navigate {url}").as_str(),
            "resize 1280x720",
            "take screenshot"
        ]
    );
    assert_eq!(shot["artifacts"][0]["kind"], "screenshot");
    assert_eq!(shot["artifacts"][0]["media_type"], "image/png");
    assert_eq!(shot["observed"][0], format!("loaded {url}"));
    // Observed behavior and checks not run are kept apart.
    assert_eq!(
        shot["not_run"],
        json!([
            "console errors not inspected",
            "failed network requests not inspected"
        ])
    );
    assert_eq!(console["observed"], json!(["console: 1 error message(s)"]));
    assert_eq!(
        console["not_run"],
        json!(["failed network requests not inspected"])
    );
    // Input is part of the action summary; every check has run by the last record.
    assert_eq!(
        network["actions"],
        json!(["type into Search", "network requests"])
    );
    assert_eq!(network["observed"], json!(["network: 0 failed request(s)"]));
    assert_eq!(network["not_run"], json!([]));
    let captured =
        std::fs::read_to_string(console["artifacts"][0]["path"].as_str().unwrap()).unwrap();
    assert!(
        captured.contains("cart.js failed") && !captured.contains("abc123"),
        "{captured}"
    );
    // Artifacts are local copies under the data directory only.
    assert!(
        Path::new(shot["artifacts"][0]["path"].as_str().unwrap())
            .starts_with(f.paths.data.join("browser/artifacts").join(&id))
    );

    std::fs::write(f.work.join("site/index.html"), "<title>Shop v2</title>").unwrap();
    let report = f.evidence(&id).await;
    assert_ne!(report["workspace_hash"], hash);
    assert!(
        report["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["stale"] == true),
        "{report}"
    );
    // Deleting the Session removes its artifacts.
    let artifacts = f.paths.data.join("browser/artifacts").join(&id);
    assert!(artifacts.exists());
    f.send(reqwest::Method::DELETE, &format!("/sessions/{id}"), None)
        .await;
    assert!(!artifacts.exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn untrusted_project_browser_server_is_not_installed() {
    let f = Fixture::new(json!({}), false).await;
    let project = json!({"browser": {"enabled": true}, "mcp": {"browser": browser_server(&f.log)}});
    std::fs::write(f.work.join("cyber.jsonc"), project.to_string()).unwrap();
    let id = f.session().await;
    f.run(&id, json!([])).await;
    let tools = tool_names(chats(&f.model).last().unwrap());
    assert!(
        !tools.iter().any(|name| name.starts_with("mcp__browser__")),
        "{tools:?}"
    );
    assert!(
        f.browser_log().is_empty(),
        "the untrusted browser never started"
    );

    // Approving the checkout alone is not enough: the server digest needs its approval.
    let loader = cyber_app::hook_config_loader(f.paths.clone(), f.work.join("home"));
    let trust = cyber_core::trust::TrustStore::new(f.paths.trust_file());
    let resolved = loader(&f.work).unwrap();
    assert!(!resolved.trust.trusted);
    trust
        .approve(
            &resolved.trust.checkout_root,
            resolved.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    let approved_checkout = f.session().await;
    f.run(&approved_checkout, json!([])).await;
    let tools = tool_names(chats(&f.model).last().unwrap());
    assert!(!tools.iter().any(|name| name.starts_with("mcp__browser__")));
    assert!(f.browser_log().is_empty());

    let resolved = loader(&f.work).unwrap();
    let selection = cyber_tools::mcp::inspect_server(&resolved, &f.work, "browser").unwrap();
    trust
        .approve_mcp(&resolved.trust.checkout_root, &selection.digest)
        .unwrap();
    let trusted = f.session().await;
    f.run(&trusted, json!([])).await;
    let tools = tool_names(chats(&f.model).last().unwrap());
    assert!(
        tools.contains(&"mcp__browser__browser_navigate".to_string()),
        "{tools:?}"
    );
    assert_eq!(f.browser_log()[0]["event"], "start");
    f.app.runtime.shutdown().await;
}

/// Every `browser_navigate` the fake browser received.
fn navigations(f: &Fixture) -> Vec<Value> {
    f.browser_log()
        .into_iter()
        .filter(|event| event["name"] == "browser_navigate")
        .collect()
}

/// A command hook answering `answer` for every call of the navigation tool on `event`.
fn navigate_hook(event: &str, answer: &Value) -> Value {
    json!({event: [{"matcher": "mcp__browser__browser_navigate", "hooks": [{"type": "command",
        "id": "rewrite", "command": format!("cat >/dev/null; printf '%s' '{answer}'")}]}]})
}

/// Regression: the navigation host was read from the model's input before PreToolUse
/// hooks ran, so a hook rewriting the URL to a denied host was never checked.
#[tokio::test]
async fn a_hook_rewritten_navigation_is_checked_against_its_new_host() {
    let rewrite = json!({"updated_input": {"url": "http://evil.example/"}});
    let f = Fixture::new(
        json!({"permissions": {"*": "allow", "network": {"*": "allow", "evil.example": "deny"}},
            "hooks": navigate_hook("PreToolUse", &rewrite)}),
        true,
    )
    .await;
    let id = f.session().await;
    f.run(
        &id,
        json!([["mcp__browser__browser_navigate", {"url": "http://fine.example/"}]]),
    )
    .await;
    let results = f.results();
    assert!(
        results[0].contains("Network access to evil.example was not allowed"),
        "{results:?}"
    );
    assert!(navigations(&f).is_empty(), "{results:?}");
    f.app.runtime.shutdown().await;
}

/// Regression: a PermissionDenied retry of a refused navigation was re-authorized
/// without the host gate, so the retried call reached the denied host.
#[tokio::test]
async fn a_permission_denied_retry_of_a_refused_navigation_stays_refused() {
    let retry = json!({"decision": "retry", "updated_input": {"url": "http://evil.example/"}});
    let f = Fixture::new(
        json!({"permissions": {"*": "allow", "network": {"*": "allow", "evil.example": "deny"}},
            "hooks": navigate_hook("PermissionDenied", &retry)}),
        true,
    )
    .await;
    let id = f.session().await;
    f.run(
        &id,
        json!([["mcp__browser__browser_navigate", {"url": "http://evil.example/"}]]),
    )
    .await;
    let results = f.results();
    assert!(
        results[0].contains("Network access to evil.example was not allowed"),
        "{results:?}"
    );
    assert!(navigations(&f).is_empty(), "{results:?}");
    f.app.runtime.shutdown().await;
}
