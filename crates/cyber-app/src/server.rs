//! Running the server: lock, listeners, password and registration.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use cyber_core::paths::Paths;
use cyber_server::http;
use tokio::sync::Notify;

use crate::App;
use crate::registration::{Registration, registration_path, remove_registration, write_private};

const DEFAULT_PORT: u16 = 4747;

pub struct ServeOptions {
    pub hostname: String,
    /// `None` tries 4747, then an OS-assigned port.
    pub port: Option<u16>,
    /// `None` uses `<state>/cyber.sock`.
    pub socket: Option<PathBuf>,
    pub no_tcp: bool,
    /// Write `server.json` so clients find this server.
    pub register: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            hostname: "127.0.0.1".into(),
            port: None,
            socket: None,
            no_tcp: false,
            register: false,
        }
    }
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

/// Serve until `stop` is notified.
pub async fn run_server_until(
    app: App,
    opts: ServeOptions,
    on_ready: impl FnOnce(&str),
    stop: Arc<Notify>,
) -> Result<(), String> {
    let _lock = cyber_store::OwnershipLock::acquire(&app.paths.server_lock())
        .map_err(|e| format!("another cyber server is already running for this user ({e})"))?;
    let router = app.router();
    let retention = crate::retention::Retention::from_config(
        &(app.config)(&app.state.options.default_directory)
            .map(|(v, _)| v)
            .unwrap_or_default(),
    );
    let sweeper = crate::retention::spawn(app.runtime.clone(), app.paths.data.clone(), retention);
    let tcp = if opts.no_tcp {
        None
    } else {
        Some(bind(&opts.hostname, opts.port).await?)
    };
    let url = tcp
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map(|a: SocketAddr| format!("http://{a}"));
    let socket = opts
        .socket
        .clone()
        .unwrap_or_else(|| app.paths.state.join("cyber.sock"));
    let registration = Registration {
        id: cyber_core::ids::new_id("srv"),
        version: crate::version().into(),
        url: url.clone().unwrap_or_default(),
        socket: Some(socket.display().to_string()),
        pid: std::process::id(),
    };
    cyber_core::log::info(
        "server",
        "listening",
        serde_json::json!({ "url": url, "socket": socket.display().to_string(), "pid": std::process::id() }),
    );
    on_ready(
        url.as_deref()
            .unwrap_or(&format!("unix:{}", socket.display())),
    );
    if opts.register {
        write_private(
            &registration_path(&app.paths),
            &serde_json::to_string_pretty(&registration).unwrap_or_default(),
        )?;
    }
    let tcp_task = tcp.map(|listener| {
        let stop = Arc::clone(&stop);
        tokio::spawn(http::serve_tcp(router.clone(), listener, async move {
            stop.notified().await
        }))
    });
    let unix_task = unix(router, &socket, Arc::clone(&stop));
    stop.notified().await;
    stop.notify_waiters();
    if let Some(task) = tcp_task {
        let _ = task.await;
    }
    if let Some(task) = unix_task {
        let _ = task.await;
    }
    sweeper.abort();
    let _ = std::fs::remove_file(&socket);
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
    stop: Arc<Notify>,
) -> Option<tokio::task::JoinHandle<std::io::Result<()>>> {
    let socket = socket.to_path_buf();
    Some(tokio::spawn(async move {
        http::serve_unix(router, &socket, async move { stop.notified().await }).await
    }))
}

#[cfg(not(unix))]
fn unix(
    _router: axum::Router,
    _socket: &Path,
    _stop: Arc<Notify>,
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
