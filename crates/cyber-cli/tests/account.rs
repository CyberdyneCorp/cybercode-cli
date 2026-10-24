//! Real account CLI paths against the signed local OIDC fixture. No test opens a
//! real browser or writes the machine's native credential vault.
#[path = "../../cyber-account/tests/support/oidc.rs"]
mod oidc;
use oidc::Fixture;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

struct Environment {
    _root: tempfile::TempDir,
    root: PathBuf,
}
impl Environment {
    fn new(issuer: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("installation/config")).unwrap();
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::write(root.join("installation/config/cyber.json"),json!({"account":{"issuer":issuer,"client_id":"cyber-cli","audience":"hub","allow_loopback_http":true}}).to_string()).unwrap();
        // The checkout cannot choose an issuer or loosen global account settings.
        std::fs::write(
            root.join("repo/cyber.json"),
            json!({"account":{"issuer":"http://127.0.0.1:9","allow_loopback_http":false}})
                .to_string(),
        )
        .unwrap();
        Self { _root: dir, root }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyber"));
        cmd.args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("installation"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("CYBER_OFFLINE", "1")
            .current_dir(self.root.join("repo"))
            .kill_on_drop(true);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", system);
        }
        cmd
    }
}

async fn headless_login(f: &Fixture, env: &Environment, valid_nonce: bool) -> std::process::Output {
    let mut child = env
        .command(&["login", "--no-browser", "--format", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut errors = BufReader::new(child.stderr.take().unwrap());
    let mut captured = String::new();
    let url = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut line = String::new();
            assert_ne!(
                errors.read_line(&mut line).await.unwrap(),
                0,
                "login ended before authorization: {captured}"
            );
            captured.push_str(&line);
            if let Some(url) = line.strip_prefix("Sign in at ") {
                break url::Url::parse(url.trim()).unwrap();
            }
        }
    })
    .await
    .unwrap();
    let params: HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert!(url.as_str().starts_with(&f.state.issuer));
    let id = f.signing.sign(&f.id_claims(if valid_nonce {
        &params["nonce"]
    } else {
        "wrong-nonce"
    }));
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":id,"refresh_token":"cli-refresh-sentinel","token_type":"Bearer"});
    let callback = format!(
        "{}?code=auth-code&state={}\n",
        params["redirect_uri"], params["state"]
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(callback.as_bytes())
        .await
        .unwrap();
    let mut output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    errors.read_to_string(&mut captured).await.unwrap();
    output.stderr = captured.into_bytes();
    output
}

#[tokio::test]
async fn account_is_optional_and_whoami_does_not_contact_an_issuer_without_a_login() {
    let env = Environment::new("https://must-not-be-contacted.invalid");
    let output = env
        .command(&["whoami", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["logged_in"], false);
    let logout = env
        .command(&["logout", "--all", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(logout.status.success());
}

#[tokio::test]
async fn real_headless_login_uses_global_issuer_and_rejects_an_invalid_id_nonce() {
    let f = Fixture::start().await;
    let env = Environment::new(&f.state.issuer);
    let output = headless_login(&f, &env, false).await;
    assert_eq!(output.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&output.stderr).contains("InvalidTokenError"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("cli-refresh-sentinel"));
    let whoami = env
        .command(&["whoami", "--format", "json"])
        .output()
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&whoami.stdout).unwrap();
    assert_eq!(body["logged_in"], false);
}

// Linux uses the production owner-only fallback. Other platforms exercise the native
// vault layout through injected MemoryVault tests, without touching real Keychains.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn real_login_whoami_and_logout_preserve_provider_credentials_and_local_data() {
    let f = Fixture::start().await;
    let env = Environment::new(&f.state.issuer);
    let output = headless_login(&f, &env, true).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["account"]["sub"], "alice");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("cli-refresh-sentinel"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("account tokens stored in file"));
    let data = env.root.join("installation/data");
    std::fs::write(
        data.join("chatgpt-auth.json"),
        "provider credential sentinel",
    )
    .unwrap();
    std::fs::write(data.join("local-data-sentinel"), "local data").unwrap();
    let output = env
        .command(&["whoami", "--org", "stale-jwt-org", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let output = env
        .command(&["whoami", "--org", "unrelated", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let output = env
        .command(&["logout", "--all", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(data.join("chatgpt-auth.json")).unwrap(),
        "provider credential sentinel"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("local-data-sentinel")).unwrap(),
        "local data"
    );
    let output = env
        .command(&["whoami", "--format", "json"])
        .output()
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["logged_in"], false);
}

#[tokio::test]
async fn another_process_logout_cancels_pending_login_without_touching_the_native_vault() {
    let f = Fixture::start().await;
    let env = Environment::new(&f.state.issuer);
    let gate = std::sync::Arc::new(tokio::sync::Notify::new());
    *f.state.delay.lock().unwrap() = Some(gate.clone());
    let login = headless_login(&f, &env, true);
    let logout = async {
        tokio::time::timeout(Duration::from_secs(20), async {
            while f.state.forms.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let output = env
            .command(&["logout", "--all", "--format", "json"])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        gate.notify_one();
    };
    let (output, ()) = tokio::join!(login, logout);
    assert_eq!(output.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&output.stderr).contains("LoginExpiredError"));
}

#[tokio::test]
async fn logout_uses_matching_running_owner_so_connector_hooks_are_invoked() {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::{delete, get},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let env = Environment::new("http://127.0.0.1:9");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let called = Arc::new(AtomicBool::new(false));
    let observed = called.clone();
    let router = Router::new()
        .route(
            "/api/v1/health",
            get(|| async {
                Json(json!({"healthy":true,"version":"fixture","id":"matching-owner"}))
            }),
        )
        .route(
            "/api/v1/account",
            delete(move |headers: HeaderMap| {
                let observed = observed.clone();
                async move {
                    assert_eq!(
                        headers["authorization"],
                        "Basic Y3liZXI6b3duZXItcGFzc3dvcmQtc2VudGluZWw="
                    );
                    observed.store(true, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let state = env.root.join("installation/state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("server.json"),json!({"id":"matching-owner","version":"fixture","url":base,"socket":null,"pid":std::process::id()}).to_string()).unwrap();
    std::fs::write(state.join("password"), "owner-password-sentinel").unwrap();
    let output = env
        .command(&["logout", "--all", "--format", "json"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(called.load(Ordering::SeqCst));
    task.abort();
}
