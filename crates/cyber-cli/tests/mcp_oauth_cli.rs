//! `cyber mcp auth` through the real binary: an in-process MCP fixture refuses with a
//! `resource_metadata` challenge, an in-process fake authorization server on 127.0.0.1
//! verifies PKCE and `resource`, and the test plays the browser by fetching the printed
//! authorize URL and calling the loopback callback directly. Nothing reaches the internet.
#[path = "../../cyber-tools/tests/support/mcp_http.rs"]
mod mcp_http;
#[path = "../../cyber-tools/tests/support/oauth_as.rs"]
mod oauth_as;

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    vars: Vec<(&'static str, &'static str)>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("cyber/config")).unwrap();
        Self {
            _dir: dir,
            root,
            vars: Vec::new(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--format", "json"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "oauth-test.db")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(self.vars.iter().copied())
            .current_dir(&self.root)
            .stdin(Stdio::null());
        #[cfg(windows)]
        for name in ["SystemRoot", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn global(&self, value: Value) {
        std::fs::write(
            self.root.join("cyber/config/cyber.jsonc"),
            value.to_string(),
        )
        .unwrap();
    }

    fn project(&self, value: Value) {
        std::fs::write(self.root.join("cyber.jsonc"), value.to_string()).unwrap();
    }

    fn approve_workspace(&self) {
        let inspected = body(&self.run(&["trust", "inspect"]));
        let digest = inspected["digest"].as_str().unwrap();
        assert!(
            self.run(&["trust", "approve", "--digest", digest])
                .status
                .success()
        );
    }

    fn auth_file(&self) -> PathBuf {
        self.root.join("cyber/data/mcp-auth.json")
    }
}

fn body(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The full flow stores credentials, so it runs only where `oauth::open_store` selects the
/// owner-only file (Linux and other non-macOS unix): on macOS and Windows the real binary
/// would write the user's Keychain or Credential Manager. There the production path is
/// covered in process through the app with an injected vault (cyber-app tests/mcp.rs
/// `oauth::`).
#[cfg(not(any(target_os = "macos", windows)))]
mod file_backend {
    use std::io::{BufRead, BufReader, Read};
    use std::path::Path;
    use std::process::{Child, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::{Env, mcp_http, oauth_as};

    /// Waits with a ceiling so a hung flow fails instead of blocking the suite.
    fn finish(mut child: Child) -> (Option<i32>, String) {
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status.code();
            }
            if started.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        (status, stdout)
    }

    /// Collect stderr lines on a thread; the first line that is the authorize URL is sent.
    fn watch_stderr(
        child: &mut Child,
        origin: &str,
    ) -> (mpsc::Receiver<String>, std::thread::JoinHandle<String>) {
        let stderr = child.stderr.take().unwrap();
        let prefix = format!("{origin}/authorize?");
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut all = String::new();
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if line.trim().starts_with(&prefix) {
                    let _ = sender.send(line.trim().to_string());
                }
                all.push_str(&line);
                all.push('\n');
            }
            all
        });
        (receiver, reader)
    }

    /// Run `cyber mcp auth remote`, play the browser (fetch the printed authorize URL from the
    /// fake AS, then call the loopback callback it redirects to) and return
    /// `(exit code, stdout, stderr)`.
    fn authorize_through_binary(
        env: &Env,
        runtime: &tokio::runtime::Runtime,
        origin: &str,
    ) -> (Option<i32>, String, String) {
        let mut child = env
            .command(&["mcp", "auth", "remote"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (urls, reader) = watch_stderr(&mut child, origin);
        let Ok(authorize) = urls.recv_timeout(Duration::from_secs(30)) else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("no authorize URL printed: {}", reader.join().unwrap());
        };
        let (status, location, detail) = runtime.block_on(oauth_as::get(&authorize));
        assert_eq!(status, 302, "{detail}");
        let callback = location.unwrap();
        assert!(callback.starts_with("http://127.0.0.1:"));
        let (status, _, _) = runtime.block_on(oauth_as::get(&callback));
        assert_eq!(status, 200);
        let (code, stdout) = finish(child);
        (code, stdout, reader.join().unwrap())
    }

    /// A registered server stand-in: healthy at this binary's version and recording the
    /// reconnect request `cyber mcp auth` sends after storing the token.
    /// `(server name, authorization, x-cyber-directory)` of each reconnect request.
    type Reconnects =
        std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>, Option<String>)>>>;

    struct Registered {
        requests: Reconnects,
        task: tokio::task::JoinHandle<()>,
    }

    impl Registered {
        /// The report names one reconnect the server answered, sent without any token.
        fn assert_reconnected(&self, report: &Value) {
            assert_eq!(report["reconnect"]["http_status"], 200, "{report}");
            assert_eq!(report["reconnect"]["status"], "connecting", "{report}");
            let requests = self.requests.lock().unwrap().clone();
            assert_eq!(requests.len(), 1, "{requests:?}");
            assert_eq!(requests[0].0, "remote");
            let basic = requests[0]
                .1
                .as_deref()
                .is_some_and(|auth| auth.starts_with("Basic "));
            assert!(basic, "{requests:?}");
            assert!(requests[0].2.is_some());
            self.task.abort();
        }
    }

    fn register(env: &Env, runtime: &tokio::runtime::Runtime) -> Registered {
        use axum::http::HeaderMap;
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = requests.clone();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route(
                "/api/v1/health",
                axum::routing::get(|| async {
                    axum::Json(json!({"healthy":true,"version":cyber_app::version()}))
                }),
            )
            .route(
                "/api/v1/mcp/{name}/reconnect",
                axum::routing::post(
                    move |axum::extract::Path(name): axum::extract::Path<String>,
                          headers: HeaderMap| async move {
                        let header = |key: &str| {
                            headers
                                .get(key)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned)
                        };
                        seen.lock().unwrap().push((
                            name.clone(),
                            header("authorization"),
                            header("x-cyber-directory"),
                        ));
                        axum::Json(json!({"location":{},"data":{"name":name,"configured":true,
                            "status":"connecting","tools":[]}}))
                    },
                ),
            );
        let task = runtime.spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let state = env.root.join("cyber/state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("server.json"),
            json!({"id":"srv_test","version":cyber_app::version(),"url":url,"socket":null,"pid":1})
                .to_string(),
        )
        .unwrap();
        std::fs::write(state.join("password"), "registered-server-password").unwrap();
        Registered { requests, task }
    }

    /// The owner-only credential file holds the token keyed by the exact server URL.
    fn assert_stored(env: &Env, url: &str, origin: &str) {
        use std::os::unix::fs::PermissionsExt;
        let file = env.auth_file();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let entry = &stored["credentials"][0];
        assert_eq!(entry["server"], "remote");
        assert_eq!(entry["url"], url);
        assert_eq!(entry["issuer"], format!("{origin}/"));
        let access = entry["access_token"].as_str().unwrap();
        assert!(access.starts_with(oauth_as::ACCESS_PREFIX));
    }

    #[test]
    fn mcp_auth_acquires_and_stores_a_token_through_the_real_binary() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
        let fake = runtime.block_on(oauth_as::FakeAs::start());
        fake.serve("/prm", fake.protected_resource(&http.url));
        fake.serve_metadata();
        fake.expect_resource(&http.url);
        http.require_authorization(Some("Bearer not-yet-issued"));
        let challenge = format!(
            r#"Bearer resource_metadata="{}/prm", scope="mcp.read""#,
            fake.origin
        );
        http.set_challenge(Some(&challenge));
        let env = Env::new();
        env.global(
            json!({"mcp":{"remote":{"type":"remote","url":http.url,"oauth":{"scopes":["mcp.read"]}}}}),
        );
        let registered = register(&env, &runtime);

        let (code, stdout, stderr) = authorize_through_binary(&env, &runtime, &fake.origin);
        assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
        let report: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(report["name"], "remote");
        assert_eq!(report["authenticated"], true);
        assert_eq!(report["scopes"], json!(["mcp.read"]));
        // The running registered server was asked to reconnect, without any token.
        registered.assert_reconnected(&report);
        // Discovery, registration, authorize and token happened in order with PKCE+resource.
        assert_eq!(
            fake.paths(),
            [
                "/prm",
                "/.well-known/oauth-authorization-server",
                "/register",
                "/authorize",
                "/token"
            ]
        );
        let token = &fake.requests_to("/token")[0];
        assert_eq!(token.form["resource"], http.url);
        assert_eq!(token.form["client_id"], oauth_as::REGISTERED_CLIENT);
        // The MCP server saw only the unauthenticated probe.
        let requests = http.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].status, 401);
        assert_eq!(requests[0].header("authorization"), None);

        assert_stored(&env, &http.url, &fake.origin);
        let config = std::fs::read_to_string(env.root.join("cyber/config/cyber.jsonc")).unwrap();
        for text in [&stdout, &stderr, &config] {
            assert!(!text.contains(oauth_as::ACCESS_PREFIX), "{text}");
            assert!(!text.contains(oauth_as::REFRESH_PREFIX), "{text}");
        }
        // No other file (logs, state) carries the token either.
        assert_no_token_in(&env.root.join("cyber"));
    }

    /// The real app host, as `cyber serve` runs it, registered for the CLI under
    /// `CYBER_HOME`: its HTTP server answers the reconnect `cyber mcp logout` requests.
    struct Served {
        app: cyber_app::App,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    }

    fn serve_app(env: &Env, runtime: &tokio::runtime::Runtime) -> Served {
        let cyber = env.root.join("cyber");
        let paths = cyber_core::paths::Paths {
            data: cyber.join("data"),
            config: cyber.join("config"),
            state: cyber.join("state"),
            cache: cyber.join("cache"),
            tmp: cyber.join("tmp"),
        };
        paths.ensure().unwrap();
        let app = runtime
            .block_on(cyber_app::App::build(cyber_app::AppOptions {
                paths: paths.clone(),
                home: env.root.join("home"),
                database: cyber_core::paths::DatabaseLocation::Memory,
                default_directory: env.root.clone(),
                sandbox_policy: None,
                snapshots: false,
                interactive: true,
                password: Some("logout-test-password".into()),
                // The store `cyber mcp logout` (the binary) selects on this platform.
                mcp_credentials: Some(cyber_tools::mcp::oauth::open_store(&paths)),
                remote_skill_fetcher: None,
                workflow_cores: None,
                isolation: None,
            }))
            .unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = cyber_server::http::router(app.state.clone());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        runtime.spawn(async move {
            let _ = cyber_server::http::serve_tcp(router, listener, async {
                let _ = stopped.await;
            })
            .await;
        });
        std::fs::write(
            paths.state.join("server.json"),
            json!({"id":"srv_logout","version":cyber_app::version(),"url":url,"socket":null,"pid":std::process::id()})
                .to_string(),
        )
        .unwrap();
        std::fs::write(paths.state.join("password"), "logout-test-password").unwrap();
        Served {
            app,
            stop: Some(stop),
        }
    }

    /// The first durable MCP record matching `ready`, waiting up to 15 s.
    fn wait_record(
        served: &Served,
        directory: &Path,
        ready: impl Fn(&cyber_server::runtime::McpConnectionRecord) -> bool,
    ) -> cyber_server::runtime::McpConnectionRecord {
        let started = Instant::now();
        loop {
            let records =
                cyber_server::runtime::mcp_connections(&served.app.store, directory).unwrap();
            if let Some(record) = records.into_iter().find(|record| ready(record)) {
                return record;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "no matching record"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn credential(
        server: &str,
        url: &str,
        origin: &str,
    ) -> (
        cyber_tools::mcp::oauth::CredentialKey,
        cyber_tools::mcp::oauth::StoredCredential,
    ) {
        use cyber_tools::mcp::oauth::{CredentialKey, OAuthClient, StoredCredential, TokenSet};
        (
            CredentialKey {
                server: server.into(),
                url: url.into(),
                issuer: format!("{origin}/"),
            },
            StoredCredential {
                tokens: TokenSet {
                    access_token: format!("{}seed", oauth_as::ACCESS_PREFIX),
                    refresh_token: Some(format!("{}seed", oauth_as::REFRESH_PREFIX)),
                    expires_at: None,
                    issued_at: None,
                    scopes: vec!["mcp.read".into()],
                },
                client: OAuthClient {
                    client_id: oauth_as::REGISTERED_CLIENT.into(),
                    client_secret: None,
                },
                token_endpoint: format!("{origin}/token"),
            },
        )
    }

    #[test]
    fn mcp_logout_removes_every_credential_and_the_running_host_needs_auth_again() {
        use cyber_server::runtime::{McpConnectionPhase, McpConnectionStatus};
        use cyber_tools::mcp::oauth::{FileCredentialStore, McpCredentialStore};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
        let fake = runtime.block_on(oauth_as::FakeAs::start());
        let bearer = format!("Bearer {}seed", oauth_as::ACCESS_PREFIX);
        http.require_authorization(Some(&bearer));
        let env = Env::new();
        env.global(json!({
            "providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}},
            "mcp":{"remote":{"type":"remote","url":http.url}}
        }));
        let store = FileCredentialStore::new(env.auth_file());
        let old = seed(&store, &http.url, &fake.origin);
        let mut served = serve_app(&env, &runtime);
        open_session(&served, &runtime, &env);
        wait_record(&served, &env.root, |record| {
            record.phase == McpConnectionPhase::Running
        });
        assert_eq!(
            http.posts("initialize")[0].header("authorization"),
            Some(bearer.as_str())
        );

        let report = logout(&env);
        assert_eq!(report["name"], "remote");
        assert_eq!(report["removed"], 2, "{report}");
        assert_eq!(report["reconnect"]["http_status"], 200, "{report}");
        assert!(store.find("remote", &http.url).unwrap().is_none());
        assert!(store.find("remote", &old).unwrap().is_none());
        assert!(store.find("other", &http.url).unwrap().is_some());

        // The reconnect sends no bearer and settles needs_auth.
        let record = wait_record(&served, &env.root, |record| {
            record.phase == McpConnectionPhase::Settled
                && record.status == McpConnectionStatus::NeedsAuth
        });
        assert_eq!(record.acknowledged, Some(true));
        let initializes = http.posts("initialize");
        assert_eq!(initializes.len(), 2, "{initializes:?}");
        assert_eq!(initializes[1].header("authorization"), None);
        assert!(fake.seen().is_empty(), "{:?}", fake.paths());
        assert_no_token_in(&env.root.join("cyber"));

        // Logging out again removes nothing, reconnects nothing and still succeeds.
        let again = logout(&env);
        assert_eq!(again["removed"], 0, "{again}");
        assert_eq!(again["reconnect"], Value::Null, "{again}");
        assert_eq!(http.posts("initialize").len(), 2);
        shutdown(&mut served, &runtime, &env);
    }

    /// The same name at its current and a previous URL, from two issuers, plus another
    /// server; the previous URL.
    fn seed(
        store: &cyber_tools::mcp::oauth::FileCredentialStore,
        url: &str,
        origin: &str,
    ) -> String {
        use cyber_tools::mcp::oauth::McpCredentialStore;
        let old = url.replace("/mcp", "/old-mcp");
        for (server, url, origin) in [
            ("remote", url, origin),
            ("remote", old.as_str(), "http://127.0.0.1:9"),
            ("other", url, origin),
        ] {
            let (key, credential) = credential(server, url, origin);
            store.put(&key, &credential).unwrap();
        }
        old
    }

    fn shutdown(served: &mut Served, runtime: &tokio::runtime::Runtime, env: &Env) {
        runtime
            .block_on(served.app.host.close_mcp_location(&env.root))
            .unwrap();
        runtime.block_on(served.app.runtime.shutdown());
        let _ = served.stop.take().unwrap().send(());
    }

    fn open_session(served: &Served, runtime: &tokio::runtime::Runtime, env: &Env) {
        let session = cyber_server::runtime::CreateSession {
            directory: env.root.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        };
        runtime
            .block_on(served.app.runtime.create_session(session))
            .unwrap();
    }

    /// `cyber mcp logout remote` through the binary: its JSON report, with no token in
    /// stdout or stderr.
    fn logout(env: &Env) -> Value {
        let output = env.run(&["mcp", "logout", "remote"]);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(output.status.success(), "{stdout}\n{stderr}");
        for text in [&stdout, &stderr] {
            assert!(!text.contains("TOKEN-SECRET"), "{text}");
        }
        serde_json::from_str(&stdout).unwrap()
    }

    fn assert_no_token_in(root: &Path) {
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                assert_no_token_in(&path);
            } else if path.file_name().is_some_and(|name| name != "mcp-auth.json") {
                let bytes = std::fs::read(&path).unwrap_or_default();
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains(oauth_as::ACCESS_PREFIX),
                    "{}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn mcp_auth_contacts_nothing_for_an_untrusted_project_server() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
    let fake = runtime.block_on(oauth_as::FakeAs::start());
    fake.serve("/prm", fake.protected_resource(&http.url));
    fake.serve_metadata();
    http.require_authorization(Some("Bearer never"));
    http.set_challenge(Some(&format!(
        r#"Bearer resource_metadata="{}/prm""#,
        fake.origin
    )));
    let env = Env::new();
    env.project(json!({"mcp":{"remote":{"type":"remote","url":http.url}}}));
    // Untrusted checkout, then a trusted checkout whose server digest is not approved.
    for approve in [false, true] {
        if approve {
            env.approve_workspace();
        }
        let output = env.run(&["mcp", "auth", "remote"]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("untrusted"), "{stderr}");
        assert!(http.requests().is_empty());
        assert!(fake.seen().is_empty());
        assert!(!env.auth_file().exists());
    }
}

#[test]
fn mcp_auth_refuses_a_disabled_oauth_server_without_contact() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
    let env = Env::new();
    env.global(json!({"mcp":{"remote":{"type":"remote","url":http.url,"oauth":false}}}));
    let output = env.run(&["mcp", "auth", "remote"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("OAuth is disabled"));
    assert!(http.requests().is_empty());
}

#[test]
fn definitions_show_the_oauth_secret_template_never_the_resolved_secret() {
    let mut env = Env::new();
    env.vars.push(("OAUTH_SECRET", "resolved-oauth-secret"));
    env.project(json!({"mcp":{"remote":{"type":"remote","url":"https://mcp.example.test/mcp",
        "oauth":{"client_id":"cyber-cli","client_secret":"{env:OAUTH_SECRET}","scopes":["mcp.read"],"redirect_port":8765}}}}));
    env.approve_workspace();
    let output = env.run(&["mcp", "definitions"]);
    let value = body(&output);
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(!printed.contains("resolved-oauth-secret"));
    let oauth = &value["servers"][0]["definition"]["oauth"];
    assert_eq!(oauth["client_secret"], "{env:OAUTH_SECRET}");
    assert_eq!(oauth["client_id"], "cyber-cli");
    assert_eq!(oauth["scopes"], json!(["mcp.read"]));
    assert_eq!(oauth["redirect_port"], 8765);
    let output = env.run(&["mcp", "get", "remote"]);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("resolved-oauth-secret"));
    assert_eq!(
        body(&output)["definition"]["oauth"]["client_secret"],
        "{env:OAUTH_SECRET}"
    );

    // A global literal secret stays redacted.
    let env = Env::new();
    env.global(
        json!({"mcp":{"remote":{"type":"remote","url":"https://mcp.example.test/mcp",
        "oauth":{"client_id":"cyber-cli","client_secret":"literal-oauth-secret"}}}}),
    );
    let output = env.run(&["mcp", "definitions"]);
    let value = body(&output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("literal-oauth-secret"));
    assert_eq!(
        value["servers"][0]["definition"]["oauth"]["client_secret"],
        "***"
    );
}

#[test]
fn mcp_auth_without_registration_or_client_id_needs_client_registration() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let http = runtime.block_on(mcp_http::McpHttp::start(mcp_http::Behavior::default()));
    let fake = runtime.block_on(oauth_as::FakeAs::start());
    fake.serve("/prm", fake.protected_resource(&http.url));
    fake.serve_metadata_without_registration();
    http.require_authorization(Some("Bearer never"));
    http.set_challenge(Some(&format!(
        r#"Bearer resource_metadata="{}/prm""#,
        fake.origin
    )));
    let env = Env::new();
    env.global(json!({"mcp":{"remote":{"type":"remote","url":http.url}}}));
    let output = env.run(&["mcp", "auth", "remote"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs_client_registration"), "{stderr}");
    assert!(stderr.contains("oauth.client_id"), "{stderr}");
    assert!(!stderr.contains("/authorize"), "{stderr}");
    assert_eq!(
        fake.paths(),
        ["/prm", "/.well-known/oauth-authorization-server"]
    );
    // The condition is recorded for the runtime, which never runs discovery itself: in the
    // file where it is the backend, in the index (by URL digest, no URL) on keyring
    // platforms. Recording it never touches the keyring.
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let stored: Value =
            serde_json::from_str(&std::fs::read_to_string(env.auth_file()).unwrap()).unwrap();
        assert_eq!(stored["credentials"], json!([]));
        assert_eq!(stored["markers"][0]["server"], "remote");
        assert_eq!(stored["markers"][0]["url"], http.url);
        assert_eq!(stored["markers"][0]["needs_client_registration"], true);
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let index = env.root.join("cyber/data/mcp-auth-index.json");
        let text = std::fs::read_to_string(index).unwrap();
        assert!(!text.contains(&http.url), "{text}");
        let stored: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(stored["credentials"], json!([]));
        assert_eq!(stored["markers"][0]["server"], "remote");
        assert_eq!(stored["markers"][0]["needs_client_registration"], true);
        assert!(!env.auth_file().exists());
    }
}

/// Through the platform's selected store; with nothing stored, no keyring entry is read.
#[test]
fn mcp_logout_needs_no_configuration_and_reports_nothing_removed() {
    let env = Env::new();
    let report = body(&env.run(&["mcp", "logout", "absent"]));
    assert_eq!(report["name"], "absent");
    assert_eq!(report["removed"], 0);
    assert_eq!(report["reconnect"], Value::Null);
    // The name becomes a request path segment, so only server-name characters pass.
    let output = env.run(&["mcp", "logout", "../health"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid MCP server name"));
}
