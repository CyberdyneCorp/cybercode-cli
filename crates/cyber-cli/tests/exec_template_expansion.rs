//! `cyber exec --command` expands a command template's `` !`cmd` `` through the
//! production embedded app (`skills-commands` → Shell output injection). On Windows,
//! where no sandbox route exists yet, the injection fails closed unless the run
//! explicitly selects full access, which is not confinement evidence.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
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

/// A fake OpenAI-compatible provider answering every request with `done`.
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
                seen.lock().unwrap().push(body);
                let chunk = json!({"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10}});
                let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
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

    fn saw(&self, needle: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .any(|body| body["messages"].to_string().contains(needle))
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

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A root whose global (user) command `head` injects the short HEAD hash.
fn root_with(provider: &Provider) -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "cyber/config/commands"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    let repo = root.join("repo");
    git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "init"]);
    let head = git(&repo, &["rev-parse", "--short", "HEAD"]);
    std::fs::write(
        root.join("cyber/config/cyber.jsonc"),
        json!({
            "permissions":"allow",
            "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("cyber/config/commands/head.md"),
        "Head is !`git rev-parse --short HEAD` now.",
    )
    .unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();
    (dir, root, head)
}

fn exec(root: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["--cwd", root.join("repo").to_str().unwrap(), "exec"])
        .args(["--embedded", "--model", "corp/main", "--format", "json"])
        .args(extra)
        .args(["--command", "head"])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        let helper =
            PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec.exe");
        assert!(
            helper.is_file(),
            "build the helper first: cargo build -p cyber-sandbox --bin cyber-sandbox-exec"
        );
        command.env("CYBER_SANDBOX_HELPER", helper);
        for name in ["SystemRoot", "TEMP", "TMP", "ProgramFiles"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    command.output().unwrap()
}

fn detail(output: &std::process::Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Native sandboxes (macOS Seatbelt, Linux bubblewrap) run the injection confined.
#[cfg(not(windows))]
#[test]
fn exec_command_injects_sandboxed_git_output() {
    let provider = Provider::start();
    let (_dir, root, head) = root_with(&provider);
    let output = exec(&root, &[]);
    assert!(output.status.success(), "{}", detail(&output));
    assert!(
        provider.saw(&format!("Head is {head} now.")),
        "{}",
        detail(&output)
    );
}

/// Without a sandbox route the injection never runs; only an explicit full-access
/// run (not confinement evidence) substitutes the output.
#[cfg(windows)]
#[test]
fn exec_command_injection_fails_closed_without_a_windows_sandbox() {
    let provider = Provider::start();
    let (_dir, root, head) = root_with(&provider);
    let output = exec(&root, &[]);
    assert!(output.status.success(), "{}", detail(&output));
    assert!(
        provider.saw("Head is [unavailable: sandbox] now."),
        "{}",
        detail(&output)
    );
    assert!(!provider.saw(&head));
    let output = exec(&root, &["--sandbox", "full-access"]);
    assert!(output.status.success(), "{}", detail(&output));
    assert!(
        provider.saw(&format!("Head is {head} now.")),
        "{}",
        detail(&output)
    );
}
