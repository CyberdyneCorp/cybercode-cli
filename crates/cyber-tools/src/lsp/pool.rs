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

use super::diagnostics::{Diagnostics, Publication};
use super::{LspError, StdioConnection};
use crate::HookCommandProcess;

#[derive(Clone)]
pub struct LaunchRequest {
    pub checkouts: Vec<cyber_core::worktrees::Managed>,
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
    fn admit_document(
        &mut self,
        path: PathBuf,
        checkouts: Vec<cyber_core::worktrees::Managed>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), LspError>> {
        Box::pin(async move {
            super::documents::verify_origin(path, checkouts.clone()).await?;
            if !checkouts.is_empty() {
                return Err(LspError::Protocol("document checkout claim unavailable"));
            }
            Ok(())
        })
    }
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

pub type AdmissionFn =
    Arc<dyn Fn(LaunchRequest) -> BoxFuture<'static, Result<(), LspError>> + Send + Sync>;

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
    Open {
        checkouts: Vec<cyber_core::worktrees::Managed>,
        path: PathBuf,
        text: String,
        reply: oneshot::Sender<Result<(), LspError>>,
    },
    Save {
        checkouts: Vec<cyber_core::worktrees::Managed>,
        path: PathBuf,
        text: String,
        reply: oneshot::Sender<Result<Arc<SaveReceipt>, LspError>>,
    },
    Feedback {
        path: PathBuf,
        receipt: Arc<SaveReceipt>,
        other: bool,
        reply: oneshot::Sender<Result<Feedback, LspError>>,
    },
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
    Diagnostics {
        path: PathBuf,
        reply: oneshot::Sender<Result<Option<super::DiagnosticSnapshot>, LspError>>,
    },
}

pub(super) struct SaveReceipt {
    version: i32,
    sequence: u64,
    previous: BTreeMap<PathBuf, Vec<super::Diagnostic>>,
}

pub(super) struct Feedback {
    pub complete: bool,
    pub snapshots: Vec<super::DiagnosticSnapshot>,
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
    pub(super) async fn save_observed(
        &self,
        path: PathBuf,
        text: String,
        checkouts: Vec<cyber_core::worktrees::Managed>,
    ) -> Result<Arc<SaveReceipt>, LspError> {
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Save {
                path,
                text,
                checkouts,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }

    pub(super) async fn feedback(
        &self,
        path: PathBuf,
        receipt: Arc<SaveReceipt>,
        other: bool,
    ) -> Result<Feedback, LspError> {
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Feedback {
                path,
                receipt,
                other,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }
    pub async fn open_document(&self, path: &Path, text: String) -> Result<(), LspError> {
        let checkouts = super::locations::checkout_records(path)?;
        self.open_observed(path, text, checkouts).await
    }

    async fn open_observed(
        &self,
        path: &Path,
        text: String,
        checkouts: Vec<cyber_core::worktrees::Managed>,
    ) -> Result<(), LspError> {
        let path = path.canonicalize().map_err(|_| unavailable())?;
        if !path.is_file()
            || !path.starts_with(&self.status().root)
            || text.len() > super::documents::MAX_DOCUMENT_BYTES
        {
            return Err(unavailable());
        }
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Open {
                path,
                text,
                checkouts,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }
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

    /// Returns the bounded raw archive; every message remains untrusted.
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

    /// Revalidates a canonical file's contents and checkout creation before returning a publication.
    pub async fn diagnostics(
        &self,
        path: &Path,
    ) -> Result<Option<super::DiagnosticSnapshot>, LspError> {
        if !path.is_absolute() || !path.starts_with(&self.status().root) {
            return Err(unavailable());
        }
        self.connected().await?;
        let (reply, receive) = oneshot::channel();
        self.entry
            .sender
            .send(Command::Diagnostics {
                path: path.into(),
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
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
    admission: Option<AdmissionFn>,
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
    pub(super) async fn save_observed(
        &self,
        file: &Path,
        text: String,
        checkouts: Vec<cyber_core::worktrees::Managed>,
    ) -> Vec<(ServerHandle, Arc<SaveReceipt>)> {
        let handles: Vec<_> = self
            .inner
            .servers
            .keys()
            .filter_map(|id| self.ensure(id, file).ok())
            .collect();
        futures::future::join_all(handles.into_iter().map(|handle| {
            let path = file.to_owned();
            let text = text.clone();
            let checkouts = checkouts.clone();
            async move {
                handle
                    .save_observed(path, text, checkouts)
                    .await
                    .ok()
                    .map(|receipt| (handle, receipt))
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }
    pub fn with_admission(mut self, admission: AdmissionFn) -> Result<Self, LspError> {
        Arc::get_mut(&mut self.inner)
            .ok_or_else(unavailable)?
            .admission = Some(admission);
        Ok(self)
    }
    pub async fn warm(&self, file: &Path, text: String) {
        if let Ok(checkouts) = super::locations::checkout_records(file) {
            self.warm_observed(file, text, checkouts).await;
        }
    }

    pub(crate) async fn warm_observed(
        &self,
        file: &Path,
        text: String,
        checkouts: Vec<cyber_core::worktrees::Managed>,
    ) {
        let handles: Vec<_> = self
            .inner
            .servers
            .keys()
            .filter_map(|id| self.ensure(id, file).ok())
            .collect();
        futures::future::join_all(
            handles
                .iter()
                .map(|handle| handle.open_observed(file, text.clone(), checkouts.clone())),
        )
        .await;
    }
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
                admission: None,
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
        let checkouts = cyber_core::worktrees::Repository::managed_locations_at(&root)
            .map_err(|_| unavailable())?
            .into_iter()
            .map(|(_, managed)| managed)
            .collect();
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
                    checkouts,
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
                    self.inner.admission.clone(),
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
    admission: Option<AdmissionFn>,
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
    run(
        &mut connection,
        &mut *keepalive,
        &cancel,
        commands,
        &request,
        admission,
    )
    .await;
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
    resources: &mut dyn ResourceLease,
    cancel: &CancellationToken,
    mut commands: mpsc::Receiver<Command>,
    request: &LaunchRequest,
    admission: Option<AdmissionFn>,
) {
    let mut health = tokio::time::interval(Duration::from_millis(100));
    let mut review = tokio::time::interval(Duration::from_secs(1));
    let mut documents = super::documents::Documents::default();
    let mut diagnostics = Diagnostics::default();
    while let Some(event) = next_event(
        connection,
        &mut commands,
        &mut health,
        &mut review,
        admission.is_some(),
        cancel,
    )
    .await
    {
        if !matches!(event, Event::Health)
            && !admitted(connection, &admission, request, cancel).await
        {
            break;
        }
        if !admitted_document(connection, resources, &event, cancel).await {
            break;
        }
        let healthy =
            dispatch_event(connection, &mut documents, &mut diagnostics, event, cancel).await;
        if !healthy {
            break;
        }
        if !collect_diagnostics(
            connection,
            resources,
            cancel,
            request,
            &admission,
            &documents,
            &mut diagnostics,
        )
        .await
        {
            break;
        }
    }
}

async fn dispatch_event(
    connection: &mut StdioConnection,
    documents: &mut super::documents::Documents,
    diagnostics: &mut Diagnostics,
    event: Event,
    cancel: &CancellationToken,
) -> bool {
    if let Event::Command(Command::Save {
        path, text, reply, ..
    }) = event
    {
        if reply.is_closed() {
            return true;
        }
        let Some(observation) = retained(
            connection,
            cancel,
            super::diagnostics::observe(path.clone()),
        )
        .await
        else {
            return false;
        };
        if !observation.is_some_and(|observation| observation.matches_text(&text)) {
            let _ = reply.send(Err(unavailable()));
            return true;
        }
        let sequence = diagnostics.sequence();
        let previous = diagnostics.baseline();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            result = documents.save(connection, path, text) => result.map(|version| Arc::new(SaveReceipt { version, sequence, previous })),
        };
        let healthy = result.is_ok();
        let _ = reply.send(result);
        return healthy;
    }
    if let Event::Command(Command::Feedback {
        path,
        receipt,
        other,
        reply,
    }) = event
    {
        let result = collect_feedback(
            connection,
            documents,
            diagnostics,
            cancel,
            &path,
            &receipt,
            other,
        )
        .await;
        let healthy = result.is_ok();
        let _ = reply.send(result);
        return healthy;
    }
    if let Event::Command(Command::Diagnostics { path, reply }) = event {
        let Some(observation) = retained(
            connection,
            cancel,
            super::diagnostics::observe(path.clone()),
        )
        .await
        else {
            return false;
        };
        let _ = reply.send(Ok(diagnostics.snapshot(
            &path,
            observation.as_ref(),
            documents,
        )));
        return true;
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        healthy = handle_event(connection, documents, diagnostics, event) => healthy,
    }
}

async fn collect_feedback(
    connection: &mut StdioConnection,
    documents: &super::documents::Documents,
    diagnostics: &mut Diagnostics,
    cancel: &CancellationToken,
    path: &Path,
    receipt: &SaveReceipt,
    other: bool,
) -> Result<Feedback, LspError> {
    let mut feedback = Feedback {
        complete: false,
        snapshots: vec![],
    };
    let mut paths = diagnostics.paths_after(receipt.sequence);
    paths.sort_by_key(|candidate| candidate != path);
    for candidate in paths {
        if candidate != path
            && (!other
                || feedback
                    .snapshots
                    .iter()
                    .filter(|snapshot| snapshot.path != path)
                    .count()
                    == 5)
        {
            continue;
        }
        let observation = retained(
            connection,
            cancel,
            super::diagnostics::observe(candidate.clone()),
        )
        .await
        .ok_or_else(unavailable)?;
        let Some(mut snapshot) = diagnostics.snapshot(&candidate, observation.as_ref(), documents)
        else {
            continue;
        };
        if candidate == path {
            if snapshot.document_version != Some(receipt.version) {
                continue;
            }
            feedback.complete = true;
        } else {
            snapshot.diagnostics.retain(|diagnostic| {
                diagnostic.severity == Some(1)
                    && receipt
                        .previous
                        .get(&candidate)
                        .is_none_or(|previous| !previous.contains(diagnostic))
            });
            if snapshot.diagnostics.is_empty() {
                continue;
            }
        }
        feedback.snapshots.push(snapshot);
    }
    Ok(feedback)
}

async fn retained<T>(
    connection: &mut StdioConnection,
    cancel: &CancellationToken,
    operation: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::pin!(operation);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            settle_connection(connection).await;
            let _ = operation.await;
            None
        },
        result = &mut operation => Some(result),
    }
}

async fn collect_diagnostics(
    connection: &mut StdioConnection,
    resources: &mut dyn ResourceLease,
    cancel: &CancellationToken,
    request: &LaunchRequest,
    admission: &Option<AdmissionFn>,
    documents: &super::documents::Documents,
    diagnostics: &mut Diagnostics,
) -> bool {
    for message in connection.take_notifications() {
        diagnostics.retain_raw(message.clone());
        let Some(publication) = Publication::parse(&message, &request.root, documents) else {
            continue;
        };
        if !admitted(connection, admission, request, cancel).await {
            return false;
        }
        match publication_origin(connection, resources, cancel, &publication.path, documents).await
        {
            Ok(Some(observation)) => diagnostics.publish(publication, observation, documents),
            Ok(None) => {}
            Err(()) => return false,
        }
    }
    true
}

async fn publication_origin(
    connection: &mut StdioConnection,
    resources: &mut dyn ResourceLease,
    cancel: &CancellationToken,
    path: &Path,
    documents: &super::documents::Documents,
) -> Result<Option<super::diagnostics::Observation>, ()> {
    let observation = retained(connection, cancel, super::diagnostics::observe(path.into()))
        .await
        .ok_or(())?;
    let Some(observation) = observation else {
        return Ok(None);
    };
    if !observation.matches(path, documents) {
        return Ok(None);
    }
    let claim =
        resources.admit_document(path.into(), observation.checkouts.clone(), cancel.clone());
    if !retained(connection, cancel, claim)
        .await
        .is_some_and(|result| result.is_ok())
    {
        return Err(());
    }
    let current = retained(connection, cancel, super::diagnostics::observe(path.into()))
        .await
        .ok_or(())?;
    if current.as_ref() != Some(&observation) {
        return Ok(None);
    }
    Ok(Some(observation))
}

async fn next_event(
    connection: &mut StdioConnection,
    commands: &mut mpsc::Receiver<Command>,
    health: &mut tokio::time::Interval,
    review: &mut tokio::time::Interval,
    authorized: bool,
    cancel: &CancellationToken,
) -> Option<Event> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        _ = review.tick(), if authorized => Some(Event::Review),
        _ = health.tick() => Some(Event::Health),
        command = commands.recv() => command.map(Event::Command),
        message = connection.next_idle() => message.ok().map(Event::Message),
    }
}

async fn admitted(
    connection: &mut StdioConnection,
    admission: &Option<AdmissionFn>,
    request: &LaunchRequest,
    cancel: &CancellationToken,
) -> bool {
    let observation = async {
        match admission {
            Some(admit) => admit(request.clone()).await,
            None => Ok(()),
        }
    };
    tokio::pin!(observation);
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            // Retain the observation, but do not defer native termination behind filesystem IO.
            settle_connection(connection).await;
            let _ = observation.await;
            return false;
        },
        result = &mut observation => result,
    };
    result.is_ok() && !cancel.is_cancelled()
}

enum Event {
    Health,
    Review,
    Message(Value),
    Command(Command),
}

async fn admitted_document(
    connection: &mut StdioConnection,
    resources: &mut dyn ResourceLease,
    event: &Event,
    cancel: &CancellationToken,
) -> bool {
    let Event::Command(Command::Open {
        path,
        checkouts,
        reply,
        ..
    }) = event
    else {
        if let Event::Command(Command::Save {
            path,
            checkouts,
            reply,
            ..
        }) = event
        {
            if reply.is_closed() {
                return true;
            }
            return admitted_origin(connection, resources, path, checkouts, cancel).await;
        }
        return true;
    };
    if reply.is_closed() {
        return true;
    }
    admitted_origin(connection, resources, path, checkouts, cancel).await
}

async fn admitted_origin(
    connection: &mut StdioConnection,
    resources: &mut dyn ResourceLease,
    path: &Path,
    checkouts: &[cyber_core::worktrees::Managed],
    cancel: &CancellationToken,
) -> bool {
    let admission = resources.admit_document(path.into(), checkouts.to_vec(), cancel.clone());
    tokio::pin!(admission);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            settle_connection(connection).await;
            let _ = admission.await;
            false
        },
        result = &mut admission => result.is_ok() && !cancel.is_cancelled()
    }
}

async fn handle_event(
    connection: &mut StdioConnection,
    documents: &mut super::documents::Documents,
    diagnostics: &mut Diagnostics,
    event: Event,
) -> bool {
    match event {
        Event::Health => connection.leader_exited().is_ok_and(|exited| !exited),
        Event::Review => true,
        Event::Command(command) => execute(connection, documents, diagnostics, command).await,
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

async fn execute(
    connection: &mut StdioConnection,
    documents: &mut super::documents::Documents,
    diagnostics: &mut Diagnostics,
    command: Command,
) -> bool {
    match command {
        Command::Save { .. } => unreachable!("save observations are retained by the worker"),
        Command::Open {
            path, text, reply, ..
        } => {
            if reply.is_closed() {
                return true;
            }
            let language = super::documents::language(&path);
            let result = documents.open(connection, path, text, language).await;
            let healthy = result.is_ok();
            let _ = reply.send(result);
            healthy
        }
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
            let _ = reply.send(diagnostics.take_raw());
            true
        }
        Command::Diagnostics { .. } => {
            unreachable!("diagnostic observations are retained by the worker")
        }
        Command::Feedback { .. } => {
            unreachable!("feedback observations are retained by the worker")
        }
    }
}
