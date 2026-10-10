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
async fn imported_static_commands_reach_live_catalogue_and_expansion_with_skill_precedence() {
    let tmp = tempfile::tempdir().unwrap();
    let application = app(tmp.path()).await;
    let mut converted = cyber_core::import::opencode_settings_config(&serde_json::json!({"command":{
        "db/migrate":{"template":"Migrate $1 in $2","description":"Migration","argument_hint":"<step> <targets>"},
        "goal":{"template":"Project goal $ARGUMENTS"},
        "review":{"template":"Configured review"},
        "unsupported":{"template":"Review","agent":"explore"}
    }})).unwrap();
    converted.config["commands"]["markdown/review.rs"] = cyber_core::commands::markdown(
        "---\ndescription: Markdown review\nargument-hint: '<paths>'\n---\nReview $ARGUMENTS",
    )
    .unwrap()
    .definition;
    std::fs::write(
        application.paths.config.join("cyber.json"),
        serde_json::to_vec(&converted.config).unwrap(),
    )
    .unwrap();
    let skill = tmp.path().join(".cyber/skills/review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Skill review\n---\nSkill review $ARGUMENTS",
    )
    .unwrap();
    let reserved = tmp.path().join(".cyber/skills/mode");
    std::fs::create_dir_all(&reserved).unwrap();
    std::fs::write(
        reserved.join("SKILL.md"),
        "---\nname: mode\ndescription: Project mode skill\n---\nProject mode $ARGUMENTS",
    )
    .unwrap();
    let services = &application.state.services;
    let commands = services.commands(tmp.path());
    let entry = commands.iter().find(|c| c.name == "db/migrate").unwrap();
    assert_eq!(entry.source, "command");
    assert_eq!(entry.argument_hint.as_deref(), Some("<step> <targets>"));
    let markdown = commands
        .iter()
        .find(|c| c.name == "markdown/review.rs")
        .unwrap();
    assert_eq!(markdown.description, "Markdown review");
    assert_eq!(markdown.argument_hint.as_deref(), Some("<paths>"));
    assert_eq!(
        services
            .expand_command(tmp.path(), "markdown/review.rs", "src")
            .as_deref(),
        Some("Review src")
    );
    assert!(!commands.iter().any(|c| c.name == "unsupported"));
    assert_eq!(commands.iter().filter(|c| c.name == "review").count(), 1);
    assert_eq!(
        services
            .expand_command(tmp.path(), "db/migrate", "42 \"auth module\" more")
            .as_deref(),
        Some("Migrate 42 in auth module more")
    );
    assert_eq!(
        services
            .expand_command(tmp.path(), "review", "code")
            .as_deref(),
        Some("Skill review code")
    );
    assert!(services.expand_command(tmp.path(), "goal", "fix").is_none());
    assert_eq!(
        services
            .expand_command(tmp.path(), "project:goal", "fix")
            .as_deref(),
        Some("Project goal fix")
    );
    assert!(
        commands
            .iter()
            .any(|c| c.name == "mode" && c.source == "builtin")
    );
    assert!(
        commands
            .iter()
            .any(|c| c.name == "project:mode" && c.source == "skill")
    );
    assert!(services.expand_command(tmp.path(), "mode", "fix").is_none());
    assert_eq!(
        services
            .expand_command(tmp.path(), "project:mode", "fix")
            .as_deref(),
        Some("Project mode fix")
    );
    application.runtime.shutdown().await;
}

#[tokio::test]
async fn agent_catalogue_resolves_builtins_and_live_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let application = app(tmp.path()).await;
    let names = |agents: Vec<cyber_server::http::AgentInfo>| {
        agents
            .into_iter()
            .map(|agent| agent.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(application.state.services.agents(tmp.path())),
        ["build", "explore", "general"]
    );
    std::fs::write(
        application.paths.config.join("cyber.json"),
        r#"{"agents":{
        "explore":{"disabled":true},
        "general":{"description":"Configured general agent"},
        "docs":{"description":"Write documentation"},
        "hidden":{"hidden":true},
        "title":{"hidden":false,"disabled":true},
        "max_concurrent":4
    }}"#,
    )
    .unwrap();
    let agents = application.state.services.agents(tmp.path());
    assert_eq!(agents[1].description, "Configured general agent");
    assert_eq!(agents[2].mode, "all");
    assert_eq!(names(agents), ["build", "general", "docs"]);
    std::fs::write(
        application.paths.config.join("cyber.json"),
        r#"{"agents":{"docs":{"unknown":true}}}"#,
    )
    .unwrap();
    assert!(application.state.services.agents(tmp.path()).is_empty());
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
    add_stale_listing_sessions(application, created).await;
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
async fn add_stale_listing_sessions(application: &App, created: &serde_json::Value) {
    let current = application
        .runtime
        .state(created["data"]["session"]["id"].as_str().unwrap())
        .await
        .unwrap();
    for binding in [None, Some("wt_old_creation".to_owned())] {
        let mut info = current.info.clone();
        info.id = cyber_core::ids::new_id("ses");
        info.worktree_id = binding;
        let id = info.id.clone();
        application
            .store
            .append(
                &id,
                cyber_store::Expected::Seq(-1),
                vec![cyber_store::NewEvent::new(
                    "session.created.1",
                    serde_json::json!({"info": info}),
                )],
            )
            .unwrap();
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

#[cfg(unix)]
#[tokio::test]
async fn child_setup_recovery_api_preserves_steps_and_idempotent_retry() {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use cyber_server::runtime::{Asker, CreateSession, Invocation, ToolHost, ToolOutcome};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (repo, p) = worktree_start_fixture(&root, false, false);
    std::fs::write(p.config.join("cyber.jsonc"), serde_json::json!({"shell":"bash","providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}},"permissions":{"agent":"allow","worktree":"allow"},"worktrees":{"setup":["printf once >> prefix.txt","printf attempt >> attempts.txt; test -f allow-retry"],"cleanup":"keep"}}).to_string()).unwrap();
    let application = App::build(AppOptions {
        paths: p,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: repo.clone(),
        sandbox_policy: Some("full-access".into()),
        snapshots: false,
        interactive: false,
        password: Some("test-password-123456".into()),
    })
    .await
    .unwrap();
    let parent = application
        .runtime
        .create_session(CreateSession {
            directory: repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let result = application.host.execute(Invocation {
        registration: None,
        session_id:parent.id.clone(),directory:parent.directory.clone(),agent:parent.agent.clone(),mode:parent.mode.clone(),rules:parent.rules.clone(),
        message_id:"msg_setup".into(),call_id:"call_setup".into(),operation_key:"call_setup".into(),name:"agent".into(),
        input:serde_json::json!({"prompt":"inspect","name":"api-retry","isolation":"worktree"}),attempt:1,asker:Asker::detached(),
    },tokio_util::sync::CancellationToken::new()).await;
    assert!(
        matches!(&result, ToolOutcome::Failed(message) if message.contains("setup")),
        "{result:?}"
    );
    let child = application
        .runtime
        .resolve_subagent(&parent.id, "api-retry")
        .await
        .unwrap();
    let url = format!(
        "http://cyber.internal/api/v1/sessions/{}/children/{}/setup",
        parent.id, child.id
    );
    let inspect = application
        .embedded()
        .request(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await;
    assert_eq!(inspect.status(), StatusCode::OK);
    let snapshot: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(inspect.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["data"]["setup_pending"], true);
    assert_eq!(
        snapshot["data"]["journal"]["commands"][1]["result"]["code"],
        1
    );
    std::fs::write(
        std::path::Path::new(&child.directory).join("allow-retry"),
        "ready",
    )
    .unwrap();
    let review = serde_json::json!({"revision":snapshot["data"]["journal"]["revision"],"digest":snapshot["data"]["journal"]["digest"],"retry_index":1,"reason":"Prerequisite repaired; retry explicitly approved"});
    let mut responses = Vec::new();
    for _ in 0..2 {
        let response = application
            .embedded()
            .request(
                Request::builder()
                    .method(Method::POST)
                    .uri(&url)
                    .header("content-type", "application/json")
                    .header("idempotency-key", "setup-recovery-review")
                    .body(Body::from(review.to_string()))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        responses.push(
            serde_json::from_slice::<serde_json::Value>(
                &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap(),
            )
            .unwrap(),
        );
    }
    assert_eq!(responses[0], responses[1]);
    assert_eq!(responses[0]["data"]["setup"]["status"], "completed");
    assert_eq!(responses[0]["data"]["session"]["id"], child.id);
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&child.directory).join("prefix.txt")).unwrap(),
        "once"
    );
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&child.directory).join("attempts.txt"))
            .unwrap(),
        "attemptattempt"
    );
    assert!(
        !application
            .runtime
            .state(&child.id)
            .await
            .unwrap()
            .child_worktree_setup_pending()
    );
    assert!(!application.runtime.is_running(&child.id));
    let repeated = application
        .embedded()
        .request(
            Request::builder()
                .method(Method::POST)
                .uri(&url)
                .header("content-type", "application/json")
                .body(Body::from(review.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(repeated.status(), StatusCode::CONFLICT);
    application.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn application_reloads_command_hooks_at_the_builtin_boundary() {
    use cyber_server::runtime::{Asker, CreateSession, Invocation, ToolHost, ToolOutcome};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let application = app(&root).await;
    let file = application.paths.config.join("cyber.json");
    let config = serde_json::json!({"providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}}});
    std::fs::write(&file, config.to_string()).unwrap();
    let session = application
        .runtime
        .create_session(CreateSession {
            directory: root.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut changed = config;
    changed["hooks"] = serde_json::json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"command","id":"app-guard","command":"cat >/dev/null; printf '%s' '{\"decision\":\"deny\",\"reason\":\"live guard\"}'"}]}]});
    std::fs::write(&file, changed.to_string()).unwrap();
    let outcome = application
        .host
        .execute(
            Invocation {
                registration: None,
                session_id: session.id.clone(),
                directory: session.directory.clone(),
                agent: session.agent.clone(),
                mode: session.mode.clone(),
                rules: session.rules.clone(),
                message_id: "msg_hook".into(),
                call_id: "call_hook".into(),
                operation_key: "call_hook".into(),
                name: "write".into(),
                input: serde_json::json!({"path":"blocked.txt","content":"unsafe"}),
                attempt: 1,
                asker: Asker::detached(),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&outcome, ToolOutcome::Failed(message) if message.contains("live guard")),
        "{outcome:?}"
    );
    assert!(!root.join("blocked.txt").exists());
    let records = application
        .runtime
        .hook_executions(&session.id, 10)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hook_id, "app-guard");
    assert!(records[0].io.is_none());
}

async fn hook_catalog(
    client: &reqwest::Client,
    url: &str,
    directory: &std::path::Path,
) -> serde_json::Value {
    let response = client
        .get(url)
        .header("x-cyber-directory", directory.display().to_string())
        .basic_auth("cyber", Some("test-password-123456"))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    response.json().await.unwrap()
}

async fn hook_trust_request(
    client: &reqwest::Client,
    url: &str,
    directory: &std::path::Path,
    operation: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    client
        .post(format!("{url}/{operation}"))
        .header("x-cyber-directory", directory.display().to_string())
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn hook_catalog_api_reloads_location_trust_and_redacts_without_execution() {
    use cyber_core::trust::TrustStore;
    use serde_json::json;
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    p.ensure().unwrap();
    std::fs::write(p.config.join("cyber.jsonc"),json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"http","url":"http://127.0.0.1:9/never","headers":{"Authorization":"private-token"}}]}]}}).to_string()).unwrap();
    let repo = tmp.path().join("checkout A");
    let nested = repo.join("nested");
    let other = tmp.path().join("checkout B");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let file = repo.join("cyber.jsonc");
    std::fs::write(&file,json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo unsafe > effect","id":"project-guard"}]}]}}).to_string()).unwrap();
    let application = app(tmp.path()).await;
    let before: i64 = application
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1/hooks", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(application.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let withheld = hook_catalog(&client, &url, &nested).await;
    assert_eq!(withheld["data"]["hooks"].as_array().unwrap().len(), 1);
    assert_eq!(withheld["data"]["checkout_trusted"], false);
    assert_eq!(withheld["data"]["withheld_hooks"][0]["scope"], "project");
    assert_eq!(
        withheld["data"]["withheld_hooks"][0]["value"]["PreToolUse"][0]["hooks"][0]["command"],
        "echo unsafe > effect"
    );
    assert!(
        withheld["data"]["withheld_hooks"][0]
            .get("digest")
            .is_none()
    );

    assert!(
        !withheld["data"]["withheld_definitions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!withheld.to_string().contains("private-token"));
    assert_eq!(
        withheld["data"]["hooks"][0]["handler"]["headers"]["Authorization"],
        "***"
    );
    let trust = TrustStore::new(p.trust_file());
    let home = tmp.path().join("home");
    let request = cyber_core::config::LoadRequest {
        location: &nested,
        paths: &p,
        env: &cyber_core::env::ProcessEnv,
        home: &home,
        profile: None,
        overrides: &[],
        flags: json!({}),
    };
    let resolved = cyber_core::config::load(&request).unwrap();
    let original = cyber_core::hooks::HookCatalog::from_config(&resolved).unwrap();
    assert_eq!(
        withheld["data"]["hooks"][0]["digest"],
        original.definitions[0].digest
    );
    assert_eq!(
        original.definitions[0].handler.headers["Authorization"],
        "private-token"
    );
    let report = cyber_core::config::trust_report(&request).unwrap();
    trust
        .approve(&report.checkout_root, report.digest.as_ref().unwrap())
        .unwrap();
    let review = hook_catalog(&client, &url, &nested).await;
    assert_eq!(
        review["location"]["directory"],
        nested.canonicalize().unwrap().display().to_string()
    );
    assert!(
        review["data"]["withheld_hooks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let hooks = review["data"]["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0]["scope"], "global");
    assert_eq!(hooks[1]["scope"], "project");
    assert_eq!(hooks[1]["trusted"], false);
    assert_eq!(hooks[1]["sandbox_required"], true);
    let digest = hooks[1]["digest"].as_str().unwrap();
    trust.revoke(&report.checkout_root).unwrap();
    assert_eq!(
        hook_trust_request(&client, &url, &nested, "trust", json!({"digest":digest}))
            .await
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    trust
        .approve(&report.checkout_root, report.digest.as_ref().unwrap())
        .unwrap();

    assert_eq!(
        client
            .post(format!("{url}/trust"))
            .json(&json!({"digest":digest}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    for body in [
        json!({"digest":digest,"extra":true}),
        json!({}),
        json!({"digest":hooks[0]["digest"]}),
        json!({"digest":"invalid"}),
    ] {
        assert_eq!(
            hook_trust_request(&client, &url, &nested, "trust", body)
                .await
                .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        hook_trust_request(&client, &url, &other, "trust", json!({"digest":digest}))
            .await
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let approved =
        hook_trust_request(&client, &url, &nested, "trust", json!({"digest":digest})).await;
    assert_eq!(approved.status(), reqwest::StatusCode::OK);
    assert_eq!(
        approved.json::<serde_json::Value>().await.unwrap()["data"]["digest"],
        digest
    );
    assert_eq!(
        hook_catalog(&client, &url, &nested).await["data"]["hooks"][1]["trusted"],
        true
    );
    let revoked =
        hook_trust_request(&client, &url, &nested, "untrust", json!({"digest":digest})).await;
    assert_eq!(revoked.status(), reqwest::StatusCode::OK);
    assert_eq!(
        revoked.json::<serde_json::Value>().await.unwrap()["data"]["revoked"],
        true
    );
    assert_eq!(
        hook_catalog(&client, &url, &nested).await["data"]["hooks"][1]["trusted"],
        false
    );
    assert_eq!(
        hook_catalog(&client, &url, &other).await["data"]["hooks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hook_trust_request(&client, &url, &nested, "trust", json!({"digest":digest}))
            .await
            .status(),
        reqwest::StatusCode::OK
    );
    std::fs::write(&file, "{ malformed").unwrap();
    assert_eq!(
        hook_trust_request(&client, &url, &nested, "trust", json!({"digest":digest}))
            .await
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let revoked =
        hook_trust_request(&client, &url, &nested, "untrust", json!({"digest":digest})).await;
    assert_eq!(revoked.status(), reqwest::StatusCode::OK);
    assert_eq!(
        revoked.json::<serde_json::Value>().await.unwrap()["data"]["revoked"],
        true
    );
    assert_eq!(
        hook_trust_request(&client, &url, &nested, "untrust", json!({"digest":digest}))
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap()["data"]["revoked"],
        false
    );
    std::fs::write(&file, "{}").unwrap();
    assert_eq!(
        hook_trust_request(&client, &url, &nested, "trust", json!({"digest":digest}))
            .await
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let changed = hook_catalog(&client, &url, &nested).await;
    assert_eq!(changed["data"]["hooks"].as_array().unwrap().len(), 1);
    assert!(!nested.join("effect").exists());
    let after: i64 = application
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(
        before, after,
        "catalog inspection must not create Session or hook events"
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn hook_catalog_api_last_run_preserves_outcome_without_opted_in_io() {
    use cyber_core::hooks::{HookCatalog, HookEvent, HookIdentity, HookLocation, HookOutcome};
    use cyber_server::runtime::{CreateSession, HookExecutionIo, HookExecutionResult};
    let tmp = tempfile::tempdir().unwrap();
    let application = app(tmp.path()).await;
    std::fs::write(application.paths.config.join("cyber.jsonc"),serde_json::json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo reviewed"}]}]}}).to_string()).unwrap();
    let loader = cyber_app::hook_config_loader(application.paths.clone(), tmp.path().join("home"));
    let definition = HookCatalog::from_config(&loader(tmp.path()).unwrap())
        .unwrap()
        .definitions
        .remove(0);
    let session = application
        .runtime
        .create_session(CreateSession {
            directory: tmp.path().display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let event = HookEvent::new(
        "PreToolUse",
        HookIdentity {
            session_id: session,
            location: HookLocation {
                directory: tmp.path().to_path_buf(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "build".into(),
            mode: "default".into(),
        },
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    let record = application
        .runtime
        .start_hook_execution(&event, &definition, true)
        .await
        .unwrap()
        .finish(HookExecutionResult {
            outcome: HookOutcome::Blocked,
            decision: Default::default(),
            acknowledged: true,
            must_stop: false,
            io: Some(HookExecutionIo {
                stdin: "private stdin".into(),
                stdout: "private stdout".into(),
                stderr: "private stderr".into(),
                truncated: false,
            }),
        })
        .unwrap();
    let before: i64 = application
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1/hooks", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(application.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let review = hook_catalog(&client, &url, tmp.path()).await;
    assert_eq!(review["data"]["hooks"][0]["last_run"]["id"], record.id);
    assert_eq!(review["data"]["hooks"][0]["last_run"]["outcome"], "blocked");
    for secret in ["private stdin", "private stdout", "private stderr"] {
        assert!(!review.to_string().contains(secret));
    }
    assert!(review["data"]["hooks"][0]["last_run"].get("io").is_none());
    let after: i64 = application
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(before, after);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn application_runtime_owns_configured_mcp_startup_and_shutdown() {
    use cyber_server::runtime::{CreateSession, McpConnectionPhase, mcp_connections};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let application = app(&root).await;
    let server = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 result=({'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'app','version':'1'}} if r['method']=='initialize' else {'tools':[{'name':'read','inputSchema':{'type':'object'}}]})
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#;
    std::fs::write(application.paths.config.join("cyber.json"),serde_json::json!({"sandbox":{"policy":"full-access"},"providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}},"mcp":{"app":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",server]}}}).to_string()).unwrap();
    application
        .runtime
        .create_session(CreateSession {
            directory: root.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mcp_connections(&application.state.store, &root)
                .unwrap()
                .iter()
                .any(|record| record.phase == McpConnectionPhase::Running)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    application.runtime.shutdown().await;
    assert_eq!(
        mcp_connections(&application.state.store, &root).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
}

#[tokio::test]
async fn native_markdown_commands_reach_live_catalogue_and_refresh_after_edits() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let application = app(tmp.path()).await;
    let location = tmp.path().join("nested");
    let commands = location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    let global = application.paths.config.join("commands");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(global.join("review.md"), "Global review").unwrap();
    std::fs::write(
        commands.join("review.md"),
        "---\ndescription: Native review\nargument-hint: '<paths>'\n---\nReview $ARGUMENTS",
    )
    .unwrap();
    std::fs::write(commands.join("mode.md"), "Project mode $ARGUMENTS").unwrap();
    std::fs::write(commands.join("shell.md"), "!`touch never-created-sentinel`").unwrap();
    let compat = location.join(".claude/commands");
    std::fs::create_dir_all(&compat).unwrap();
    std::fs::write(compat.join("compat.md"), "Compatibility $ARGUMENTS").unwrap();
    let services = &application.state.services;
    let rows = services.commands(&location);
    let review = rows.iter().find(|c| c.name == "review").unwrap();
    assert_eq!(review.source, "command");
    assert_eq!(review.argument_hint.as_deref(), Some("<paths>"));
    let provenance = review.provenance.as_ref().unwrap();
    assert_eq!(
        provenance.winner.paths,
        [commands.join("review.md").canonicalize().unwrap()]
    );
    assert_eq!(
        provenance.winner.scope,
        cyber_core::commands::CommandSourceScope::Project
    );
    let mode = rows.iter().find(|row| row.name == "project:mode").unwrap();
    assert_eq!(mode.namespace.as_deref(), Some("project"));
    let serialized = serde_json::to_value(review).unwrap();
    assert_eq!(serialized["provenance"]["winner"]["scope"], "project");
    assert!(serialized.get("template").is_none());

    assert!(!rows.iter().any(|c| c.name == "shell"));
    assert!(
        rows.iter()
            .any(|c| c.name == "mode" && c.source == "builtin")
    );
    assert!(
        rows.iter()
            .any(|c| c.name == "project:mode" && c.source == "command")
    );
    assert_eq!(
        services
            .expand_command(&location, "review", "src")
            .as_deref(),
        Some("Review src")
    );
    assert_eq!(
        services
            .expand_command(&location, "compat", "src")
            .as_deref(),
        Some("Compatibility src")
    );
    assert_eq!(
        services
            .expand_command(&location, "project:mode", "test")
            .as_deref(),
        Some("Project mode test")
    );
    std::fs::write(commands.join("review.md"), "Updated review $ARGUMENTS").unwrap();
    assert_eq!(
        services
            .expand_command(&location, "review", "src")
            .as_deref(),
        Some("Updated review src")
    );
    std::fs::write(
        commands.join("review.md"),
        "---\nagent: explore\n---\nUnsupported new review",
    )
    .unwrap();
    assert!(
        services
            .expand_command(&location, "review", "src")
            .is_none()
    );
    assert!(!location.join("never-created-sentinel").exists());
    application.runtime.shutdown().await;
}

#[tokio::test]
async fn commands_resolve_real_loader_scope_labels_before_markdown_precedence() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let application = app(tmp.path()).await;
    let location = tmp.path().join("nested");
    let commands = location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        application.paths.config.join("cyber.json"),
        r#"{"commands":{"review":{"template":"Global inline"}}}"#,
    )
    .unwrap();
    std::fs::write(commands.join("review.md"), "Nearest Markdown").unwrap();
    assert_eq!(
        application
            .state
            .services
            .expand_command(&location, "review", ""),
        Some("Nearest Markdown".into())
    );
    let rows = application.state.services.commands(&location);
    let review = rows.iter().find(|row| row.name == "review").unwrap();
    let provenance = review.provenance.as_ref().unwrap();
    assert_eq!(
        provenance.shadowed[0].scope,
        cyber_core::commands::CommandSourceScope::Global
    );
    assert_eq!(
        provenance.shadowed[0].paths,
        [application.paths.config.join("cyber.json")]
    );
    // An ancestor inline definition must not mask a nearer unsupported definition.
    std::fs::write(
        tmp.path().join("cyber.json"),
        r#"{"commands":{"review":{"template":"Ancestor inline"}}}"#,
    )
    .unwrap();
    std::fs::write(
        commands.join("review.md"),
        "---\nagent: reviewer\n---\nUnsupported",
    )
    .unwrap();
    assert!(
        application
            .state
            .services
            .expand_command(&location, "review", "")
            .is_none()
    );
    application.runtime.shutdown().await;
}

#[tokio::test]
async fn command_catalog_api_exposes_location_provenance_without_template_values() {
    let tmp = tempfile::tempdir().unwrap();
    let application = app(tmp.path()).await;
    let location = tmp.path().join("checkout/nested");
    std::fs::create_dir_all(tmp.path().join("checkout/.git")).unwrap();
    let commands = location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        application.paths.config.join("cyber.json"),
        r#"{"commands":{"review":{"template":"private-global-body"}}}"#,
    )
    .unwrap();
    std::fs::write(commands.join("review.md"), "private-project-body").unwrap();
    std::fs::write(commands.join("mode.md"), "private-mode-body").unwrap();
    std::fs::write(
        commands.join("blocked.md"),
        "---\nagent: reviewer\n---\nprivate-blocked-body",
    )
    .unwrap();
    let before: i64 = application
        .store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1/commands", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(application.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let response = hook_catalog(&client, &url, &location).await;
    let rows = response["data"].as_array().unwrap();
    let review = rows.iter().find(|row| row["name"] == "review").unwrap();
    assert_eq!(review["provenance"]["winner"]["scope"], "project");
    assert_eq!(
        review["provenance"]["winner"]["paths"][0],
        commands
            .join("review.md")
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(review["provenance"]["shadowed"][0]["scope"], "global");
    assert_eq!(
        rows.iter()
            .find(|row| row["name"] == "project:mode")
            .unwrap()["namespace"],
        "project"
    );
    assert!(!rows.iter().any(|row| row["name"] == "blocked"));
    assert!(!response.to_string().contains("private-"));
    let elsewhere = hook_catalog(&client, &url, tmp.path()).await;
    let global = elsewhere["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "review")
        .unwrap();
    assert_eq!(global["provenance"]["winner"]["scope"], "global");
    assert_eq!(
        application
            .store
            .read(|conn| Ok(
                conn.query_row("SELECT count(*) FROM event", [], |row| row.get::<_, i64>(0))?
            ))
            .unwrap(),
        before
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    application.runtime.shutdown().await;
}

#[tokio::test]
async fn authenticated_skill_commands_capture_declarations_and_refuse_denials() {
    use serde_json::json;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let application = app(tmp.path()).await;
    let mut config = json!({"sandbox":{"policy":"full-access"},"providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{},"skill":{}}}}});
    std::fs::write(
        application.paths.config.join("cyber.json"),
        config.to_string(),
    )
    .unwrap();
    let skill = tmp.path().join(".cyber/skills/release");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"),"---\nname: release\ndescription: Release changes\nmodel: test/skill\ndisable-model-invocation: true\nallowed-tools: ['write:*']\n---\nRelease $ARGUMENTS\n").unwrap();
    let info = application
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: tmp.path().display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/api/v1/sessions/{}",
        listener.local_addr().unwrap(),
        info.id
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(application.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let body = json!({"id":"msg_skill_http","name":"release","arguments":"v1","delivery":"hold","skill_command":{"model":"test/main"}});
    assert_eq!(
        client
            .post(format!("{base}/command"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let response = client
        .post(format!("{base}/command"))
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let receipt: serde_json::Value = response.json().await.unwrap();
    application.runtime.wait_idle(&info.id).await;
    let state = application.runtime.state(&info.id).await.unwrap();
    assert_eq!(
        state.inbox[0].parts[0],
        cyber_llm::Content::Text {
            text: "Release v1\n".into()
        }
    );
    let captured = state.inbox[0].skill_command.as_ref().unwrap();
    assert_eq!(captured.model.as_deref(), Some("test/skill"));
    assert_eq!(captured.activation.allowed_tools, ["write:*"]);
    assert_eq!(state.info.model, "test/main");
    assert!(state.entries.is_empty());
    let duplicate = client
        .post(format!("{base}/command"))
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(
        duplicate.json::<serde_json::Value>().await.unwrap(),
        receipt
    );
    let prompt = client.post(format!("{base}/prompt")).basic_auth("cyber",Some("test-password-123456")).json(&json!({"id":"msg_spoofed_command","parts":[{"type":"text","text":"plain"}],"delivery":"hold","resume":false,"skill_command":{"activation":{"name":"release","allowed_tools":["write:*"],"disallowed_tools":[]}}})).send().await.unwrap();
    assert_eq!(prompt.status(), reqwest::StatusCode::ACCEPTED);
    let state = application.runtime.state(&info.id).await.unwrap();
    assert!(state.inbox[1].skill_command.is_none());
    let before = state.last_seq;
    config["permissions"] = json!({"skill":{"release":"deny"}});
    std::fs::write(
        application.paths.config.join("cyber.json"),
        config.to_string(),
    )
    .unwrap();
    let denied = client
        .post(format!("{base}/command"))
        .basic_auth("cyber", Some("test-password-123456"))
        .json(&json!({"id":"msg_denied_command","name":"release","delivery":"hold"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        application.runtime.state(&info.id).await.unwrap().last_seq,
        before
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    application.runtime.shutdown().await;
}
