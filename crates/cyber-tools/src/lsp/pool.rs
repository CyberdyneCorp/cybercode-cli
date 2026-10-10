use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use cyber_core::intelligence::{DetectedServer, server_root};
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{LspError, StdioConnection};
use crate::HookCommandProcess;

#[derive(Clone)]
pub struct LaunchRequest {
    pub location: PathBuf,
    pub root: PathBuf,
    pub server: DetectedServer,
}

/// The caller grants launch authority and retains sandbox/proxy resources here.
pub struct AuthorizedProcess {
    pub process: HookCommandProcess,
    pub keepalive: Box<dyn ResourceLease>,
}

/// Explicit resource settlement follows native process settlement; Drop grants no acknowledgement.
pub trait ResourceLease: Send {
    fn close(&mut self) -> BoxFuture<'_, bool>;
}
impl ResourceLease for () {
    fn close(&mut self) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("LSP launch failed (native termination acknowledged: {acknowledged})")]
pub struct LaunchError {
    #[source]
    pub error: LspError,
    pub acknowledged: bool,
}

/// Launch must respond to cancellation and settle any acquired native resources before error.
pub type LaunchFn = Arc<
    dyn Fn(
            LaunchRequest,
            CancellationToken,
        ) -> BoxFuture<'static, Result<AuthorizedProcess, LaunchError>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    Starting,
    Connected,
    Broken,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServerStatus {
    pub id: String,
    pub root: PathBuf,
    pub status: ServerState,
}

#[derive(Debug, Clone)]
pub struct Settlement {
    pub id: String,
    pub root: PathBuf,
    pub acknowledged: bool,
}

enum Command {
    Request {
        method: String,
        params: Value,
        timeout: Duration,
        reply: oneshot::Sender<Result<Value, LspError>>,
    },
    Notify {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<(), LspError>>,
    },
    Notifications(oneshot::Sender<Vec<Value>>),
}

struct Worker {
    task: Option<JoinHandle<Settlement>>,
    settled: Option<Settlement>,
}

struct Entry {
    status: watch::Receiver<ServerStatus>,
    sender: mpsc::Sender<Command>,
    worker: tokio::sync::Mutex<Worker>,
}

#[derive(Clone)]
pub struct ServerHandle {
    entry: Arc<Entry>,
}

impl ServerHandle {
    pub fn status(&self) -> ServerStatus {
        let mut status = self.entry.status.borrow().clone();
        if self.entry.status.has_changed().is_err() {
            status.status = ServerState::Broken;
        }
        status
    }

    pub async fn connected(&self) -> Result<(), LspError> {
        let mut receiver = self.entry.status.clone();
        loop {
            match self.status().status {
                ServerState::Connected => return Ok(()),
                ServerState::Broken => return Err(unavailable()),
                ServerState::Starting => receiver.changed().await.map_err(|_| unavailable())?,
            }
        }
    }

    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Request {
                method: method.into(),
                params,
                timeout,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Notify {
                method: method.into(),
                params,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }

    pub async fn take_notifications(&self) -> Result<Vec<Value>, LspError> {
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Notifications(reply))
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())
    }
}

#[derive(Default)]
struct Table {
    closing: bool,
    entries: BTreeMap<(String, PathBuf), Arc<Entry>>,
}

struct Inner {
    location: PathBuf,
    servers: BTreeMap<String, DetectedServer>,
    launcher: LaunchFn,
    table: Mutex<Table>,
    cancel: CancellationToken,
    close: tokio::sync::Mutex<()>,
    initialize_timeout: Duration,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// One immutable configuration generation for one Location; failed roots never respawn.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

impl Pool {
    pub fn new(
        location: &Path,
        servers: Vec<DetectedServer>,
        launcher: LaunchFn,
    ) -> Result<Self, LspError> {
        let location = location
            .canonicalize()
            .map_err(|_| LspError::Protocol("invalid Location"))?;
        if !location.is_dir() {
            return Err(LspError::Protocol("Location is not a directory"));
        }
        let mut definitions = BTreeMap::new();
        for server in servers {
            if definitions
                .insert(server.definition.id.clone(), server)
                .is_some()
            {
                return Err(LspError::Protocol("duplicate server ID"));
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                location,
                servers: definitions,
                launcher,
                table: Mutex::default(),
                cancel: CancellationToken::new(),
                close: tokio::sync::Mutex::new(()),
                initialize_timeout: Duration::from_secs(45),
            }),
        })
    }

    /// Admission is synchronous; the owned worker initializes in the background.
    pub fn ensure(&self, id: &str, file: &Path) -> Result<ServerHandle, LspError> {
        let server = self.inner.servers.get(id).ok_or_else(unavailable)?;
        if !server.enabled
            || !server.installed
            || server.executable.is_none()
            || server.definition.command.is_empty()
            || !matches_file(server, file)
        {
            return Err(unavailable());
        }
        let root = server_root(&self.inner.location, file, &server.definition.root_markers)
            .map_err(|_| LspError::Protocol("file is outside available Location roots"))?;
        let mut table = self.inner.table.lock().map_err(|_| unavailable())?;
        if table.closing {
            return Err(unavailable());
        }
        let key = (id.to_owned(), root.clone());
        let entry = table
            .entries
            .entry(key)
            .or_insert_with(|| {
                let initial = ServerStatus {
                    id: id.into(),
                    root: root.clone(),
                    status: ServerState::Starting,
                };
                let (status, receiver) = watch::channel(initial);
                let (sender, commands) = mpsc::channel(32);
                let request = LaunchRequest {
                    location: self.inner.location.clone(),
                    root,
                    server: server.clone(),
                };
                let task = tokio::spawn(serve(
                    request,
                    self.inner.launcher.clone(),
                    self.inner.cancel.child_token(),
                    status,
                    commands,
                    self.inner.initialize_timeout,
                ));
                Arc::new(Entry {
                    status: receiver,
                    sender,
                    worker: tokio::sync::Mutex::new(Worker {
                        task: Some(task),
                        settled: None,
                    }),
                })
            })
            .clone();
        Ok(ServerHandle { entry })
    }

    pub fn status(&self) -> Result<Vec<ServerStatus>, LspError> {
        let table = self.inner.table.lock().map_err(|_| unavailable())?;
        Ok(table
            .entries
            .values()
            .map(|entry| {
                ServerHandle {
                    entry: entry.clone(),
                }
                .status()
            })
            .collect())
    }

    /// Cancellation of this future preserves each join handle for subsequent settlement.
    pub async fn close(&self) -> Result<Vec<Settlement>, LspError> {
        let _close = self.inner.close.lock().await;
        let entries = {
            let mut table = self.inner.table.lock().map_err(|_| unavailable())?;
            table.closing = true;
            self.inner.cancel.cancel();
            table.entries.values().cloned().collect::<Vec<_>>()
        };
        let mut settlements = Vec::new();
        for entry in entries {
            settlements.push(settle_entry(&entry).await);
        }
        Ok(settlements)
    }
}

async fn settle_entry(entry: &Entry) -> Settlement {
    let mut worker = entry.worker.lock().await;
    if let Some(result) = &worker.settled {
        return result.clone();
    }
    let status = entry.status.borrow().clone();
    let result = match worker.task.as_mut() {
        Some(task) => task.await.unwrap_or(Settlement {
            id: status.id,
            root: status.root,
            acknowledged: false,
        }),
        None => Settlement {
            id: status.id,
            root: status.root,
            acknowledged: false,
        },
    };
    worker.task.take();
    worker.settled = Some(result.clone());
    result
}

fn matches_file(server: &DetectedServer, file: &Path) -> bool {
    let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    server.definition.extensions.iter().any(|extension| {
        let suffix = if extension.starts_with('.') {
            extension.clone()
        } else {
            format!(".{extension}")
        };
        name.ends_with(&suffix)
    })
}

fn unavailable() -> LspError {
    LspError::Protocol("language server is unavailable")
}

async fn serve(
    request: LaunchRequest,
    launcher: LaunchFn,
    cancel: CancellationToken,
    status: watch::Sender<ServerStatus>,
    commands: mpsc::Receiver<Command>,
    initialize_timeout: Duration,
) -> Settlement {
    let identity = Settlement {
        id: request.server.definition.id.clone(),
        root: request.root.clone(),
        acknowledged: false,
    };
    let (mut connection, mut keepalive) =
        match startup(&request, &launcher, &cancel, &status, initialize_timeout).await {
            Ok(resources) => resources,
            Err(acknowledged) => {
                status.send_modify(|s| s.status = ServerState::Broken);
                return Settlement {
                    acknowledged,
                    ..identity
                };
            }
        };
    status.send_modify(|s| s.status = ServerState::Connected);
    run(&mut connection, &cancel, commands).await;
    status.send_modify(|s| s.status = ServerState::Broken);
    settle_connection(&mut connection).await;
    settle_resources(&mut *keepalive).await;
    drop(keepalive);
    Settlement {
        acknowledged: true,
        ..identity
    }
}

type ConnectionResources = (StdioConnection, Box<dyn ResourceLease>);

async fn startup(
    request: &LaunchRequest,
    launcher: &LaunchFn,
    cancel: &CancellationToken,
    status: &watch::Sender<ServerStatus>,
    initialize_timeout: Duration,
) -> Result<ConnectionResources, bool> {
    let AuthorizedProcess {
        process,
        mut keepalive,
    } = launcher(request.clone(), cancel.clone())
        .await
        .map_err(|error| error.acknowledged)?;
    let connected = StdioConnection::connect_with_cancellation(
        process,
        &request.root,
        request
            .server
            .definition
            .initialization_options
            .clone()
            .unwrap_or(Value::Null),
        initialize_timeout,
        cancel.clone(),
    )
    .await;
    match connected {
        Ok(connection) => Ok((connection, keepalive)),
        Err(mut error) => {
            status.send_modify(|s| s.status = ServerState::Broken);
            while !error.retry_shutdown().await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            settle_resources(&mut *keepalive).await;
            Err(true)
        }
    }
}

async fn settle_resources(resources: &mut dyn ResourceLease) {
    while !resources.close().await {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run(
    connection: &mut StdioConnection,
    cancel: &CancellationToken,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut health = tokio::time::interval(Duration::from_millis(100));
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = health.tick() => {
                if connection.leader_exited().unwrap_or(true) { break; }
                continue;
            },
            command = commands.recv() => match command {Some(c) => Event::Command(c), None => break},
            message = connection.next_idle() => match message {Ok(message) => Event::Message(message), Err(_) => break}
        };
        let healthy = tokio::select! { biased; _ = cancel.cancelled() => break, healthy = handle_event(connection, event) => healthy };
        if !healthy {
            break;
        }
    }
}

enum Event {
    Message(Value),
    Command(Command),
}

async fn handle_event(connection: &mut StdioConnection, event: Event) -> bool {
    match event {
        Event::Command(command) => execute(connection, command).await,
        Event::Message(message) => {
            tokio::time::timeout(Duration::from_secs(30), connection.handle_idle(message))
                .await
                .is_ok_and(|result| result.is_ok())
        }
    }
}

async fn settle_connection(connection: &mut StdioConnection) {
    let mut shutdown = connection.shutdown().await;
    while !shutdown.acknowledged {
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown = connection.shutdown().await;
    }
}

#[cfg(all(test, unix))]
#[path = "pool_tests.rs"]
mod tests;

async fn execute(connection: &mut StdioConnection, command: Command) -> bool {
    match command {
        Command::Request {
            method,
            params,
            timeout,
            reply,
        } => {
            if reply.is_closed() {
                return true;
            }
            let result = connection.request(&method, params, timeout).await;
            let healthy = result.is_ok() || matches!(result, Err(LspError::Remote(_)));
            let _ = reply.send(result);
            healthy
        }
        Command::Notify {
            method,
            params,
            reply,
        } => {
            if reply.is_closed() {
                return true;
            }
            let result =
                tokio::time::timeout(Duration::from_secs(30), connection.notify(&method, params))
                    .await
                    .unwrap_or(Err(LspError::Timeout));
            let healthy = result.is_ok();
            let _ = reply.send(result);
            healthy
        }
        Command::Notifications(reply) => {
            let _ = reply.send(connection.take_notifications());
            true
        }
    }
}
