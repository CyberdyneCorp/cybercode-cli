//! Running the server: lock, listeners (main, Unix socket and peer), password and
//! registration.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use cyber_core::paths::Paths;
use cyber_server::http;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::App;
use crate::registration::{Registration, registration_path, remove_registration, write_private};

const DEFAULT_PORT: u16 = 4747;

pub struct ServeOptions {
    pub hostname: String,
    /// `None` tries 4747, then an OS-assigned port.
    pub port: Option<u16>,
    /// `None` uses `<state>/cyber.sock`; overrides must stay inside the private state directory.
    pub socket: Option<PathBuf>,
    pub no_tcp: bool,
    /// Write `server.json` so clients find this server.
    pub register: bool,
    /// Publish the peer listener over mDNS (`cyber serve --mdns`), as `peers.mdns` does.
    pub mdns: bool,
    /// How long a peer attach waits for `remote.attach`: 2 minutes; tests shorten it.
    pub peer_attach_wait: std::time::Duration,
    /// The background update check (`installation-upgrade`): the running build's
    /// version and channel; tests pin them and the cadence.
    pub update: crate::update_check::UpdateCheckOptions,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            hostname: "127.0.0.1".into(),
            port: None,
            socket: None,
            no_tcp: false,
            register: false,
            mdns: false,
            peer_attach_wait: cyber_server::http::peer::ATTACH_WAIT,
            update: crate::update_check::UpdateCheckOptions::default(),
        }
    }
}

/// Owner HTTPS settings come exclusively from the global configuration, never a project.
pub fn owner_tls_files(global: &serde_json::Value) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let Some(server) = global.get("server") else {
        return Ok(None);
    };
    if !server.is_object() {
        return Err("server must be an object in the global configuration".into());
    }
    let Some(tls) = server.get("tls") else {
        return Ok(None);
    };
    if tls.is_null() {
        return Ok(None);
    }
    let pair = tls
        .get("cert")
        .and_then(|v| v.as_str())
        .zip(tls.get("key").and_then(|v| v.as_str()));
    let Some((cert, key)) = pair else {
        return Err("server.tls requires both cert and key file paths".into());
    };
    if cert.is_empty() || key.is_empty() {
        return Err("server.tls paths cannot be empty".into());
    }
    let (cert, key) = (PathBuf::from(cert), PathBuf::from(key));
    if !cert.is_absolute() || !key.is_absolute() {
        return Err(
            "server.tls cert and key must be absolute paths in the global configuration".into(),
        );
    }
    Ok(Some((cert, key)))
}

/// The local server password, generated on first use (32 random bytes, base64url).
pub fn password(paths: &Paths) -> Result<String, String> {
    let file = paths.state.join("password");
    if let Ok(existing) = std::fs::read_to_string(&file)
        && !existing.trim().is_empty()
    {
        return Ok(existing.trim().to_string());
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    write_private(&file, &value)?;
    Ok(value)
}

/// Stop a registered server using its existing credentials before replacing the password.
/// The caller restarts the service when this returns `true`.
pub async fn replace_password(paths: &Paths, value: &str) -> Result<bool, String> {
    if value.len() < 16 {
        return Err("the password must be at least 16 characters".into());
    }
    let restart = crate::read_registration(paths).is_some();
    if restart {
        crate::stop_service(paths).await?;
    }
    write_private(&paths.state.join("password"), value)?;
    Ok(restart)
}

/// Serve until SIGINT or SIGTERM. Only one server per OS user holds the lock.
pub async fn run_server(
    app: App,
    opts: ServeOptions,
    on_ready: impl FnOnce(&str),
) -> Result<(), String> {
    let stop = Arc::new(Notify::new());
    spawn_signal_handler(Arc::clone(&stop));
    run_server_until(app, opts, on_ready, stop).await
}

/// Stop background loops (retention, update checks) and wait for them to end.
async fn abort_all<const N: usize>(tasks: [tokio::task::JoinHandle<()>; N]) {
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

/// Serve until `stop` is notified.
pub async fn run_server_until(
    mut app: App,
    opts: ServeOptions,
    on_ready: impl FnOnce(&str),
    stop: Arc<Notify>,
) -> Result<(), String> {
    let global = (app.owner_server_config)()?;
    let tls_files = owner_tls_files(&global)?;
    let tls = tls_files
        .as_ref()
        .map(|(cert, key)| http::owner_tls(cert, key))
        .transpose()?;
    let socket = local_socket(&opts, &app.paths)?;
    let _lock = cyber_store::OwnershipLock::acquire(&app.paths.server_lock())
        .map_err(|e| format!("another cyber server is already running for this user ({e})"))?;
    let server_id = cyber_core::ids::new_id("srv");
    // Holding the lock proves earlier server processes exited: release what they owned.
    app.runtime
        .adopt_server_lock(&server_id)
        .map_err(|e| format!("cannot recover child ownership of an earlier server: {e}"))?;
    let retention = crate::retention::Retention::from_config(
        &(app.config)(&app.state.options.default_directory)
            .map(|(v, _)| v)
            .unwrap_or_default(),
    );
    let tcp = if opts.no_tcp {
        None
    } else {
        Some(bind(&opts.hostname, opts.port).await?)
    };
    if let Some(listener) = &tcp
        && !listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .ip()
            .is_loopback()
        && tls.is_none()
    {
        return Err("non-loopback owner listeners require global server.tls.cert and server.tls.key; use loopback with an SSH forward or configure HTTPS".into());
    }
    let events = app.telemetry_events.take();
    let telemetry = start_telemetry(&app, events).await;
    let sweeper = crate::retention::spawn(app.runtime.clone(), app.paths.data.clone(), retention);
    let updates = crate::update_check::spawn(&app, opts.update.clone());
    let scheme = if tls.is_some() { "https" } else { "http" };
    let url = tcp
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map(|a: SocketAddr| {
            // Preserve DNS names for certificate verification. Wildcard listeners are
            // reached locally through loopback; remote clients use their certificate hostname.
            let host = if a.ip().is_unspecified() {
                if a.is_ipv6() {
                    "[::1]".to_string()
                } else {
                    "127.0.0.1".to_string()
                }
            } else if opts.hostname.contains(':') {
                format!("[{}]", opts.hostname.trim_matches(['[', ']']))
            } else {
                opts.hostname.clone()
            };
            format!("{scheme}://{host}:{}", a.port())
        });
    let peer = crate::peer_listener::bind(&app, opts.peer_attach_wait).await;
    let mut registration = Registration::current(
        server_id,
        url.clone().unwrap_or_default(),
        socket.as_ref().map(|p| p.display().to_string()),
    );
    registration.bind_host = (!opts.no_tcp).then(|| opts.hostname.clone());
    registration.tls_cert = tls_files
        .as_ref()
        .map(|(cert, _)| cert.display().to_string());
    let peer_handle = peer.as_ref().map(|peer| Arc::clone(&peer.handle));
    registration.peer_url = peer_handle.as_ref().map(|handle| handle.url());
    registration.peer_fingerprint = peer_handle.as_ref().map(|handle| handle.info().fingerprint);
    if let Some(handle) = &peer_handle
        && opts.register
    {
        handle.on_rotate(rewrite_fingerprint(&app.paths, &registration));
    }
    let mut state = app.state.clone();
    state.service = Some(http::ServiceControl {
        id: registration.id.clone(),
        stop: Arc::clone(&stop),
        peer: peer_handle,
    });
    state.telemetry = Some(Arc::clone(&telemetry) as _);
    let router = http::router(state);
    cyber_core::log::info(
        "server",
        "listening",
        serde_json::json!({
            "url": url,
            "socket": registration.socket,
            "peer_url": registration.peer_url,
            "peer_fingerprint": registration.peer_fingerprint,
            "pid": std::process::id(),
        }),
    );
    let address = url
        .clone()
        .or_else(|| socket.as_ref().map(|p| format!("unix:{}", p.display())))
        .ok_or_else(|| "server has no available listener".to_string())?;
    on_ready(&address);
    if opts.register {
        write_private(
            &registration_path(&app.paths),
            &serde_json::to_string_pretty(&registration).unwrap_or_default(),
        )?;
    }
    // The listeners stop on a token, not on `stop` itself: `notify_waiters` wakes only
    // tasks already waiting, and a listener task that was not polled yet (a stop right
    // after the server registered) would miss it and keep the server from stopping.
    let closing = CancellationToken::new();
    let tcp_task = tcp.map(|listener| {
        let closing = closing.clone().cancelled_owned();
        tokio::spawn(http::serve_tcp_tls(router.clone(), listener, tls, closing))
    });
    let publication =
        crate::peer_discovery::start(&app, peer.as_ref().map(|peer| &peer.handle), opts.mdns);
    let peer_task = peer.map(|peer| peer.spawn(&app, closing.clone().cancelled_owned()));
    let unix_task = socket
        .as_ref()
        .and_then(|socket| unix(router, socket, closing.clone()));
    stop.notified().await;
    closing.cancel();
    stop.notify_waiters();
    crate::peer_discovery::unpublish(publication).await;
    shut_down(&app, &telemetry, [sweeper, updates]).await;
    for task in [tcp_task, peer_task, unix_task].into_iter().flatten() {
        let _ = task.await;
    }
    if let Some(socket) = socket {
        let _ = std::fs::remove_file(socket);
    }
    cyber_core::log::info(
        "server",
        "stopped",
        serde_json::json!({ "pid": std::process::id() }),
    );
    if opts.register {
        remove_registration(&app.paths, &registration.id);
    }
    Ok(())
}

/// Stop the background `tasks`, then the runtime, then telemetry: the runtime's last
/// events are mapped before the open spans end.
async fn shut_down<const N: usize>(
    app: &App,
    telemetry: &cyber_telemetry::Telemetry,
    tasks: [tokio::task::JoinHandle<()>; N],
) {
    abort_all(tasks).await;
    app.runtime.shutdown().await;
    telemetry.shutdown().await;
}

/// Resolve the telemetry settings once (a change applies on restart, and `cyber telemetry
/// show` says so) and start exporting the runtime's events when export is on: from
/// `events`, which `App::build` subscribed before its recovery, or from now on when the
/// settings changed on disk since.
async fn start_telemetry(
    app: &App,
    events: Option<tokio::sync::broadcast::Receiver<cyber_server::runtime::LiveEvent>>,
) -> Arc<cyber_telemetry::Telemetry> {
    let telemetry = Arc::new(cyber_telemetry::Telemetry::start(
        Arc::clone(&app.telemetry_config),
        Arc::new(cyber_core::env::ProcessEnv),
        &app.state.options.default_directory,
    ));
    if telemetry.settings().enabled {
        telemetry.export(
            events.unwrap_or_else(|| app.runtime.subscribe()),
            Arc::new(app.runtime.clone()),
            telemetry_account(app).await,
        );
    }
    telemetry
}

/// The ids of the account signed in at start, for the telemetry resource. Signing in or
/// out applies at the next start; a slow account read is not waited for.
async fn telemetry_account(app: &App) -> Option<cyber_telemetry::AccountIds> {
    let service = app.state.services.account()?;
    let view = tokio::time::timeout(std::time::Duration::from_secs(2), service.current())
        .await
        .ok()?
        .ok()?;
    let account = view.account?;
    Some(cyber_telemetry::AccountIds {
        sub: account.sub,
        org_id: account.org.map(|org| org.id),
    })
}

/// After a key rotation, `server.json` names the new fingerprint.
fn rewrite_fingerprint(
    paths: &Paths,
    registration: &Registration,
) -> http::peer::handle::RotateHook {
    let path = registration_path(paths);
    let registration = std::sync::Mutex::new(registration.clone());
    Box::new(move |fingerprint| {
        let mut registration = registration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registration.peer_fingerprint = Some(fingerprint.to_string());
        let text = serde_json::to_string_pretty(&*registration).unwrap_or_default();
        if let Err(error) = write_private(&path, &text) {
            cyber_core::log::warn(
                "server",
                "cannot record the rotated peer key",
                serde_json::json!({ "reason": error }),
            );
        }
    })
}

#[cfg(unix)]
fn local_socket(opts: &ServeOptions, paths: &Paths) -> Result<Option<PathBuf>, String> {
    let root = std::fs::canonicalize(&paths.state).map_err(|error| error.to_string())?;
    use std::os::unix::fs::FileTypeExt;
    let socket = opts
        .socket
        .clone()
        .unwrap_or_else(|| root.join("cyber.sock"));
    let invalid = || {
        "Unix socket must be an absolute path inside the private state directory, with an existing non-symlink parent; use <state>/cyber.sock or authenticated TCP".to_string()
    };
    if !socket.is_absolute()
        || socket
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    // Accept the configured state's spelling and its canonical spelling, but no
    // workspace alias. A model-writable parent could impersonate the listener
    // and collect an owner's Basic credential even with same-UID peer checks.
    let relative = socket
        .strip_prefix(&paths.state)
        .or_else(|_| socket.strip_prefix(&root))
        .map_err(|_| invalid())?;
    let mut checked = root.clone();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(invalid());
        }
        checked.push(part);
        match std::fs::symlink_metadata(&checked) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let parent = checked.parent().ok_or_else(invalid)?;
    let parent = std::fs::canonicalize(parent).map_err(|_| invalid())?;
    if !parent.starts_with(&root) || checked == root {
        return Err(invalid());
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&checked)
        && !metadata.file_type().is_socket()
    {
        return Err("refusing to replace a non-socket file in the private state directory".into());
    }
    Ok(Some(checked))
}

#[cfg(not(unix))]
fn local_socket(opts: &ServeOptions, _paths: &Paths) -> Result<Option<PathBuf>, String> {
    if opts.socket.is_some() || opts.no_tcp {
        return Err("Unix socket listeners are unavailable on this platform; use TCP without --socket or --no-tcp".into());
    }
    Ok(None)
}

async fn bind(host: &str, port: Option<u16>) -> Result<tokio::net::TcpListener, String> {
    match tokio::net::TcpListener::bind((host, port.unwrap_or(DEFAULT_PORT))).await {
        Ok(l) => Ok(l),
        Err(_) if port.is_none() => tokio::net::TcpListener::bind((host, 0))
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(format!(
            "cannot listen on {host}:{}: {e}",
            port.unwrap_or(DEFAULT_PORT)
        )),
    }
}

#[cfg(unix)]
fn unix(
    router: axum::Router,
    socket: &Path,
    closing: CancellationToken,
) -> Option<tokio::task::JoinHandle<std::io::Result<()>>> {
    let socket = socket.to_path_buf();
    Some(tokio::spawn(async move {
        http::serve_unix(router, &socket, closing.cancelled_owned()).await
    }))
}

#[cfg(not(unix))]
fn unix(
    _router: axum::Router,
    _socket: &Path,
    _closing: CancellationToken,
) -> Option<tokio::task::JoinHandle<std::io::Result<()>>> {
    None
}

/// SIGINT and SIGTERM stop the server; `notify_one` stores a permit for the main wait.
fn spawn_signal_handler(stop: Arc<Notify>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        stop.notify_one();
    });
}

/// JSON-RPC over stdin and stdout until stdin closes (`server-api` → Stdio transport).
pub async fn serve_stdio(app: App) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(256);
    let session = Arc::new(http::rpc::RpcSession::new(&app.state, tx));
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(frame) = rx.recv().await {
            let line = format!("{frame}\n");
            if out.write_all(line.as_bytes()).await.is_err() || out.flush().await.is_err() {
                return;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut pending = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let session = Arc::clone(&session);
        pending.push(tokio::spawn(async move { session.handle(&line).await }));
    }
    for task in pending {
        let _ = task.await;
    }
    drop(session);
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod dashboard_tests {
    use super::*;
    #[test]
    fn owner_tls_requires_complete_absolute_operator_paths() {
        use serde_json::json;
        assert!(owner_tls_files(&json!({})).unwrap().is_none());
        assert!(
            owner_tls_files(
                &json!({"server":{"tls":{"cert":"relative.pem","key":"relative.key"}}})
            )
            .is_err()
        );
        assert!(owner_tls_files(&json!({"server":{"tls":{"cert":"only"}}})).is_err());
        for server in [json!(false), json!("invalid"), json!([]), json!(null)] {
            assert!(owner_tls_files(&json!({"server":server})).is_err());
        }
        for tls in [json!(false), json!("invalid"), json!([])] {
            assert!(owner_tls_files(&json!({"server":{"tls":tls}})).is_err());
        }
        assert!(
            owner_tls_files(&json!({"server":{"tls":{"cert":"","key":"/operator/key.pem"}}}))
                .is_err()
        );
        let dir = std::env::temp_dir();
        let config =
            json!({"server":{"tls":{"cert":dir.join("cert.pem"),"key":dir.join("key.pem")}}});
        assert!(owner_tls_files(&config).unwrap().is_some());
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use super::*;

    #[test]
    fn owner_socket_cannot_use_a_model_writable_parent_or_symlink_alias() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(state.join("nested")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let paths = Paths {
            state: state.clone(),
            data: temp.path().join("data"),
            config: temp.path().join("config"),
            cache: temp.path().join("cache"),
            tmp: temp.path().join("tmp"),
        };
        let select = |path| {
            local_socket(
                &ServeOptions {
                    socket: Some(path),
                    ..Default::default()
                },
                &paths,
            )
        };
        assert_eq!(
            select(state.join("custom.sock")).unwrap(),
            Some(std::fs::canonicalize(&state).unwrap().join("custom.sock"))
        );
        assert!(select(state.join("nested/custom.sock")).is_ok());
        assert!(select(workspace.join("steal.sock")).is_err());
        assert!(select(state.join("../workspace/steal.sock")).is_err());
        assert!(select(state.join("missing/custom.sock")).is_err());
        assert!(select(state.clone()).is_err());
        std::fs::write(state.join("password"), "owner-canary").unwrap();
        assert!(select(state.join("password")).is_err());
        assert_eq!(
            std::fs::read_to_string(state.join("password")).unwrap(),
            "owner-canary"
        );
        assert!(select(PathBuf::from("relative.sock")).is_err());
        std::os::unix::fs::symlink(&workspace, state.join("escape")).unwrap();
        assert!(select(state.join("escape/steal.sock")).is_err());
        std::os::unix::fs::symlink(state.join("nested"), state.join("alias")).unwrap();
        assert!(select(state.join("alias/custom.sock")).is_err());
        std::os::unix::fs::symlink(&state, workspace.join("alias")).unwrap();
        assert!(select(workspace.join("alias/custom.sock")).is_err());
        std::os::unix::fs::symlink(workspace.join("sentinel"), state.join("leaf.sock")).unwrap();
        assert!(select(state.join("leaf.sock")).is_err());
    }
}
