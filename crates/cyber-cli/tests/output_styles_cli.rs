//! Output styles through the real binary (`system-context` → Output style definitions,
//! `workspace-trust` → Trust before interpretation): project styles are in the
//! checkout's trust digest and are listed by the server only once it is approved, and
//! `cyber exec --output-style` selects one for the Session it creates.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

/// A `cyber serve --register` of an [`Env`], stopped on drop.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // An empty catalog: nothing is fetched.
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("managed"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.repo);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn write(&self, path: PathBuf, text: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn serve(&self) -> Server {
        let server = Server(
            self.command(&["serve", "--register", "--port", "0"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let registration = self.root.join("cyber-home/state/server.json");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !registration.is_file() {
            assert!(Instant::now() < deadline, "no server registered");
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    /// `GET /api/v1/config/output-styles` for the checkout through `cyber api`: its `data`.
    fn listing(&self) -> Value {
        let listed = json(&self.cyber(&["api", "GET", "/config/output-styles"]));
        assert_eq!(
            listed["location"]["directory"],
            self.repo.display().to_string()
        );
        listed["data"].clone()
    }
}

fn json(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
}

fn names(listing: &Value) -> Vec<&str> {
    listing["styles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|style| style["name"].as_str().unwrap())
        .collect()
}

/// Scenario "Project style in an untrusted checkout", then trust approval.
#[test]
fn project_styles_follow_checkout_trust() {
    let env = Env::new();
    let user = env.root.join("cyber-home/config/output-styles");
    env.write(user.join("terse.md"), "Be terse.");
    let reviewer = env.write(
        env.repo.join(".cyber/output-styles/reviewer.md"),
        "---\ndescription: Review like a maintainer\n---\nReview every change carefully.\n",
    );
    let _server = env.serve();

    let held = env.listing();
    assert_eq!(
        names(&held),
        ["default", "concise", "explanatory", "learning", "terse"]
    );
    assert_eq!(
        held["issues"][0]["message"],
        "project output styles are held until this checkout is trusted (cyber trust inspect)"
    );
    assert_eq!(held["effective"], "default");

    let inspect = json(&env.cyber(&["trust", "inspect", "--format", "json"]));
    let definitions = inspect["definitions"].as_array().unwrap();
    assert!(
        definitions
            .iter()
            .any(|d| d.as_str() == Some(&reviewer.display().to_string())),
        "{inspect}"
    );
    let digest = inspect["digest"].as_str().unwrap();
    let approved = env.cyber(&["trust", "approve", "--digest", digest]);
    assert!(approved.status.success(), "{approved:?}");

    let trusted = env.listing();
    assert!(
        trusted["issues"].as_array().unwrap().is_empty(),
        "{trusted}"
    );
    let listed = trusted["styles"].as_array().unwrap();
    let style = listed.iter().find(|s| s["name"] == "reviewer").unwrap();
    assert_eq!(style["origin"], "project");
    assert_eq!(style["description"], "Review like a maintainer");
    assert_eq!(style["path"], reviewer.display().to_string());
    assert!(
        style.get("text").is_none(),
        "the listing carries no prompt text"
    );
}

/// A scripted local OpenAI-compatible provider answering every request with `done`.
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
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, &seen),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            requests,
            done,
            server: Some(server),
        }
    }

    /// The system prompt of the Turn's request (the one offering tools; the title
    /// request has none).
    fn system(&self) -> String {
        let requests = self.requests.lock().unwrap();
        let first = requests
            .iter()
            .find(|body| body["tools"].is_array())
            .expect("the Turn called the model");
        let system = &first["messages"][0];
        assert_eq!(system["role"], "system", "{first}");
        match &system["content"] {
            Value::String(text) => text.clone(),
            parts => parts
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect(),
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

fn answer(mut stream: TcpStream, seen: &Mutex<Vec<Value>>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Some(body) = read_body(&mut stream) else {
        return;
    };
    seen.lock().unwrap().push(body);
    let chunk = json!({"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":1}});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
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

fn sessions(env: &Env) -> usize {
    let listed = json(&env.cyber(&["api", "GET", "/sessions"]));
    listed["data"]["data"].as_array().unwrap().len()
}

/// Scenarios "Output style flag" and "Unknown output style".
#[test]
fn exec_output_style_flag() {
    let env = Env::new();
    let provider = Provider::start();
    let config = json!({
        "permissions": "allow",
        "providers": {"local": {"api": {"type": "openai-compatible", "url": provider.url}, "models": {"main": {}}}},
    });
    env.write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        &config.to_string(),
    );
    let _server = env.serve();
    let exec = |style: &str| {
        env.command(&["exec", "--model", "local/main"])
            .args(["--output-style", style, "hi"])
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };

    let unknown = exec("nope");
    assert_eq!(unknown.status.code(), Some(2), "{unknown:?}");
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("unknown output style \"nope\""), "{stderr}");
    assert!(
        stderr.contains("available: default, concise, explanatory, learning"),
        "{stderr}"
    );
    assert_eq!(sessions(&env), 0, "a refused style creates no Session");
    assert!(provider.requests.lock().unwrap().is_empty());

    let ran = exec("concise");
    assert!(ran.status.success(), "{ran:?}");
    let concise = include_str!("../../cyber-core/prompts/output-styles/concise.md");
    let body = concise.rsplit("---\n").next().unwrap().trim_end();
    // The style ends the system prefix (the base prompt), before the Baseline.
    let system = provider.system();
    let style = system.find(&format!("Output style: concise\n\n{body}"));
    assert!(style.is_some(), "{system}");
    assert!(style < system.find("<available_skills>"), "{system}");
    let listed = json(&env.cyber(&["api", "GET", "/sessions"]));
    let id = listed["data"]["data"][0]["id"].as_str().unwrap();
    let shown = json(&env.cyber(&["api", "GET", &format!("/sessions/{id}")]));
    assert_eq!(shown["data"]["output_style"], "concise", "{shown}");

    // Without the flag, a configured style that does not apply is reported on stderr.
    let mut config = config;
    config["output_style"] = json!("ghost");
    env.write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        &config.to_string(),
    );
    let warned = env
        .command(&["exec", "--model", "local/main", "hi"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(warned.status.success(), "{warned:?}");
    let stderr = String::from_utf8_lossy(&warned.stderr);
    assert!(
        stderr
            .contains("warning: unknown output style \"ghost\"; the default output style applies"),
        "{stderr}"
    );
}
