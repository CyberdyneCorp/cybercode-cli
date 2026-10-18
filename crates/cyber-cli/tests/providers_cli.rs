//! `cyber providers login|logout chatgpt` through the real binary: an in-process fake OpenAI
//! issuer on 127.0.0.1 answers the code exchange, and the test plays the user by pasting
//! the redirect URL (`--method code`). Nothing reaches the internet.
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
