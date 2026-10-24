//! `cyber exec --file` with images (`exec-mode` → Slash commands, files and attachments)
//! through the real binary and its embedded server against a local stub provider: a
//! screenshot reaches a vision model, a text-only model refuses it before any Turn, and a
//! file outside the Location is denied in `dont-ask` with a hint to add its directory.
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// A minimal PDF.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << >> endobj\ntrailer << >>\n%%EOF\n";

/// A valid 1x1 PNG.
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

fn read_body(stream: &mut TcpStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 16384];
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

/// A root with a Location `repo` holding `shot.png` and `spec.pdf`, another
/// `shots/outside.png` beside it, and provider `corp` whose `see` model takes images,
/// `docs` PDFs, and `text` neither.
fn root_with(provider: &Provider) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for path in ["home", "repo", "shots", "cyber/config"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap();
    std::fs::write(root.join("repo/shot.png"), &png).unwrap();
    std::fs::write(root.join("shots/outside.png"), &png).unwrap();
    std::fs::write(root.join("repo/spec.pdf"), PDF).unwrap();
    let config = json!({
        "providers":{"corp":{"api":{"type":"openai-compatible","url":provider.url},"models":{
            "see":{"capabilities":{"vision":true}},
            "docs":{"capabilities":{"pdf":true}},
            "text":{}
        }}}
    });
    std::fs::write(root.join("cyber/config/cyber.jsonc"), config.to_string()).unwrap();
    std::fs::write(root.join("models.json"), "{}").unwrap();
    (dir, root)
}

fn exec(root: &Path, model: &str, file: &Path) -> Output {
    exec_prompt(root, model, file, "what is wrong in this UI?", &["--quiet"])
}

/// `cyber exec` with `output` flags (`--quiet`, or `--format json` for the result).
fn exec_prompt(root: &Path, model: &str, file: &Path, prompt: &str, output: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(["--cwd", root.join("repo").to_str().unwrap(), "exec"])
        .args(["--embedded", "--model", model])
        .args(output)
        .arg("--file")
        .arg(file)
        .arg(prompt)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("CYBER_HOME", root.join("cyber"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MODELS_PATH", root.join("models.json"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root.join("repo"))
        .stdin(Stdio::null());
    for name in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_screenshot_reaches_a_vision_model() {
    let provider = Provider::start();
    let (_dir, root) = root_with(&provider);
    let output = exec(&root, "corp/see", Path::new("shot.png"));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let requests = provider.requests();
    let sent: Vec<String> = requests
        .iter()
        .map(|body| body["messages"].to_string())
        .collect();
    // The Turn's request; the title request carries the text only.
    assert!(
        sent.iter()
            .any(|messages| messages.contains("what is wrong in this UI?")
                && messages.contains("data:image/png;base64,")),
        "{sent:?}"
    );
}

#[test]
fn a_text_only_model_refuses_the_image_with_exit_2() {
    let provider = Provider::start();
    let (_dir, root) = root_with(&provider);
    let output = exec(&root, "corp/text", Path::new("shot.png"));
    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("corp/text has no image input"), "{stderr}");
    assert!(provider.requests().is_empty(), "no Turn ran");
}

#[test]
fn a_file_outside_the_location_is_denied_in_dont_ask_with_the_add_dir_hint() {
    let provider = Provider::start();
    let (_dir, root) = root_with(&provider);
    let outside = root.join("shots/outside.png");
    let output = exec(&root, "corp/see", &outside);
    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("PermissionDeniedError"), "{stderr}");
    let hint = format!("Hint: add --add-dir {}", root.join("shots").display());
    assert!(stderr.contains(&hint), "{stderr}");
    assert!(provider.requests().is_empty(), "no Turn ran");
}

/// The event rows of every child Session in the embedded database.
fn child_events(root: &Path) -> String {
    let mut events = Vec::new();
    for entry in std::fs::read_dir(root.join("cyber/data/embedded")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "db") {
            continue;
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        let mut query = db
            .prepare(
                "SELECT data FROM event WHERE aggregate_id IN \
                 (SELECT id FROM session WHERE parent_id IS NOT NULL)",
            )
            .unwrap();
        let rows = query.query_map([], |row| row.get::<_, String>(0)).unwrap();
        events.extend(rows.map(Result::unwrap));
    }
    events.join("\n")
}

#[test]
fn a_delegated_pdf_is_stored_by_reference_and_needs_pdf_input() {
    let provider = Provider::start();
    let (_dir, root) = root_with(&provider);
    let prompt = "@explore summarize the spec";
    let output = exec_prompt(
        &root,
        "corp/docs",
        Path::new("spec.pdf"),
        prompt,
        &["--quiet"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let encoded = base64::engine::general_purpose::STANDARD.encode(PDF);
    assert!(
        provider
            .requests()
            .iter()
            .any(|body| body.to_string().contains(&encoded)),
        "the child's model gets the PDF"
    );
    let events = child_events(&root);
    assert!(events.contains("\"type\":\"attachment\""), "{events}");
    assert!(
        !events.contains(&encoded),
        "the child's events hold the reference only"
    );

    let provider = Provider::start();
    let (_dir, root) = root_with(&provider);
    let output = exec_prompt(
        &root,
        "corp/text",
        Path::new("spec.pdf"),
        prompt,
        &["--format", "json"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}{}", stderr(&output));
    assert!(stdout.contains("corp/text has no pdf input"), "{stdout}");
    assert!(provider.requests().is_empty(), "no Turn ran");
}
