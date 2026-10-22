//! `cyber providers login|logout chatgpt` through the real binary: an in-process fake OpenAI
//! issuer on 127.0.0.1 answers the code exchange, and the test plays the user by pasting
//! the redirect URL (`--method code`). Nothing reaches the internet.
// Login refuses on Windows, so only the other-provider test runs there and the login
// fixtures go unused.
#![cfg_attr(windows, allow(dead_code, unused_imports))]
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{Value, json};

type Exchanges = Arc<Mutex<Vec<HashMap<String, String>>>>;

struct Issuer {
    origin: String,
    exchanges: Exchanges,
    _runtime: tokio::runtime::Runtime,
}

impl Issuer {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let exchanges = Exchanges::default();
        let recorded = exchanges.clone();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route(
                "/models",
                axum::routing::get(|| async {
                    axum::Json(json!({"models": [
                        {"slug": "gpt-6-sol", "display_name": "GPT-6-Sol", "visibility": "list"},
                        {"slug": "gpt-reserve", "visibility": "hide"}
                    ]}))
                }),
            )
            .route(
            "/oauth/token",
            axum::routing::post(
                move |axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                    recorded.lock().unwrap().push(form);
                    axum::Json(json!({
                        "access_token": jwt(json!({"exp": 4_000_000_000u64})),
                        "refresh_token": "refresh-1",
                        "id_token": jwt(json!({"email": "dev@example.com",
                            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-1", "chatgpt_plan_type": "plus"}})),
                    }))
                },
            ),
        );
        runtime.spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            origin,
            exchanges,
            _runtime: runtime,
        }
    }
}

fn jwt(payload: Value) -> String {
    let encode =
        |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!("{}.{}.sig", encode(json!({"alg": "none"})), encode(payload))
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        Self { _dir: dir, root }
    }

    fn command(&self, issuer: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--format", "json", "providers"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_CHATGPT_ISSUER", issuer)
            .env("CYBER_CHATGPT_BACKEND", issuer)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        command
    }

    fn auth_file(&self) -> PathBuf {
        self.root.join("cyber/data/chatgpt-auth.json")
    }

    /// Run `login chatgpt --method code`, pasting the redirect URL `paste` builds from the
    /// `state` of the printed authorize URL.
    fn login_pasting(&self, issuer: &str, paste: impl Fn(&str) -> String) -> Output {
        let mut child = self
            .command(issuer, &["login", "chatgpt", "--method", "code"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let url = loop {
            let mut line = String::new();
            assert!(stderr.read_line(&mut line).unwrap() > 0, "no authorize URL");
            if line.starts_with("http") {
                break line;
            }
        };
        let state = query(&url, "state");
        assert_eq!(query(&url, "client_id"), "app_EMoamEEZ73f0CkXaXp7hrann");
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", paste(&state)).unwrap();
        drop(stdin);
        let mut output = child.wait_with_output().unwrap();
        std::io::Read::read_to_end(&mut stderr, &mut output.stderr).unwrap();
        output
    }
}

fn query(url: &str, name: &str) -> String {
    url.trim()
        .split_once('?')
        .unwrap()
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        .unwrap()
        .to_string()
}

fn stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn headless_login_stores_an_owner_only_login_and_logout_forgets_it() {
    let (env, issuer) = (Env::new(), Issuer::start());
    let output = env.login_pasting(&issuer.origin, |state| {
        format!("http://localhost:1455/auth/callback?code=c-1&state={state}")
    });

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        json!({"provider": "chatgpt", "email": "dev@example.com", "plan": "plus",
            "expires_at": 4_000_000_000u64, "models": ["gpt-6-sol"]})
    );
    let exchanges = issuer.exchanges.lock().unwrap().clone();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0]["grant_type"], "authorization_code");
    assert_eq!(exchanges[0]["code"], "c-1");
    assert_eq!(
        exchanges[0]["redirect_uri"],
        "http://localhost:1455/auth/callback"
    );
    assert!(exchanges[0]["code_verifier"].len() >= 43);
    assert_eq!(mode(&env.auth_file()), 0o600);
    let stored: Value = serde_json::from_slice(&std::fs::read(env.auth_file()).unwrap()).unwrap();
    assert_eq!(stored["account_id"], "acct-1");
    std::fs::write(env.root.join("models.json"), "{}").unwrap();
    let listed = Command::new(env!("CARGO_BIN_EXE_cyber"))
        .args(["--format", "json", "models", "chatgpt"])
        .env_clear()
        .env("HOME", env.root.join("home"))
        .env("CYBER_HOME", env.root.join("cyber"))
        .env("CYBER_MODELS_PATH", env.root.join("models.json"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(&env.root)
        .output()
        .unwrap();
    let refs: Vec<_> = stdout(&listed)["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["ref"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(refs, ["chatgpt/gpt-6-sol"]);

    let logout = env
        .command(&issuer.origin, &["logout", "chatgpt"])
        .output()
        .unwrap();
    assert_eq!(
        stdout(&logout),
        json!({"provider": "chatgpt", "removed": true})
    );
    assert!(!env.auth_file().exists());
    assert!(!env.root.join("cyber/data/chatgpt-models.json").exists());
    let again = env
        .command(&issuer.origin, &["logout", "chatgpt"])
        .output()
        .unwrap();
    assert_eq!(
        stdout(&again),
        json!({"provider": "chatgpt", "removed": false})
    );
}

#[cfg(unix)]
#[test]
fn a_forged_state_is_refused_before_any_token_request() {
    let (env, issuer) = (Env::new(), Issuer::start());
    let output = env.login_pasting(&issuer.origin, |_| {
        "http://localhost:1455/auth/callback?code=c-1&state=forged".to_string()
    });

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(issuer.exchanges.lock().unwrap().is_empty());
    assert!(!env.auth_file().exists());
}

#[test]
fn other_providers_are_told_to_use_their_key_variable() {
    let env = Env::new();
    let output = env
        .command("https://auth.openai.com", &["login", "anthropic"])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        error["error"]["message"],
        "Provider login is not available for anthropic yet"
    );
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("ANTHROPIC_API_KEY")
    );
}

/// A `cyber serve --register` of `env`, killed on drop.
struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Env {
    /// The real binary with the fake issuer and an empty model catalog.
    fn cyber(&self, issuer: &str) -> Command {
        std::fs::write(self.root.join("models.json"), "{}").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_CHATGPT_ISSUER", issuer)
            .env("CYBER_CHATGPT_BACKEND", issuer)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        command
    }

    fn serve(&self, issuer: &str) -> Server {
        let server = Server(
            self.cyber(issuer)
                .args(["serve", "--register", "--port", "0"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let registration = self.root.join("cyber/state/server.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !registration.is_file() {
            assert!(std::time::Instant::now() < deadline, "no server registered");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        server
    }

    /// `cyber api <args>` against the registered server: its exit code and JSON answer.
    fn api(&self, issuer: &str, args: &[&str]) -> (Option<i32>, Value) {
        let output = self.cyber(issuer).arg("api").args(args).output().unwrap();
        let body = if output.stdout.trim_ascii().is_empty() {
            Value::Null
        } else {
            stdout(&output)
        };
        (output.status.code(), body)
    }

    fn chatgpt_models(&self, issuer: &str) -> Vec<String> {
        let (_, models) = self.api(issuer, &["GET", "/models"]);
        models["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|model| model["provider"] == "chatgpt")
            .map(|model| model["id"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// `provider-credentials` → Provider login API: a running server signs in through the API,
/// lists the plan's models without a restart, refuses a forged `state` and signs out.
#[cfg(unix)]
#[test]
fn the_server_signs_in_through_the_api_and_reloads_its_catalog() {
    let (env, issuer) = (Env::new(), Issuer::start());
    let origin = issuer.origin.as_str();
    let _server = env.serve(origin);
    assert!(env.chatgpt_models(origin).is_empty());

    let (code, started) = env.api(origin, &["POST", "/providers/chatgpt/login"]);
    assert_eq!(code, Some(0), "{started}");
    assert_eq!(started["data"]["status"], "pending");
    let attempt = started["data"]["attempt_id"].as_str().unwrap().to_string();
    let state = query(
        started["data"]["authorization_url"].as_str().unwrap(),
        "state",
    );

    let (code, refused) = env.paste_code(origin, &attempt, "forged");
    assert_eq!(code, Some(1), "{refused}");
    assert_eq!(refused["_tag"], "InvalidRequestError");
    assert!(issuer.exchanges.lock().unwrap().is_empty());
    let (_, still) = env.api(
        origin,
        &["GET", &format!("/providers/chatgpt/login/{attempt}")],
    );
    assert_eq!(still["data"]["status"], "pending");

    let (code, done) = env.paste_code(origin, &attempt, &state);
    assert_eq!(code, Some(0), "{done}");
    assert_signed_in(&done["data"]);
    assert_eq!(mode(&env.auth_file()), 0o600);
    assert_eq!(env.chatgpt_models(origin), ["chatgpt/gpt-6-sol"]);

    let (_, logout) = env.api(origin, &["DELETE", "/providers/chatgpt/login"]);
    assert_eq!(logout["data"]["removed"], true);
    assert!(!env.auth_file().exists());
    assert!(env.chatgpt_models(origin).is_empty());
}

impl Env {
    /// Post the redirect URL a browser would land on with `state` to the attempt.
    fn paste_code(&self, issuer: &str, attempt: &str, state: &str) -> (Option<i32>, Value) {
        let path = format!("/providers/chatgpt/login/{attempt}/code");
        let body = json!({"redirect_url": format!(
            "http://localhost:1455/auth/callback?code=c-1&state={state}"
        )});
        self.api(issuer, &["POST", &path, "-d", &body.to_string()])
    }
}

/// A complete attempt names the account and models and carries no token.
fn assert_signed_in(attempt: &Value) {
    assert_eq!(attempt["status"], "complete");
    assert_eq!(attempt["email"], "dev@example.com");
    assert_eq!(attempt["plan"], "plus");
    assert_eq!(attempt["models"], json!(["gpt-6-sol"]));
    assert!(
        !attempt.to_string().contains("refresh-1"),
        "no token leaves the API"
    );
}

#[cfg(unix)]
#[test]
fn the_api_cancels_attempts_and_refuses_other_providers() {
    let (env, issuer) = (Env::new(), Issuer::start());
    let origin = issuer.origin.as_str();
    let _server = env.serve(origin);

    let (code, google) = env.api(origin, &["POST", "/providers/google/login"]);
    assert_eq!(code, Some(1));
    assert_eq!(google["_tag"], "ProviderLoginUnsupportedError");

    let (_, started) = env.api(origin, &["POST", "/providers/chatgpt/login"]);
    let attempt = format!(
        "/providers/chatgpt/login/{}",
        started["data"]["attempt_id"].as_str().unwrap()
    );
    let (code, _) = env.api(origin, &["DELETE", &attempt]);
    assert_eq!(code, Some(0));
    let (_, cancelled) = env.api(origin, &["GET", &attempt]);
    assert_eq!(cancelled["data"]["status"], "failed");
    assert_eq!(cancelled["data"]["error"], "login cancelled");
    let (code, missing) = env.api(origin, &["GET", "/providers/chatgpt/login/pla_none"]);
    assert_eq!(code, Some(1));
    assert_eq!(missing["_tag"], "ProviderLoginNotFoundError");
}
