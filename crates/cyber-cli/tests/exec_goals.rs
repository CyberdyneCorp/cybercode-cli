//! `cyber exec --goal` (`goals` → Non-interactive goals) through the real binary with an
//! embedded server and a fake OpenAI-compatible provider that also plays the evaluator.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

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

fn send(stream: &mut TcpStream, text: &str) {
    let chunk = json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":10}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A provider whose agent always answers `working`, and whose evaluator answers the
/// verdicts in order (the last one repeats).
struct Provider {
    url: String,
    evaluations: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn start(verdicts: &[&'static str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let evaluations = Arc::new(AtomicUsize::new(0));
        let (stop, count, verdicts) = (done.clone(), evaluations.clone(), verdicts.to_vec());
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
                if body["messages"]
                    .to_string()
                    .contains("judge whether a coding agent")
                {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    let verdict = verdicts[n.min(verdicts.len() - 1)];
                    let reply =
                        json!({"verdict": verdict, "reason": format!("verdict {n}: {verdict}")});
                    send(&mut stream, &reply.to_string());
                } else {
                    send(&mut stream, "working");
                }
            }
        });
        Self {
            url,
            evaluations,
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

fn root(provider: &Provider) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo/.git", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(
        root.join("cyber/config/cyber.jsonc"),
        json!({"providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{"main":{}}}}})
            .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();
    (dir, root)
}

/// `cyber exec --embedded --format json <flags>`, started.
fn spawn(root: &Path, flags: &[&str]) -> std::process::Child {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_cyber")).with_file_name("cyber-sandbox-exec");
    Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["--cwd", root.join("repo").to_str().unwrap()])
        .args([
            "exec",
            "--embedded",
            "--model",
            "corp/main",
            "--format",
            "json",
        ])
        .args(flags)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("CYBER_SANDBOX_HELPER", helper)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Wait for `child`, killing it after 90 s; its exit code, stdout and stderr.
fn finish(child: std::process::Child) -> (Option<i32>, String, String) {
    let id = child.id();
    let (exited, deadline) = mpsc::channel::<()>();
    let killer = thread::spawn(move || {
        if deadline.recv_timeout(Duration::from_secs(90)) == Err(mpsc::RecvTimeoutError::Timeout) {
            let _ = Command::new("kill").args(["-9", &id.to_string()]).status();
        }
    });
    let output = child.wait_with_output().unwrap();
    let _ = exited.send(());
    killer.join().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `cyber exec --embedded --format json <flags>`, killed after 90 s.
fn exec(root: &Path, flags: &[&str]) -> (Option<i32>, Value, String) {
    let (code, stdout, stderr) = finish(spawn(root, flags));
    let result = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("stdout={stdout}\nstderr={stderr}"));
    (code, result, stderr)
}

#[test]
fn a_met_goal_whose_check_passes_exits_zero() {
    let provider = Provider::start(&["met"]);
    let (_dir, root) = root(&provider);
    let (code, result, stderr) = exec(
        &root,
        &[
            "--goal",
            "the repository exists",
            "--goal-check",
            "test -d .git",
        ],
    );
    assert_eq!(code, Some(0), "{result}\n{stderr}");
    assert_eq!(result["goals"][0]["status"], "met", "{result}");
    assert_eq!(result["goals"][0]["condition"], "the repository exists");
}

#[test]
fn a_failing_check_keeps_working_until_it_passes() {
    let provider = Provider::start(&["met"]);
    let (_dir, root) = root(&provider);
    let check = "test -f .marker || { touch .marker; exit 1; }";
    let (code, result, stderr) = exec(
        &root,
        &["--goal", "the marker exists", "--goal-check", check],
    );
    assert_eq!(code, Some(0), "{result}\n{stderr}");
    assert_eq!(result["goals"][0]["turns"], 2, "{result}");
    assert_eq!(provider.evaluations.load(Ordering::SeqCst), 2);
}

#[test]
fn an_impossible_goal_exits_three() {
    let provider = Provider::start(&["impossible"]);
    let (_dir, root) = root(&provider);
    let (code, result, stderr) = exec(&root, &["--goal", "deploy to production"]);
    assert_eq!(code, Some(3), "{result}\n{stderr}");
    assert_eq!(result["goals"][0]["status"], "impossible");
    assert_eq!(result["goals"][0]["last_reason"], "verdict 0: impossible");
}

#[test]
fn a_spent_turn_budget_exits_four() {
    let provider = Provider::start(&["not_met"]);
    let (_dir, root) = root(&provider);
    let (code, result, stderr) = exec(&root, &["--goal", "never done", "--max-turns", "2"]);
    assert_eq!(code, Some(4), "{result}\n{stderr}");
    assert_eq!(result["goals"][0]["status"], "budget_exceeded");
    assert_eq!(result["goals"][0]["turns"], 2);
}

#[test]
fn the_exit_reflects_the_first_non_met_goal_of_a_queue() {
    let provider = Provider::start(&["met", "impossible"]);
    let (_dir, root) = root(&provider);
    let (code, result, stderr) = exec(
        &root,
        &["--goal", "first", "--goal", "second", "--goal", "third"],
    );
    assert_eq!(code, Some(3), "{result}\n{stderr}");
    let statuses: Vec<&str> = result["goals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["met", "impossible", "pending"]);
}

#[test]
fn a_check_without_a_goal_is_a_usage_error() {
    let provider = Provider::start(&["met"]);
    let (_dir, root) = root(&provider);
    let output = Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["exec", "--embedded", "--goal-check", "make", "x"])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// SIGINT while a Goal keeps continuing exits 130.
#[cfg(unix)]
#[test]
fn exec_goal_sigint_exits_130() {
    let provider = Provider::start(&["not_met"]);
    let (_dir, root) = root(&provider);
    let child = spawn(&root, &["--goal", "never done", "--max-turns", "50"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while provider.evaluations.load(Ordering::SeqCst) < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "no goal Turn evaluated"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let interrupt = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupt.success());
    let (code, stdout, stderr) = finish(child);
    assert_eq!(code, Some(130), "stdout={stdout}\nstderr={stderr}");
}
