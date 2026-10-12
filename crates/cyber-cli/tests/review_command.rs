//! `cyber review [target]`: host-resolved targets, owned reviewer Jobs and
//! validated, deterministically ordered findings (`vcs-integration` → Local review command).
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
    time::{Duration, Instant},
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

fn stream(stream: &mut TcpStream, chunks: &[Value]) {
    let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn usage() -> Value {
    json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":10}})
}

fn reply(out: &mut TcpStream, text: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]});
    stream(out, &[chunk, usage()]);
}

/// Each attempt has its own call ID, as a real provider issues.
fn return_result(out: &mut TcpStream, id: usize, result: &Value) {
    let call = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call_{id}"),"type":"function",
        "function":{"name":"return_result","arguments":result.to_string()}}]},"finish_reason":"tool_calls"}]});
    stream(out, &[call, usage()]);
}

/// The scripted reviewer's first tool call: a `read` of `path`.
fn read_call(out: &mut TcpStream, path: &str) {
    let call = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_read","type":"function",
        "function":{"name":"read","arguments":json!({"path":path}).to_string()}}]},"finish_reason":"tool_calls"}]});
    stream(out, &[call, usage()]);
}

/// The tool message answering the scripted reviewer's `read`, if any.
#[cfg(unix)]
fn read_result(body: &Value) -> Option<String> {
    body["messages"]
        .as_array()?
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "call_read")
        .map(|m| m["content"].to_string())
}

fn is_reviewer(body: &Value) -> bool {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tool| tool["function"]["name"] == "return_result")
}

/// The reviewer answers with `result` (after `delay`, or once released); a
/// reviewer request that follows a tool result gets a text-only answer, and
/// the parent answers its handback (or an overriding command) with text.
/// With `read_first`, the reviewer first reads that path and answers with
/// `result` once the read's tool result (successful or not) comes back.
#[derive(Clone)]
struct Script {
    result: Value,
    delay: Duration,
    read_first: Option<&'static str>,
}

struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

fn hold(delay: Duration, release: &AtomicBool) {
    let started = Instant::now();
    while started.elapsed() < delay && !release.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(10));
    }
}

fn answer(out: &mut TcpStream, body: &Value, script: &Script, release: &AtomicBool) {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let after_tool = messages.last().is_some_and(|m| m["role"] == "tool");
    if !is_reviewer(body) {
        let text = if body["messages"].to_string().contains("OVERRIDE REVIEW") {
            "override answer"
        } else {
            "review handed back"
        };
        return reply(out, text);
    }
    if let Some(path) = script.read_first {
        let requested = body["messages"].to_string().contains("call_read");
        if !requested {
            return read_call(out, path);
        }
        if messages
            .last()
            .is_some_and(|m| m["tool_call_id"] == "call_read")
        {
            return return_result(out, messages.len(), &script.result);
        }
    }
    if after_tool {
        return reply(out, "reviewer text-only answer");
    }
    hold(script.delay, release);
    return_result(out, messages.len(), &script.result);
}

impl Provider {
    fn start(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let (seen, stop, released) = (requests.clone(), done.clone(), release.clone());
        let server = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let mut out = match listener.accept() {
                    Ok((out, _)) => out,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                let (seen, released, script) = (seen.clone(), released.clone(), script.clone());
                thread::spawn(move || {
                    out.set_nonblocking(false).unwrap();
                    out.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let Some(body) = read_body(&mut out) else {
                        return;
                    };
                    seen.lock().unwrap().push(body.clone());
                    answer(&mut out, &body, &script, &released);
                });
            }
        });
        Self {
            url,
            requests,
            done,
            release,
            server: Some(server),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn reviewer_prompt(&self) -> Option<String> {
        self.requests()
            .iter()
            .find(|body| is_reviewer(body))
            .map(|body| body["messages"].to_string())
    }

    fn release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        self.release();
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn helper() -> PathBuf {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_cyber"));
    let helper = exe.with_file_name(if cfg!(windows) {
        "cyber-sandbox-exec.exe"
    } else {
        "cyber-sandbox-exec"
    });
    assert!(
        helper.is_file(),
        "build the helper first: cargo build -p cyber-sandbox --bin cyber-sandbox-exec"
    );
    helper
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

impl Env {
    /// A committed repository on `main` with two tracked files.
    fn new(provider: &Provider) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        for path in ["home", "repo", "cyber/config", "empty-path"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::write(
            root.join("cyber/config/cyber.jsonc"),
            json!({
                "permissions":"allow",
                "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.name", "Review test"]);
        git(&repo, &["config", "user.email", "review@example.invalid"]);
        let env = Self {
            _dir: dir,
            root,
            repo,
        };
        env.write("a.rs", "fn a() {}\n");
        env.write("b.rs", "fn b() {}\n");
        env.commit("baseline");
        env
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.repo.join(name), text).unwrap();
    }

    /// Replaces the user configuration's permissions.
    fn permissions(&self, provider: &Provider, permissions: Value) {
        self.configure(provider, permissions, None);
    }

    /// Allows every tool and installs global (user-scope) `hooks`.
    #[cfg(unix)]
    fn hooks(&self, provider: &Provider, hooks: Value) {
        self.configure(provider, json!("allow"), Some(hooks));
    }

    fn configure(&self, provider: &Provider, permissions: Value, hooks: Option<Value>) {
        let mut config = json!({
            "permissions":permissions,
            "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}
        });
        if let Some(hooks) = hooks {
            config["hooks"] = hooks;
        }
        std::fs::write(
            self.root.join("cyber/config/cyber.jsonc"),
            config.to_string(),
        )
        .unwrap();
    }

    fn commit(&self, message: &str) -> String {
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "--quiet", "-m", message]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--cwd", self.repo.to_str().unwrap(), "review"])
            .args(["--model", "corp/main"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_SANDBOX_HELPER", helper())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Runs `cyber review --embedded`, killing it after 60 s so a hang fails.
    fn review(&self, args: &[&str]) -> std::process::Output {
        self.run(self.command(&[&["--embedded"], args].concat()))
    }

    fn run(&self, mut command: Command) -> std::process::Output {
        let child = command.spawn().unwrap();
        let id = child.id();
        let (exited, deadline) = mpsc::channel::<()>();
        let killer = thread::spawn(move || {
            if deadline.recv_timeout(Duration::from_secs(60))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                let _ = Command::new("kill").args(["-9", &id.to_string()]).status();
            }
        });
        let output = child.wait_with_output().unwrap();
        let _ = exited.send(());
        killer.join().unwrap();
        output
    }

    fn databases(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.root.join("cyber/data/embedded")) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
            .collect()
    }

    /// `cyber hooks history --session <session>` over the run's embedded database.
    #[cfg(unix)]
    fn hook_history(&self, session: &str) -> Value {
        let [database] = self.databases().try_into().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["hooks", "history", "--session", session, "--format", "json"])
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", database)
            .current_dir(&self.repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Statuses of every background Job in the run's embedded database.
    fn job_statuses(&self) -> Vec<String> {
        let mut statuses = Vec::new();
        for path in self.databases() {
            let db = rusqlite::Connection::open(&path).unwrap();
            let mut query = db.prepare("SELECT status FROM job").unwrap();
            let rows = query.query_map([], |row| row.get::<_, String>(0)).unwrap();
            statuses.extend(rows.map(Result::unwrap));
        }
        statuses
    }
}

fn describe(output: &std::process::Output, provider: &Provider) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}\nrequests (reviewer, roles)={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        provider
            .requests()
            .iter()
            .map(|body| {
                let roles: Vec<_> = body["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|m| m["role"].as_str().unwrap_or_default().to_string())
                    .collect();
                (is_reviewer(body), roles)
            })
            .collect::<Vec<_>>()
    )
}

fn result(output: &std::process::Output, detail: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}\n{detail}"))
}

fn finding(severity: &str, file: &str, line: u64) -> Value {
    json!({"file":file,"line":line,"severity":severity,"summary":format!("{severity} defect"),"failure_scenario":"a concrete input"})
}

fn script(result: Value) -> Script {
    Script {
        result,
        delay: Duration::ZERO,
        read_first: None,
    }
}

/// `severity file:line` of each reported finding, in reported order.
fn order(result: &Value, detail: &str) -> Vec<String> {
    result["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("{detail}"))
        .iter()
        .map(|f| {
            let severity = f["severity"].as_str().unwrap();
            format!("{severity} {}:{}", f["file"].as_str().unwrap(), f["line"])
        })
        .collect()
}

/// A completed bundled review: its target, reviewer identity and handback text.
fn assert_completed_review(result: &Value, kind: &str, detail: &str) {
    assert_eq!(result["target_kind"], kind, "{detail}");
    assert_eq!(result["reviewed"], true, "{detail}");
    assert_eq!(result["result"], "review handed back", "{detail}");
    assert_eq!(result["exit_code"], 0, "{detail}");
    let delegation = result["delegation_id"].as_str().unwrap_or_default();
    assert!(delegation.starts_with("op_skill_"), "{detail}");
    assert!(result["job_id"].as_str().is_some(), "{detail}");
    assert_ne!(
        result["session_id"], result["parent_session_id"],
        "{detail}"
    );
}

fn unsorted() -> Value {
    json!({"findings":[finding("low","b.rs",3),finding("critical","z.rs",9),finding("high","a.rs",20),finding("high","a.rs",4)],"limitations":["no tests ran"]})
}

#[test]
fn review_waits_for_the_reviewer_job_and_reports_sorted_findings() {
    // The reviewer is held, so a CLI that returned on the parent's first idle
    // would exit before the reviewer Job and its handback settled.
    let provider = Provider::start(Script {
        result: unsorted(),
        delay: Duration::from_millis(1500),
        read_first: None,
    });
    let env = Env::new(&provider);
    env.write("a.rs", "fn a() { changed() }\n");
    env.write("b.rs", "fn b() { changed() }\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(
        order(&result, &detail),
        [
            "critical z.rs:9",
            "high a.rs:4",
            "high a.rs:20",
            "low b.rs:3"
        ],
        "{detail}"
    );
    assert_eq!(
        result["counts"],
        json!({"critical":1,"high":2,"medium":0,"low":1}),
        "{detail}"
    );
    assert_eq!(result["limitations"], json!(["no tests ran"]), "{detail}");
    assert_eq!(result["target"], "", "{detail}");
    assert_completed_review(&result, "uncommitted", &detail);
    let prompt = provider
        .reviewer_prompt()
        .unwrap_or_else(|| panic!("{detail}"));
    assert!(
        prompt.contains("<review-target kind=\\\"uncommitted\\\""),
        "{prompt}"
    );
    for name in ["a.rs", "b.rs"] {
        assert!(prompt.contains(&format!(" {name} (")), "{name}: {prompt}");
    }
    assert_eq!(env.job_statuses(), ["completed"], "{detail}");
}

#[test]
fn review_text_output_renders_findings_most_severe_first() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.write("a.rs", "fn a() { changed() }\n");
    let output = env.review(&["--format", "text"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let positions: Vec<usize> = [
        "[critical] z.rs:9",
        "[high] a.rs:4",
        "[high] a.rs:20",
        "[low] b.rs:3",
        "Limitation: no tests ran",
    ]
    .iter()
    .map(|line| {
        stdout
            .find(line)
            .unwrap_or_else(|| panic!("{line}\n{detail}"))
    })
    .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{detail}");
}

#[test]
fn review_resolves_ranges_and_commits_to_the_changed_files() {
    let provider = Provider::start(script(json!({"findings":[],"limitations":[]})));
    let env = Env::new(&provider);
    git(&env.repo, &["checkout", "--quiet", "-b", "feature"]);
    env.write("c.rs", "fn c() {}\n");
    env.write("a.rs", "fn a() { range() }\n");
    let sha = env.commit("feature");
    for (target, kind) in [("main..HEAD", "range"), (sha.as_str(), "commit")] {
        let output = env.review(&["--format", "json", target]);
        let detail = describe(&output, &provider);
        assert!(output.status.success(), "{target}: {detail}");
        let result = result(&output, &detail);
        assert_eq!(result["target"], target, "{detail}");
        assert_eq!(result["target_kind"], kind, "{detail}");
        assert_eq!(result["findings"], json!([]), "{detail}");
        let prompt = provider
            .requests()
            .iter()
            .rev()
            .find(|body| is_reviewer(body))
            .map(|body| body["messages"].to_string())
            .unwrap_or_else(|| panic!("{detail}"));
        assert!(
            prompt.contains(&format!("<review-target kind=\\\"{kind}\\\"")),
            "{prompt}"
        );
        for name in ["a.rs", "c.rs"] {
            assert!(prompt.contains(&format!(" {name} (")), "{name}: {prompt}");
        }
        assert!(!prompt.contains(" b.rs ("), "{prompt}");
    }
}

#[test]
fn review_of_a_clean_tree_reports_no_changes_without_a_job() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    let output = env.review(&[]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No changes to review"),
        "{detail}"
    );
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["findings"], json!([]), "{detail}");
    assert_eq!(result["reviewed"], false, "{detail}");
    assert_eq!(result["exit_code"], 0, "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
    assert!(provider.requests().is_empty(), "{detail}");
}

/// Regression: changes withheld in full by read policy exited 0 with no
/// findings, indistinguishable from a clean tree to a CI gate.
#[test]
fn review_of_only_withheld_changes_fails_instead_of_passing_as_clean() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.permissions(
        &provider,
        json!({"*":"allow","read":{".env.production":"ask"}}),
    );
    env.write(".env.production", "TOKEN=production-secret\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(2), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["code"], "all_withheld", "{detail}");
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert_eq!(result["reviewed"], false, "{detail}");
    assert_eq!(result["exit_code"], 2, "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
    assert!(provider.requests().is_empty(), "{detail}");
}

#[test]
fn invalid_targets_exit_2_before_any_session_is_admitted() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.write("a.rs", "changed\n");
    for (args, code) in [
        (&["--", "--output=x"][..], "invalid_ref"),
        (&["a...b"][..], "invalid_target"),
        (&["HEAD@{1}"][..], "invalid_ref"),
    ] {
        let output = env.review(&[&["--format", "json"], args].concat());
        let detail = describe(&output, &provider);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {detail}");
        let result = result(&output, &detail);
        assert_eq!(result["code"], code, "{detail}");
        assert_eq!(result["findings"], Value::Null, "{detail}");
        assert_eq!(result["exit_code"], 2, "{detail}");
    }
    assert!(!env.repo.join("x").exists() && !env.root.join("x").exists());
    assert!(env.databases().is_empty(), "a Session was created");
    assert!(provider.requests().is_empty());
}

#[test]
fn invalid_findings_twice_fail_the_review_with_schema_mismatch() {
    let provider = Provider::start(script(
        json!({"findings":[finding("blocker","a.rs",0)],"limitations":[]}),
    ));
    let env = Env::new(&provider);
    env.write("a.rs", "changed\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(1), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|e| e.contains("SchemaMismatch")),
        "{detail}"
    );
    assert_eq!(result["exit_code"], 1, "{detail}");
    assert_eq!(env.job_statuses(), ["error"], "{detail}");
}

#[test]
fn review_timeout_cancels_only_the_owned_reviewer_job() {
    let provider = Provider::start(Script {
        result: unsorted(),
        delay: Duration::from_secs(120),
        read_first: None,
    });
    let env = Env::new(&provider);
    env.write("a.rs", "changed\n");
    let output = env.review(&["--format", "json", "--timeout", "3s"]);
    // A handback that survived exit would reach the provider once the reviewer answered.
    provider.release();
    thread::sleep(Duration::from_secs(1));
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(4), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["stop_reason"], "timeout", "{detail}");
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert!(provider.reviewer_prompt().is_some(), "{detail}");
    assert!(
        !provider.requests().iter().any(|body| !is_reviewer(body)),
        "a parent Turn ran after the stopped review\n{detail}"
    );
    assert_eq!(env.job_statuses(), ["cancelled"], "{detail}");
}

#[test]
fn a_project_review_command_overrides_the_bundled_review() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    std::fs::create_dir_all(env.repo.join(".cyber/commands")).unwrap();
    env.write(".cyber/commands/review.md", "OVERRIDE REVIEW $ARGUMENTS\n");
    env.commit("override");
    let output = env.review(&["--format", "json", "HEAD"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert_eq!(result["result"], "override answer", "{detail}");
    assert_eq!(result["exit_code"], 0, "{detail}");
    assert!(provider.reviewer_prompt().is_none(), "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
}

#[test]
fn pull_request_review_without_gh_exits_2() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    let mut command = env.command(&["--embedded", "--format", "json", "pr", "3"]);
    command.env("PATH", env.root.join("empty-path"));
    let output = env.run(command);
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(2), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["code"], "gh_unavailable", "{detail}");
    assert_eq!(result["target"], "pr 3", "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
    assert!(provider.requests().is_empty(), "{detail}");
}

/// Regression: a 403 refusal printed only a stderr error, not the review object.
#[test]
fn a_permission_denied_review_prints_the_review_object_without_a_job() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.permissions(&provider, json!({"*":"allow","bash":{"gh *":"deny"}}));
    for (flags, code) in [(&[][..], 1), (&["--fail-on-deny"][..], 5)] {
        let output = env.review(&[&["--format", "json"], flags, &["pr", "3"]].concat());
        let detail = describe(&output, &provider);
        assert_eq!(output.status.code(), Some(code), "{detail}");
        let result = result(&output, &detail);
        assert_eq!(result["target"], "pr 3", "{detail}");
        assert_eq!(result["target_kind"], "pr", "{detail}");
        assert_eq!(result["reviewed"], false, "{detail}");
        assert_eq!(result["findings"], Value::Null, "{detail}");
        assert_eq!(result["stop_reason"], "error", "{detail}");
        assert_eq!(result["exit_code"], code, "{detail}");
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|e| e.contains("Permission denied")),
            "{detail}"
        );
    }
    let output = env.review(&["--format", "stream-json", "pr", "3"]);
    let detail = describe(&output, &provider);
    let last = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .unwrap_or_else(|| panic!("{detail}"));
    assert_eq!(last["type"], "result", "{detail}");
    assert_eq!(last["target"], "pr 3", "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
    assert!(provider.requests().is_empty(), "{detail}");
}

/// A reviewer fork refused before its Job exists (here an `agent` rule denies
/// the reviewer at delegation admission) fails the review instead of passing it.
#[test]
fn a_reviewer_refused_at_delegation_admission_fails_the_review() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.permissions(&provider, json!({"*":"allow","agent":{"reviewer":"deny"}}));
    env.write("a.rs", "fn a() { changed() }\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(1), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["reviewed"], false, "{detail}");
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert_eq!(result["exit_code"], 1, "{detail}");
    assert!(
        result["delegation_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("op_skill_")),
        "{detail}"
    );
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|e| e.contains("Permission denied")),
        "{detail}"
    );
    assert!(provider.reviewer_prompt().is_none(), "{detail}");
    assert!(env.job_statuses().is_empty(), "{detail}");
}

/// A global command hook that discards its event and prints `output`.
#[cfg(unix)]
fn hook(output: &str) -> Value {
    json!({"type":"command","command":format!("cat >/dev/null; printf '%s' '{output}'"),"id":"guard"})
}

#[cfg(unix)]
fn read_first(result: Value) -> Script {
    Script {
        read_first: Some("a.rs"),
        ..script(result)
    }
}

/// The reviewer launch is a subagent launch: a global SubagentStart deny
/// refuses it before any reviewer Job or provider request, failing the review.
#[cfg(unix)]
#[test]
fn a_subagent_start_hook_deny_refuses_the_reviewer_launch() {
    let provider = Provider::start(script(unsorted()));
    let env = Env::new(&provider);
    env.hooks(
        &provider,
        json!({"SubagentStart":[{"matcher":"reviewer","hooks":[hook(r#"{"decision":"deny","reason":"reviews frozen"}"#)]}]}),
    );
    env.write("a.rs", "fn a() { changed() }\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert_eq!(output.status.code(), Some(1), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["reviewed"], false, "{detail}");
    assert_eq!(result["findings"], Value::Null, "{detail}");
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|e| e.contains("blocked by hook guard: reviews frozen")),
        "{detail}"
    );
    assert!(env.job_statuses().is_empty(), "{detail}");
    assert!(
        !provider.requests().iter().any(is_reviewer),
        "the reviewer reached the provider\n{detail}"
    );
}

/// The reviewer's own tool calls run through the invoking Location's
/// PreToolUse hooks: a denied read reaches the reviewer as a tool error, the
/// review still completes, and the blocked receipt is on the reviewer Session.
#[cfg(unix)]
#[test]
fn a_pre_tool_use_hook_deny_blocks_the_reviewers_read() {
    let provider = Provider::start(read_first(unsorted()));
    let env = Env::new(&provider);
    env.hooks(
        &provider,
        json!({"PreToolUse":[{"matcher":"read","hooks":[hook(r#"{"decision":"deny","reason":"no reads"}"#)]}]}),
    );
    env.write("a.rs", "fn a() { PRIVATE_BODY() }\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    assert_completed_review(&result, "uncommitted", &detail);
    assert_eq!(result["counts"]["critical"], 1, "{detail}");
    let tool = provider
        .requests()
        .iter()
        .filter(|body| is_reviewer(body))
        .find_map(read_result)
        .unwrap_or_else(|| panic!("no reviewer follow-up after the read\n{detail}"));
    assert!(tool.contains("blocked by hook guard: no reads"), "{tool}");
    assert!(!tool.contains("PRIVATE_BODY"), "{tool}");
    let child = result["session_id"].as_str().unwrap();
    let history = env.hook_history(child);
    let receipts = history["data"].as_array().unwrap();
    assert_eq!(receipts.len(), 1, "{history}");
    assert_eq!(receipts[0]["event"], "PreToolUse", "{history}");
    assert_eq!(receipts[0]["tool_name"], "read", "{history}");
    assert_eq!(receipts[0]["outcome"], "blocked", "{history}");
}

/// Host-side target resolution is not a tool call: a PreToolUse hook matching
/// every tool sees the reviewer's read first and nothing on the parent.
#[cfg(unix)]
#[test]
fn review_target_resolution_runs_no_pre_tool_use_hook() {
    let provider = Provider::start(read_first(json!({"findings":[],"limitations":[]})));
    let env = Env::new(&provider);
    let log = env.root.join("pre-tool-use.jsonl");
    env.hooks(
        &provider,
        json!({"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":format!("{{ cat; echo; }} >> '{}'",log.display()),"id":"recorder"}]}]}),
    );
    env.write("a.rs", "fn a() { changed() }\n");
    let output = env.review(&["--format", "json"]);
    let detail = describe(&output, &provider);
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    let child = result["session_id"].as_str().unwrap();
    let parent = result["parent_session_id"].as_str().unwrap();
    let calls: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let first = calls.first().unwrap_or_else(|| panic!("{detail}"));
    assert_eq!(first["tool_name"], "read", "{calls:?}");
    assert_eq!(first["session_id"], child, "{calls:?}");
    assert!(
        calls.iter().all(|call| call["session_id"] != parent),
        "{calls:?}"
    );
}
