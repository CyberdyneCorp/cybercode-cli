//! Server lifecycle: password, lock, listeners and registration.

use std::sync::Arc;
use std::time::Duration;

use cyber_app::{App, AppOptions, ServeOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use tokio::sync::Notify;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}

async fn app(root: &std::path::Path) -> App {
    let p = paths(root);
    p.ensure().unwrap();
    App::build(AppOptions {
        paths: p,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("test-password-123456".into()),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn dropping_an_idle_or_stopped_application_releases_its_host_and_database() {
    for stopped in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let application = app(tmp.path()).await;
        let host = Arc::downgrade(&application.host);
        let store = Arc::downgrade(&application.store);
        if stopped {
            application.runtime.shutdown().await;
        }
        drop(application);
        assert!(
            host.upgrade().is_none(),
            "the host outlived its application"
        );
        assert!(
            store.upgrade().is_none(),
            "the database outlived its application"
        );
    }
}

#[cfg(unix)]
#[test]
fn the_password_is_generated_once_and_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let first = cyber_app::password(&p).unwrap();
    assert_eq!(first.len(), 43, "32 bytes in base64url");
    assert_eq!(cyber_app::password(&p).unwrap(), first);
    let mode = std::fs::metadata(p.state.join("password"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test]
async fn a_registered_server_is_healthy_and_unregisters_on_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    #[cfg(not(unix))]
    {
        p.ensure().unwrap();
        std::fs::write(p.state.join("cyber.sock"), "user file").unwrap();
    }
    let stop = Arc::new(Notify::new());
    let opts = ServeOptions {
        port: Some(0),
        socket: cfg!(unix).then(|| tmp.path().join("s.sock")),
        register: true,
        ..ServeOptions::default()
    };
    let server = tokio::spawn(cyber_app::run_server_until(
        app(tmp.path()).await,
        opts,
        |_| {},
        Arc::clone(&stop),
    ));
    let reg = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(r) = cyber_app::read_registration(&p) {
                break r;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("server did not register");
    assert!(cyber_app::health(&reg.url, cyber_app::version()).await);
    assert!(
        !cyber_app::health(&reg.url, "9.9.9").await,
        "a version mismatch is not healthy"
    );
    #[cfg(unix)]
    assert!(tmp.path().join("s.sock").exists());
    #[cfg(not(unix))]
    assert!(reg.socket.is_none());

    // A second server for the same user is refused by the lock.
    let second = cyber_app::run_server_until(
        app(tmp.path()).await,
        ServeOptions {
            port: Some(0),
            no_tcp: cfg!(unix),
            socket: cfg!(unix).then(|| tmp.path().join("t.sock")),
            ..ServeOptions::default()
        },
        |_| {},
        Arc::new(Notify::new()),
    )
    .await;
    assert!(second.unwrap_err().contains("already running"));

    stop.notify_one();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(cyber_app::read_registration(&p).is_none());
    assert!(!tmp.path().join("s.sock").exists());
    #[cfg(not(unix))]
    assert_eq!(
        std::fs::read_to_string(p.state.join("cyber.sock")).unwrap(),
        "user file"
    );
}

/// Regression: `GET /tools` listed only built-in tools, not client-registered ones.
#[tokio::test]
async fn registered_tools_are_listed_with_the_built_ins() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(tmp.path()).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let session = cyber_server::http::rpc::RpcSession::new(&app.state, tx);
    session.handle(r#"{"jsonrpc":"2.0","id":1,"method":"v1.tool.register","params":{"name":"lookup_ticket","description":"Find a ticket"}}"#).await;
    assert_eq!(
        rx.recv().await.unwrap()["result"]["registered"],
        "lookup_ticket"
    );
    session
        .handle(r#"{"jsonrpc":"2.0","id":2,"method":"v1.tool.list","params":{}}"#)
        .await;
    let listed = rx.recv().await.unwrap();
    let names: Vec<&str> = listed["result"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"lookup_ticket") && names.contains(&"read"),
        "{names:?}"
    );
    drop(session);
    assert!(
        app.state.remote_tools.definitions().is_empty(),
        "registrations end with the channel"
    );
}

#[cfg(not(unix))]
#[tokio::test]
async fn unsupported_unix_listener_options_fail_before_readiness() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("user.socket");
    std::fs::write(&marker, "keep me").unwrap();
    for opts in [
        ServeOptions {
            socket: Some(marker.clone()),
            register: true,
            ..ServeOptions::default()
        },
        ServeOptions {
            no_tcp: true,
            register: true,
            ..ServeOptions::default()
        },
    ] {
        let ready = std::sync::atomic::AtomicBool::new(false);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            cyber_app::run_server_until(
                app(tmp.path()).await,
                opts,
                |_| ready.store(true, std::sync::atomic::Ordering::SeqCst),
                Arc::new(Notify::new()),
            ),
        )
        .await
        .expect("unsupported transport did not fail promptly");
        assert!(
            result
                .unwrap_err()
                .contains("Unix socket listeners are unavailable")
        );
        assert!(!ready.load(std::sync::atomic::Ordering::SeqCst));
        assert!(cyber_app::read_registration(&paths(tmp.path())).is_none());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "keep me");
    }
}

async fn registered(
    socket_only: bool,
) -> (
    tempfile::TempDir,
    Paths,
    Arc<Notify>,
    tokio::task::JoinHandle<Result<(), String>>,
    cyber_app::Registration,
    Arc<cyber_server::http::remote_tools::RemoteTools>,
    cyber_server::runtime::Runtime,
) {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let application = app(tmp.path()).await;
    let tools = application.state.remote_tools.clone();
    let runtime = application.runtime.clone();
    std::fs::write(p.state.join("password"), "test-password-123456").unwrap();
    let stop = Arc::new(Notify::new());
    let task = tokio::spawn(cyber_app::run_server_until(
        application,
        ServeOptions {
            port: Some(0),
            no_tcp: socket_only,
            register: true,
            ..ServeOptions::default()
        },
        |_| {},
        Arc::clone(&stop),
    ));
    let reg = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(reg) = cyber_app::read_registration(&p) {
                break reg;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server registration");
    (tmp, p, stop, task, reg, tools, runtime)
}

#[tokio::test]
async fn service_stop_requires_authentication_and_matching_registration_identity() {
    let (_tmp, p, stop, task, reg, _tools, _runtime) = registered(false).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let url = format!("{}/api/v1/service/stop", reg.url);
    let unauthorized = client
        .post(&url)
        .json(&serde_json::json!({"id": reg.id}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    let mismatch = client
        .post(&url)
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&serde_json::json!({"id": "srv_stale"}))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status(), 409);
    let forbidden = client
        .post(&url)
        .basic_auth("cyber", Some("test-password-123456"))
        .header("origin", "https://untrusted.example")
        .json(&serde_json::json!({"id": reg.id}))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
    assert_eq!(cyber_app::read_registration(&p).unwrap().id, reg.id);
    assert!(!task.is_finished());
    stop.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_service_ignores_stale_pid_and_stops_the_registered_server() {
    let (_tmp, p, stop, mut task, mut reg, _tools, _runtime) = registered(false).await;
    // An invalid PID is safe against the old implementation and proves dispatch is identity-bound.
    reg.pid = u32::MAX;
    std::fs::write(
        cyber_app::registration_path(&p),
        serde_json::to_string(&reg).unwrap(),
    )
    .unwrap();
    let result = cyber_app::stop_service(&p).await;
    let finished = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    // Clean up the owned test server even when testing a baseline without the new route.
    if finished.is_err() {
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    assert!(result.is_ok(), "{result:?}");
    assert!(
        finished.is_ok(),
        "registered server continued running after service stop"
    );
    assert_eq!(result.unwrap(), Some(reg));
    assert!(cyber_app::read_registration(&p).is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn socket_only_service_stops_over_its_authenticated_peer_transport() {
    let (_tmp, p, _stop, task, reg, _tools, _runtime) = registered(true).await;
    assert!(reg.url.is_empty());
    assert_eq!(cyber_app::stop_service(&p).await.unwrap(), Some(reg));
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(cyber_app::read_registration(&p).is_none());
}

#[tokio::test]
async fn password_replacement_authenticates_shutdown_with_the_existing_password() {
    let (_tmp, p, _stop, task, _reg, _tools, _runtime) = registered(false).await;
    assert!(
        cyber_app::replace_password(&p, "replacement-password-654321")
            .await
            .unwrap()
    );
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(p.state.join("password")).unwrap(),
        "replacement-password-654321"
    );
    assert!(cyber_app::read_registration(&p).is_none());
}

#[tokio::test]
async fn refused_password_replacement_preserves_credentials_and_registration() {
    let (_tmp, p, stop, task, mut reg, _tools, _runtime) = registered(false).await;
    reg.id = "srv_stale".into();
    std::fs::write(
        cyber_app::registration_path(&p),
        serde_json::to_string(&reg).unwrap(),
    )
    .unwrap();
    let result = cyber_app::replace_password(&p, "replacement-password-654321").await;
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(p.state.join("password")).unwrap(),
        "test-password-123456"
    );
    assert_eq!(cyber_app::read_registration(&p), Some(reg));
    assert!(!task.is_finished());
    stop.notify_waiters();
    stop.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_closes_attached_event_streams_and_websocket_tools() {
    use base64::Engine;
    use futures::{SinkExt, StreamExt};
    let (_tmp, p, stop, mut task, reg, tools, _runtime) = registered(false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .get(format!("{}/api/v1/event", reg.url))
        .basic_auth("cyber", Some("test-password-123456"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut instance = response.bytes_stream();
    tokio::time::timeout(Duration::from_secs(2), instance.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response = client
        .post(format!("{}/api/v1/sessions", reg.url))
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&serde_json::json!({"model":"test/main"}))
        .send()
        .await
        .unwrap();
    let session: serde_json::Value = response.json().await.unwrap();
    let id = session["data"]["id"].as_str().unwrap();
    let response = client
        .get(format!("{}/api/v1/sessions/{id}/events", reg.url))
        .basic_auth("cyber", Some("test-password-123456"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut events = response.bytes_stream();
    tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let token = base64::engine::general_purpose::STANDARD.encode("cyber:test-password-123456");
    let url = format!(
        "{}/api/v1/ws?auth_token={token}",
        reg.url.replace("http://", "ws://")
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"v1.tool.register","params":{"name":"shutdown_tool","description":"owned channel tool"}}).to_string().into()
    )).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        tools
            .definitions()
            .iter()
            .any(|tool| tool.spec.name == "shutdown_tool")
    );
    let result = cyber_app::stop_service(&p).await;
    let finished = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    // Release only these owned clients before reporting a baseline failure.
    if finished.is_err() {
        drop(instance);
        drop(events);
        drop(ws);
        stop.notify_waiters();
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        panic!("service shutdown was held open by its attached clients: {result:?}");
    }
    assert_eq!(result.unwrap(), Some(reg));
    async fn ended<S, T, E>(stream: &mut S)
    where
        S: futures::Stream<Item = Result<T, E>> + Unpin,
        E: std::fmt::Debug,
    {
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(item) = stream.next().await {
                item.unwrap();
            }
        })
        .await
        .expect("event stream remained open");
    }
    ended(&mut instance).await;
    ended(&mut events).await;
    let close = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap();
    assert!(
        close.is_none()
            || matches!(
                close,
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
            ),
        "{close:?}"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while !tools.definitions().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("WebSocket tool registrations survived shutdown");
    assert!(cyber_app::read_registration(&p).is_none());
}

#[tokio::test]
async fn shutdown_finishes_with_a_backpressured_event_connection() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery};
    let (tmp, p, stop, mut task, reg, _tools, runtime) = registered(false).await;
    let mut socket = slow_event_connection(&reg.url).await;
    let id = runtime
        .create_session(CreateSession {
            directory: tmp.path().display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let mut prompt = Admission::text("x".repeat(8 * 1024 * 1024), Delivery::Queue);
    prompt.resume = false;
    runtime.admit(&id, prompt).await.unwrap();
    read_event_prefix(&mut socket, b"event: session.prompt.admitted.1\n").await;
    let started = std::time::Instant::now();
    let stopped = cyber_app::stop_service(&p).await;
    let finished = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    drop(socket);
    if finished.is_err() {
        stop.notify_waiters();
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        panic!("backpressured connection held service shutdown open: {stopped:?}");
    }
    finished.unwrap().unwrap().unwrap();
    assert_eq!(stopped.unwrap(), Some(reg));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(cyber_app::read_registration(&p).is_none());
}

async fn slow_event_connection(url: &str) -> tokio::net::TcpStream {
    use base64::Engine;
    use tokio::io::AsyncWriteExt;
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(1024).unwrap();
    let address = url.trim_start_matches("http://").parse().unwrap();
    let mut socket = socket.connect(address).await.unwrap();
    let token = base64::engine::general_purpose::STANDARD.encode("cyber:test-password-123456");
    socket.write_all(format!("GET /api/v1/event?scope=all HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {token}\r\n\r\n").as_bytes()).await.unwrap();
    let initial = read_event_prefix(&mut socket, b"\n\n\r\n").await;
    assert!(
        initial
            .windows(b"server.connected".len())
            .any(|w| w == b"server.connected")
    );
    socket
}

async fn read_event_prefix(socket: &mut tokio::net::TcpStream, needle: &[u8]) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        loop {
            bytes.push(socket.read_u8().await.unwrap());
            if bytes.ends_with(needle) {
                break bytes;
            }
            assert!(bytes.len() < 16384, "event prefix was not sent");
        }
    })
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn public_worktree_creation_streams_setup_and_retains_failed_session() {
    public_worktree_start(false, false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn public_worktree_creation_uses_approved_source_recipe_without_trusting_target() {
    public_worktree_start(true, true).await;
}

#[cfg(unix)]
#[tokio::test]
async fn public_worktree_creation_leaves_unapproved_source_setup_inactive() {
    public_worktree_start(true, false).await;
}

#[cfg(unix)]
async fn public_worktree_start(project_setup: bool, approve: bool) {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use cyber_server::runtime::{LiveEvent, SetupUpdate};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (repo, p) = worktree_start_fixture(&root, project_setup, approve);
    let runs_setup = !project_setup || approve;
    let application = app(&root).await;
    let mut events = application.runtime.subscribe();
    let client = application.embedded();
    let repo_header = repo.display().to_string();
    let idempotency_key = format!("worktree-public-start-{project_setup}-{approve}");
    let request = Request::builder().method(Method::POST).uri("http://cyber.internal/api/v1/worktrees")
        .header("x-cyber-directory", &repo_header).header("content-type", "application/json")
        .header("idempotency-key", &idempotency_key)
        .body(Body::from(serde_json::json!({"name": "public", "call_id": "call_public", "session": {"model": "test/main", "mode": "dont-ask"}}).to_string())).unwrap();
    let task = tokio::spawn(async move { client.request(request).await });
    if runs_setup {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let LiveEvent::WorktreeSetup {
                    call_id,
                    update: SetupUpdate::Output { .. },
                    ..
                } = events.recv().await.unwrap()
                {
                    assert_eq!(call_id, "call_public");
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            !task.is_finished(),
            "output arrived only after setup finished"
        );
    }
    let response = task.await.unwrap();
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    verify_worktree_start(
        &application,
        &repo_header,
        &p,
        &body,
        runs_setup,
        &idempotency_key,
    )
    .await;
    application.runtime.shutdown().await;
}

#[cfg(unix)]
fn worktree_start_fixture(
    root: &std::path::Path,
    project_setup: bool,
    approve: bool,
) -> (std::path::PathBuf, Paths) {
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Worktree"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repo.join("tracked"), "base").unwrap();
    for args in [
        vec!["add", "tracked"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let p = paths(root);
    p.ensure().unwrap();
    // Bash leaves masked HOME absent; zsh synthesizes it from the account record.
    std::fs::write(
        p.config.join("cyber.jsonc"),
        serde_json::json!({"shell": "bash"}).to_string(),
    )
    .unwrap();
    let config_file = if project_setup {
        repo.join("cyber.jsonc")
    } else {
        p.config.join("cyber.jsonc")
    };
    std::fs::write(config_file, serde_json::json!({"shell": "bash", "providers": {"local": {"disabled": true, "env": ["HOME"]}}, "worktrees": {"setup": ["if [ \"${HOME+x}\" ]; then exit 8; fi; printf ready; sleep 1; printf once >> setup-result; exit 7"]}}).to_string()).unwrap();
    if project_setup {
        for args in [
            vec!["add", "cyber.jsonc"],
            vec!["commit", "--quiet", "-m", "setup fixture"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    if approve {
        let home = root.join("home");
        let report = cyber_core::config::trust_report(&cyber_core::config::LoadRequest {
            location: &repo,
            paths: &p,
            home: &home,
            env: &cyber_core::env::ProcessEnv,
            profile: None,
            overrides: &[],
            flags: Default::default(),
        })
        .unwrap();
        cyber_core::trust::TrustStore::new(p.trust_file())
            .approve(&report.checkout_root, report.digest.as_deref().unwrap())
            .unwrap();
    }
    (repo, p)
}

#[cfg(unix)]
async fn verify_worktree_start(
    application: &App,
    repo_header: &str,
    p: &Paths,
    body: &serde_json::Value,
    runs_setup: bool,
    idempotency_key: &str,
) {
    assert_eq!(
        body["data"]["setup"]["status"],
        if runs_setup { "failed" } else { "completed" }
    );
    if runs_setup {
        assert_eq!(body["data"]["setup"]["code"], 7);
    }
    assert_eq!(body["data"]["session"]["mode"], "dont-ask");
    let directory = body["data"]["session"]["directory"].as_str().unwrap();
    assert_eq!(body["location"]["directory"], directory);
    assert_eq!(body["data"]["worktree"]["path"], directory);
    let result_file = std::path::Path::new(directory).join("setup-result");
    if runs_setup {
        assert_eq!(std::fs::read_to_string(result_file).unwrap(), "once");
    } else {
        assert!(!result_file.exists());
    }
    assert!(
        cyber_core::trust::TrustStore::new(p.trust_file())
            .approval(std::path::Path::new(directory))
            .unwrap()
            .is_none()
    );
    verify_worktree_replay(application, repo_header, body, idempotency_key).await;
    verify_worktree_listing(application, repo_header, body, runs_setup).await;
}

#[cfg(unix)]
async fn verify_worktree_listing(
    application: &App,
    repo_header: &str,
    created: &serde_json::Value,
    dirty: bool,
) {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    let source = std::path::Path::new(created["data"]["worktree"]["path"].as_str().unwrap());
    let target_header = source.display().to_string();
    let session_count = if dirty {
        1
    } else {
        add_listing_sessions(
            application,
            &target_header,
            created["data"]["session"]["id"].as_str().unwrap(),
        )
        .await
    };
    for header in [repo_header, &target_header] {
        let response = application
            .embedded()
            .request(
                Request::builder()
                    .method(Method::GET)
                    .uri("http://cyber.internal/api/v1/worktrees")
                    .header("x-cyber-directory", header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let entries = listed["data"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_listing_entry(&entries[0], created, dirty, session_count);
    }
}

#[cfg(unix)]
fn assert_listing_entry(
    entry: &serde_json::Value,
    created: &serde_json::Value,
    dirty: bool,
    session_count: usize,
) {
    assert_eq!(entry["status"], "ready");
    assert_eq!(entry["worktree"], created["data"]["worktree"]);
    assert_eq!(entry["dirty"], dirty);
    assert_eq!(entry["ahead"], 0);
    assert_eq!(entry["behind"], 0);
    let sessions = entry["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), session_count);
    assert!(
        sessions
            .iter()
            .any(|row| row["id"] == created["data"]["session"]["id"])
    );
    if !dirty {
        assert!(sessions.iter().any(|row| row["archived"] == true));
    }
}

#[cfg(unix)]
async fn add_listing_sessions(application: &App, directory: &str, parent: &str) -> usize {
    for index in 0..201 {
        let session = application
            .runtime
            .create_session(cyber_server::runtime::CreateSession {
                directory: directory.into(),
                model: "test/main".into(),
                parent_id: Some(parent.into()),
                ..Default::default()
            })
            .await
            .unwrap();
        if index == 0 {
            application
                .runtime
                .archive(&session.id, true)
                .await
                .unwrap();
        }
    }
    202
}

#[cfg(unix)]
async fn verify_worktree_replay(
    application: &App,
    repo_header: &str,
    body: &serde_json::Value,
    idempotency_key: &str,
) {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    let source_sessions = application
        .embedded()
        .request(
            Request::builder()
                .method(Method::GET)
                .uri("http://cyber.internal/api/v1/sessions")
                .header("x-cyber-directory", repo_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let source_sessions: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(source_sessions.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        source_sessions["data"]["data"]
            .as_array()
            .unwrap()
            .is_empty(),
        "startup created an extra source Session"
    );
    // Replaying the same API request retains both the Session and its failed result.
    let replay = application.embedded().request(Request::builder().method(Method::POST).uri("http://cyber.internal/api/v1/worktrees")
        .header("x-cyber-directory", repo_header).header("content-type", "application/json").header("idempotency-key", idempotency_key)
        .body(Body::from(serde_json::json!({"name": "public", "call_id": "call_public", "session": {"model": "test/main", "mode": "dont-ask"}}).to_string())).unwrap()).await;
    let replayed: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(replay.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(&replayed, body);
}
