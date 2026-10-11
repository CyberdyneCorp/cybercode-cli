//! `cyber exec --command <forked skill>` waits for the forked child and its handback Turn.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
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

fn reply(stream: &mut TcpStream, text: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A fake OpenAI-compatible provider. The forked child (`corp/skill`) answers
/// only after `child_delay` (or once released), so an exec that returned on the
/// parent's first idle would tear down the embedded server before the child or
/// the handback ran; the parent (`corp/main`) answers the handback with the final text.
struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

/// Blocks until `delay` passed or the test released the child.
fn hold(delay: Duration, release: &AtomicBool) {
    let started = Instant::now();
    while started.elapsed() < delay && !release.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(10));
    }
}

impl Provider {
    fn start(child_delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let (seen, stop, released) = (requests.clone(), done.clone(), release.clone());
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
                let (seen, released) = (seen.clone(), released.clone());
                thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let Some(body) = read_body(&mut stream) else {
                        return;
                    };
                    seen.lock().unwrap().push(body.clone());
                    let messages = body["messages"].to_string();
                    match body["model"].as_str() {
                        Some("skill") => {
                            hold(child_delay, &released);
                            reply(&mut stream, "audit summary");
                        }
                        Some("main") if messages.contains("audit summary") => {
                            reply(&mut stream, "final summary");
                        }
                        Some("main") => reply(&mut stream, "inline answer"),
                        _ => reply(&mut stream, "unexpected request"),
                    }
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

    /// Lets a held child answer now.
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

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new(provider: &Provider, skill: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for path in ["home", "repo", "cyber/config/skills/audit"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::write(
            root.join("cyber/config/cyber.jsonc"),
            json!({
                "permissions":"allow",
                "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{},"skill":{}}}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(root.join("cyber/config/skills/audit/SKILL.md"), skill).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self { _dir: dir, root }
    }

    /// Runs exec, killing it after 60 s so a hung exec fails instead of blocking.
    fn exec(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--cwd", self.root.join("repo").to_str().unwrap(), "exec"])
            .args(["--embedded", "--model", "corp/main", "--format", "json"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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

    /// Statuses of every background Job in the run's embedded database.
    fn job_statuses(&self) -> Vec<String> {
        let dir = self.root.join("cyber/data/embedded");
        let mut statuses = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "db") {
                continue;
            }
            let db = rusqlite::Connection::open(&path).unwrap();
            let mut query = db.prepare("SELECT status FROM job").unwrap();
            let rows = query.query_map([], |row| row.get::<_, String>(0)).unwrap();
            statuses.extend(rows.map(Result::unwrap));
        }
        statuses
    }
}

fn describe(output: &std::process::Output, requests: &[Value]) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}\nprovider models={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        requests
            .iter()
            .map(|body| body["model"].clone())
            .collect::<Vec<_>>()
    )
}

fn skill(frontmatter: &str) -> String {
    format!(
        "---\nname: audit\ndescription: Audit\ncontext: fork\n{frontmatter}model: corp/skill\n---\nPRIVATE FORK BODY\n"
    )
}

fn result(output: &std::process::Output, detail: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}\n{detail}"))
}

/// A forked command's answer is the parent's handback Turn, which only runs
/// after the child completed; an early exit would never reach it.
fn assert_forked_command_reports_the_handback_answer(frontmatter: &str) {
    let provider = Provider::start(Duration::from_millis(1500));
    let env = Env::new(&provider, &skill(frontmatter));
    let output = env.exec(&["--command", "audit", "check-the-ledger"]);
    let requests = provider.requests();
    let detail = describe(&output, &requests);
    let child = requests
        .iter()
        .find(|body| body["model"] == "skill")
        .unwrap_or_else(|| panic!("the forked child never reached the provider\n{detail}"));
    let child_messages = child["messages"].to_string();
    assert!(child_messages.contains("PRIVATE FORK BODY"), "{detail}");
    assert!(
        child_messages.contains("check-the-ledger"),
        "the forked child lacks the parent's command message: {child_messages}"
    );
    assert!(
        requests.iter().any(|body| {
            body["model"] == "main" && body["messages"].to_string().contains("audit summary")
        }),
        "exec returned before the parent's handback Turn ran\n{detail}"
    );
    assert!(output.status.success(), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["result"], "final summary", "{detail}");
    assert_eq!(result["exit_code"], 0, "{detail}");
    // Every provider call (the child's included) is spend of this run's subtree.
    assert_eq!(
        result["total_tokens"],
        110 * requests.len(),
        "usage omits descendant spend\n{detail}"
    );
    assert_eq!(env.job_statuses(), ["completed"], "{detail}");
}

#[test]
fn exec_forked_command_waits_for_child_completion_and_reports_the_handback_answer() {
    assert_forked_command_reports_the_handback_answer("");
}

#[test]
fn exec_forked_command_with_named_agent_waits_for_the_handback_answer() {
    assert_forked_command_reports_the_handback_answer("agent: reviewer\n");
}

#[test]
fn exec_timeout_cancels_the_owned_forked_child_without_a_later_handback() {
    // The child is held until exec exits, so it is in flight when the timeout
    // fires however slow the runner is.
    let provider = Provider::start(Duration::from_secs(120));
    let env = Env::new(&provider, &skill(""));
    let output = env.exec(&["--timeout", "3s", "--command", "audit", "now"]);
    // A handback that survived exit would reach the provider once the child answered.
    provider.release();
    thread::sleep(Duration::from_secs(1));
    let requests = provider.requests();
    let detail = describe(&output, &requests);
    assert_eq!(output.status.code(), Some(4), "{detail}");
    let result = result(&output, &detail);
    assert_eq!(result["stop_reason"], "timeout", "{detail}");
    assert!(
        requests.iter().any(|body| body["model"] == "skill"),
        "the forked child never started\n{detail}"
    );
    assert!(
        !requests.iter().any(|body| body["model"] == "main"),
        "a parent Turn ran after the stopped run\n{detail}"
    );
    assert_eq!(env.job_statuses(), ["cancelled"], "{detail}");
}

#[test]
fn exec_without_background_work_finishes_on_the_first_idle() {
    let provider = Provider::start(Duration::ZERO);
    let env = Env::new(&provider, &skill(""));
    for args in [&["plain", "question"][..], &["--command", "inline", "now"]] {
        std::fs::create_dir_all(env.root.join("cyber/config/skills/inline")).unwrap();
        std::fs::write(
            env.root.join("cyber/config/skills/inline/SKILL.md"),
            "---\nname: inline\ndescription: Inline\n---\nINLINE BODY\n",
        )
        .unwrap();
        let output = env.exec(args);
        let requests = provider.requests();
        let detail = describe(&output, &requests);
        assert!(output.status.success(), "{detail}");
        assert_eq!(
            result(&output, &detail)["result"],
            "inline answer",
            "{detail}"
        );
        assert!(env.job_statuses().is_empty(), "{detail}");
    }
}
