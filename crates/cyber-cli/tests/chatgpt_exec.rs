//! `provider-credentials` → ChatGPT token refresh through the real binary: `cyber exec` on
//! `chatgpt/<slug>` renews a due stored login under the shared subscription store's
//! refresh lock before the Turn, and a rejected renewal fails closed and keeps the file. An
//! in-process fake issuer and Codex backend on 127.0.0.1 answer; nothing reaches the
//! internet.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use base64::Engine;
use serde_json::{Value, json};

#[derive(Default)]
struct Seen {
    refreshes: Vec<Value>,
    /// `(authorization, chatgpt-account-id)` of each `/responses` request.
    turns: Vec<(String, String)>,
    /// `(session-id, thread-id)` of each `/responses` request.
    routing: Vec<(String, String)>,
}

struct Backend {
    origin: String,
    seen: Arc<Mutex<Seen>>,
    _runtime: tokio::runtime::Runtime,
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn completed() -> String {
    [
        ("response.output_text.delta", json!({"delta": "renewed hi"})),
        (
            "response.completed",
            json!({"response": {"usage": {"input_tokens": 3, "output_tokens": 1}}}),
        ),
    ]
    .iter()
    .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
    .collect()
}

impl Backend {
    /// The fake issuer and backend; `refresh_status` answers every token refresh.
    fn start(refresh_status: u16) -> Self {
        Self::serve(refresh_status, false)
    }

    /// A backend whose plan has reached its usage limit, resetting in 40 minutes.
    fn usage_limited() -> Self {
        Self::serve(200, true)
    }

    fn serve(refresh_status: u16, limited: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (refreshes, turns) = (seen.clone(), seen.clone());
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route(
                "/oauth/token",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
                    refreshes.lock().unwrap().refreshes.push(body);
                    if refresh_status != 200 {
                        let status = StatusCode::from_u16(refresh_status).unwrap();
                        return (status, axum::Json(json!({"error": "invalid_grant"})));
                    }
                    (
                        StatusCode::OK,
                        axum::Json(json!({
                            "access_token": jwt(json!({"exp": 4_000_000_000u64})),
                            "refresh_token": "refresh-2",
                        })),
                    )
                }),
            )
            .route(
                "/responses",
                axum::routing::post(move |headers: HeaderMap| async move {
                    let mut seen = turns.lock().unwrap();
                    seen.turns.push((
                        header(&headers, "authorization"),
                        header(&headers, "chatgpt-account-id"),
                    ));
                    seen.routing.push((
                        header(&headers, "session-id"),
                        header(&headers, "thread-id"),
                    ));
                    if limited {
                        let error = json!({"error": {"type": "usage_limit_reached",
                            "message": "The usage limit has been reached",
                            "plan_type": "plus", "resets_at": now() + 40 * 60}});
                        return (StatusCode::TOO_MANY_REQUESTS, axum::Json(error)).into_response();
                    }
                    ([("content-type", "text/event-stream")], completed()).into_response()
                }),
            );
        runtime.spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            origin,
            seen,
            _runtime: runtime,
        }
    }
}

fn jwt(payload: Value) -> String {
    let encode =
        |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!("{}.{}.sig", encode(json!({"alg": "none"})), encode(payload))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    /// A stored login whose access token expires inside the refresh window, and a fresh
    /// cached model list naming `gpt-6-sol`.
    fn with_due_login() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let env = Self { _dir: dir, root };
        let data = env.root.join("cyber/data");
        std::fs::create_dir_all(&data).unwrap();
        let expires_at = now() + 60;
        write_private(
            &env.auth_file(),
            &json!({
                "version": 1,
                "access_token": jwt(json!({"exp": expires_at})),
                "refresh_token": "refresh-1",
                "account_id": "acct-1",
                "expires_at": expires_at,
            }),
        );
        write_private(
            &data.join("chatgpt-models.json"),
            &json!({"fetched_at": now(), "models": [{"slug": "gpt-6-sol", "name": "GPT-6-Sol",
                "context_window": 272000, "vision": false, "efforts": []}]}),
        );
        std::fs::write(env.root.join("models.json"), "{}").unwrap();
        env
    }

    fn auth_file(&self) -> PathBuf {
        self.root.join("cyber/data/chatgpt-auth.json")
    }

    fn exec(&self, origin: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["exec", "--embedded", "--ephemeral", "--format", "json"])
            .args(["-m", "chatgpt/gpt-6-sol", "say hi"])
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_CHATGPT_ISSUER", origin)
            .env("CYBER_CHATGPT_BACKEND", origin)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .output()
            .unwrap()
    }
}

fn write_private(path: &Path, document: &Value) {
    std::fs::write(path, document.to_string()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn describe(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn exec_renews_a_due_login_once_and_stores_it_owner_only() {
    let (env, backend) = (Env::with_due_login(), Backend::start(200));
    let output = env.exec(&backend.origin);
    assert!(output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("renewed hi"),
        "{}",
        describe(&output)
    );

    let seen = backend.seen.lock().unwrap();
    assert_eq!(seen.refreshes.len(), 1, "one renewal per due login");
    assert_eq!(seen.refreshes[0]["refresh_token"], "refresh-1");
    let renewed = jwt(json!({"exp": 4_000_000_000u64}));
    assert!(!seen.turns.is_empty());
    for (authorization, account) in &seen.turns {
        assert_eq!(authorization, &format!("Bearer {renewed}"));
        assert_eq!(account, "acct-1");
    }
    // `provider-catalog` → Session routing headers: a root Session is its own thread.
    for (session, thread) in &seen.routing {
        assert!(session.starts_with("ses_"), "{session}");
        assert_eq!(session, thread);
    }

    assert_stored(&env.auth_file(), &renewed);
    let transcript = describe(&output);
    assert!(!transcript.contains("refresh-1") && !transcript.contains("refresh-2"));
    assert!(!transcript.contains(&renewed));
}

/// The renewed login was written back, owner-only, in the same format.
fn assert_stored(file: &Path, access_token: &str) {
    assert_eq!(mode(file), 0o600);
    let stored: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(stored["version"], 1);
    assert_eq!(stored["access_token"], access_token);
    assert_eq!(stored["refresh_token"], "refresh-2");
    assert_eq!(stored["account_id"], "acct-1");
}

#[test]
fn exec_with_a_revoked_login_fails_closed_and_keeps_the_file() {
    let (env, backend) = (Env::with_due_login(), Backend::start(400));
    let before = std::fs::read(env.auth_file()).unwrap();
    let output = env.exec(&backend.origin);
    let transcript = describe(&output);
    assert!(!output.status.success(), "{transcript}");
    assert!(
        transcript.contains("cyber providers login chatgpt"),
        "{transcript}"
    );
    assert!(!transcript.contains("refresh-1"));

    let seen = backend.seen.lock().unwrap();
    assert!(!seen.refreshes.is_empty());
    assert!(
        seen.turns.is_empty(),
        "a revoked login never reaches the backend"
    );
    assert_eq!(std::fs::read(env.auth_file()).unwrap(), before);
    assert_eq!(mode(&env.auth_file()), 0o600);
}

/// `provider-catalog` → "Subscription limit reached": with no fallback model, `exec`
/// fails with the local time the plan's limit resets.
#[test]
fn exec_reports_when_a_usage_limit_resets() {
    let (env, backend) = (Env::with_due_login(), Backend::usage_limited());
    let output = env.exec(&backend.origin);
    let transcript = describe(&output);
    assert!(!output.status.success(), "{transcript}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let at = stderr
        .find("usage limit reached; resets at ")
        .unwrap_or_else(|| panic!("{transcript}"));
    let after = &stderr[at + 31..];
    assert!(
        after
            .bytes()
            .take(16)
            .collect::<Vec<_>>()
            .windows(5)
            .any(|w| w[2] == b':' && [0, 1, 3, 4].iter().all(|&i| w[i].is_ascii_digit())),
        "{transcript}"
    );
}
