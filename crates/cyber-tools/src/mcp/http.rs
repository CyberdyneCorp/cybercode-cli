//! Streamable HTTP and legacy HTTP+SSE transports for remote MCP servers beneath the shared
//! protocol client.
//!
//! An owned bridge task sits behind a duplex pipe: every newline frame the protocol client
//! writes becomes one POST, and JSON bodies or SSE `message` events come back as newline
//! frames. Any unexpected status, content type, oversized body or transport error records a
//! typed failure and closes the pipe, which the client observes as transport loss.
//!
//! A connect tries one transport and, only when its first initialize fails for a
//! non-authentication transport reason, joins that bridge and retries the other inside the
//! same absolute deadline. Authorization failures (401/403) never fall back.
//!
//! After initialization the Streamable HTTP bridge opens the optional GET event stream
//! (405 or another non-session client error means the server offers none) and pumps it into
//! the same pipe, so server notifications, requests and progress reach the one protocol
//! client. A stream that ends cleanly is reopened with `Last-Event-ID` after the server's
//! `retry` delay, and a request's POST event stream that ends before its response is resumed
//! the same way. A failed reopen, a POST transport error, a refusal or a 404 for a request
//! carrying the session closes the pipe as loss. Without a stream, the idle pump pings the
//! server on a bounded cadence.
//! Every HTTP request is owned by the bridge, so a timed-out RPC's request ends when the
//! owner shuts the unresolved connection down.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::connection::startup_cancelled;
use super::{
    ConnectionError, DiscoveredTool, IdleUpdate, McpError, StartupOptions, StderrCapture,
    StdioClient,
};

const SESSION: HeaderName = HeaderName::from_static("mcp-session-id");
const VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const STOP_ACK: Duration = Duration::from_secs(2);
const PIPE: usize = 64 * 1024;
/// Idle client pings run only without a GET stream, at most this often.
const PING_EVERY: Duration = Duration::from_secs(10);
/// An unanswered idle ping is loss after this long.
const PING_TIMEOUT: Duration = Duration::from_secs(5);
/// SSE reconnection delay when the server sends no `retry` field, and its bounds.
const RETRY_DEFAULT: Duration = Duration::from_secs(1);
const RETRY_MIN: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// Consecutive resumptions of one request's event stream that may end without a new id.
const RESUME_LIMIT: u32 = 3;
/// Longer event ids are ignored rather than echoed in a request header.
const EVENT_ID_LIMIT: usize = 1024;

type Client = StdioClient<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;
type Writer = Arc<tokio::sync::Mutex<WriteHalf<DuplexStream>>>;

/// Why the remote transport closed. Diagnostics never contain URLs or header values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpFailure {
    /// HTTP 401/403 with the server's parsed `Bearer` challenge. The challenge drives
    /// OAuth discovery and never appears in diagnostics.
    #[error("authorization required (HTTP {status})")]
    Unauthorized {
        status: u16,
        challenge: Option<Box<super::oauth::AuthChallenge>>,
    },
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("unsupported response content type")]
    ContentType,
    #[error("response exceeds 1 MiB")]
    Oversized,
    #[error("invalid response body")]
    Body,
    #[error("request failed: {0}")]
    Request(String),
    #[error("SSE endpoint is not same-origin with the configured URL")]
    Endpoint,
    /// HTTP 404 for a request that carried `Mcp-Session-Id`: the server ended the session.
    #[error("remote session expired")]
    SessionExpired,
}

impl HttpFailure {
    /// The scopes a 403 `insufficient_scope` challenge asks for (RFC 6750 §3.1), when this
    /// is one. Only RFC 6749 scope tokens are kept, at most [`SCOPE_LIMIT`]. A refreshed
    /// token carries the same scopes, so this is never cured by a refresh.
    pub fn insufficient_scope(&self) -> Option<Vec<String>> {
        let Self::Unauthorized {
            status: 403,
            challenge: Some(challenge),
        } = self
        else {
            return None;
        };
        if challenge.error.as_deref() != Some("insufficient_scope") {
            return None;
        }
        let scope = challenge.scope.as_deref().unwrap_or_default();
        Some(
            scope
                .split(' ')
                .filter(|token| scope_token(token))
                .take(SCOPE_LIMIT)
                .map(str::to_owned)
                .collect(),
        )
    }

    /// Failures that show the server does not speak the attempted transport. Authorization,
    /// size, body and endpoint-policy failures never select another transport.
    fn falls_back(&self) -> bool {
        matches!(
            self,
            Self::Status(400 | 404 | 405) | Self::ContentType | Self::Request(_)
        )
    }
}

/// At most this many scopes are taken from one `insufficient_scope` challenge.
const SCOPE_LIMIT: usize = 64;

/// An RFC 6749 §3.3 scope token: `%x21 / %x23-5B / %x5D-7E`, bounded.
fn scope_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 256
        && token
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

/// The remote transport a connection uses; reconnects try the last connected one first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteTransport {
    #[default]
    StreamableHttp,
    /// The legacy HTTP+SSE transport: a GET event stream names a same-origin POST endpoint.
    Sse,
}

impl RemoteTransport {
    fn other(self) -> Self {
        match self {
            Self::StreamableHttp => Self::Sse,
            Self::Sse => Self::StreamableHttp,
        }
    }
}

/// A policy-checked client, the endpoint and the configured request headers.
pub struct HttpTarget {
    pub client: reqwest::Client,
    pub url: reqwest::Url,
    pub headers: HeaderSet,
}

/// One remote server's request headers. A `headers_command` refresh replaces the whole set
/// at once, and every later request (POST, GET stream, DELETE) reads the current set.
#[derive(Clone, Default)]
pub struct HeaderSet(Arc<Mutex<HeaderMap>>);

impl HeaderSet {
    pub fn new(headers: HeaderMap) -> Self {
        Self(Arc::new(Mutex::new(headers)))
    }

    pub fn replace(&self, headers: HeaderMap) {
        *slot(&self.0) = headers;
    }

    pub fn current(&self) -> HeaderMap {
        slot(&self.0).clone()
    }
}

/// Whether the server-initiated GET event stream is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Listening {
    /// Not yet requested or answered.
    #[default]
    Pending,
    Open,
    /// The server offers no stream (405 or another non-session client error): liveness
    /// falls back to client pings.
    Absent,
}

/// State the bridge shares with its owner: session identity, negotiated version, the GET
/// stream state and the first failure.
#[derive(Default)]
struct Shared {
    session: Mutex<Option<HeaderValue>>,
    version: Mutex<Option<HeaderValue>>,
    failure: Mutex<Option<HttpFailure>>,
    listening: Mutex<Listening>,
}

fn slot<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn fail(&self, failure: HttpFailure) {
        slot(&self.failure).get_or_insert(failure);
    }

    fn failure(&self) -> Option<HttpFailure> {
        slot(&self.failure).clone()
    }

    fn has_session(&self) -> bool {
        slot(&self.session).is_some()
    }

    fn listen(&self, state: Listening) {
        *slot(&self.listening) = state;
    }

    /// Current configured headers plus the session and protocol version once negotiated.
    fn headers(&self, base: &HeaderSet) -> HeaderMap {
        let mut headers = base.current();
        if let Some(session) = slot(&self.session).clone() {
            headers.insert(SESSION, session);
        }
        if let Some(version) = slot(&self.version).clone() {
            headers.insert(VERSION, version);
        }
        headers
    }

    fn capture_session(&self, headers: &HeaderMap) {
        if let Some(session) = headers.get(SESSION) {
            *slot(&self.session) = Some(session.clone());
        }
    }

    fn capture_version(&self, message: &Value) {
        if let Some(version) = message["result"]["protocolVersion"]
            .as_str()
            .and_then(|version| HeaderValue::from_str(version).ok())
        {
            *slot(&self.version) = Some(version);
        }
    }
}

/// Authorization, network policy and durable admission belong to the caller.
/// Dropping this owner aborts the bridge but supplies no settlement acknowledgement.
pub struct HttpConnection {
    name: String,
    client: Option<Client>,
    bridge: Option<JoinHandle<()>>,
    stop: CancellationToken,
    shared: Arc<Shared>,
    target: Arc<HttpTarget>,
    transport: RemoteTransport,
    initialized: bool,
    pinged: tokio::time::Instant,
    /// Prompts discovered at startup, until the owner takes them.
    prompts: Vec<super::DiscoveredPrompt>,
}

/// One transport attempt's failure and whether it may select the other transport.
struct AttemptFailure {
    error: ConnectionError,
    fallback: bool,
}

/// Per-connect inputs reused by a fallback attempt.
struct Startup<'a> {
    name: &'a str,
    filter: &'a cyber_core::config::McpToolFilter,
    deadline: tokio::time::Instant,
    roots: Option<super::McpRoots>,
    sampling: bool,
    cancellation: Option<CancellationToken>,
}

impl HttpConnection {
    /// One absolute deadline covers initialization, any transport fallback and every
    /// initial listing page. `first` is tried first; the other transport is tried only
    /// after `first` failed its initialize for a non-authentication transport reason.
    pub async fn connect_with_options(
        target: HttpTarget,
        name: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
        options: StartupOptions,
        first: RemoteTransport,
    ) -> Result<(Self, Vec<DiscoveredTool>), ConnectionError> {
        let startup = Startup {
            name,
            filter,
            deadline: tokio::time::Instant::now() + timeout,
            roots: options.roots,
            sampling: options.sampling,
            cancellation: options.cancellation,
        };
        let target = Arc::new(target);
        match Self::attempt(target.clone(), &startup, first).await {
            Ok(connected) => Ok(connected),
            // The first bridge is already joined; the retry keeps the original deadline.
            Err(failure) if failure.fallback && failure.error.acknowledged => {
                Self::attempt(target, &startup, first.other())
                    .await
                    .map_err(|failure| failure.error)
            }
            Err(failure) => Err(failure.error),
        }
    }

    async fn attempt(
        target: Arc<HttpTarget>,
        startup: &Startup<'_>,
        transport: RemoteTransport,
    ) -> Result<(Self, Vec<DiscoveredTool>), AttemptFailure> {
        let mut connection = Self::open(target, startup.name, transport);
        let started = tokio::select! {
            biased;
            _ = startup_cancelled(startup.cancellation.as_ref()) => Err(McpError::Protocol("MCP startup cancelled")),
            result = connection.start(startup) => result,
        };
        let error = match started {
            Ok(tools) => return Ok((connection, tools)),
            Err(error) => connection.classify(error),
        };
        let fallback = !connection.initialized
            && !startup
                .cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            && matches!(&error, McpError::Http(failure) if failure.falls_back());
        let (acknowledged, stderr) = connection.shutdown().await;
        Err(AttemptFailure {
            error: ConnectionError {
                error,
                acknowledged,
                stderr,
            },
            fallback,
        })
    }

    fn open(target: Arc<HttpTarget>, name: &str, transport: RemoteTransport) -> Self {
        let (local, remote) = tokio::io::duplex(PIPE);
        let (reader, writer) = tokio::io::split(local);
        let shared = Arc::new(Shared::default());
        let stop = CancellationToken::new();
        let bridge = match transport {
            RemoteTransport::StreamableHttp => {
                tokio::spawn(bridge(target.clone(), shared.clone(), remote, stop.clone()))
            }
            RemoteTransport::Sse => tokio::spawn(legacy_bridge(
                target.clone(),
                shared.clone(),
                remote,
                stop.clone(),
            )),
        };
        Self {
            name: name.into(),
            client: Some(StdioClient::new(reader, writer)),
            bridge: Some(bridge),
            stop,
            shared,
            target,
            transport,
            initialized: false,
            pinged: tokio::time::Instant::now(),
            prompts: Vec::new(),
        }
    }

    /// The transport this connection uses.
    pub fn transport(&self) -> RemoteTransport {
        self.transport
    }

    async fn start(&mut self, startup: &Startup<'_>) -> Result<Vec<DiscoveredTool>, McpError> {
        let client = self
            .client
            .take()
            .ok_or(McpError::Protocol("MCP transport is closed"))?;
        let client = match startup.roots.clone() {
            Some(roots) => client.with_roots(roots)?,
            None => client,
        };
        let client = self.client.insert(
            client
                .with_form_elicitation(startup.name)?
                .with_sampling(startup.sampling)?,
        );
        let deadline = startup.deadline;
        let remaining = || deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining().is_zero() {
            return Err(McpError::Timeout);
        }
        tokio::time::timeout_at(deadline, client.initialize(remaining()))
            .await
            .map_err(|_| McpError::Timeout)??;
        self.initialized = true;
        let tools = self
            .client()?
            .discover_tools(startup.name, startup.filter, remaining())
            .await?;
        self.prompts = self.discover_prompts(startup.name, remaining()).await?;
        Ok(tools)
    }

    /// Report the bridge's typed failure in place of the resulting pipe closure.
    fn classify(&self, error: McpError) -> McpError {
        match (self.shared.failure(), error) {
            (Some(failure), McpError::Io(_) | McpError::Protocol(_)) => McpError::Http(failure),
            (_, error) => error,
        }
    }

    fn client(&mut self) -> Result<&mut Client, McpError> {
        self.client
            .as_mut()
            .ok_or(McpError::Protocol("MCP transport is closed"))
    }

    fn bridge_closed(&self) -> bool {
        self.bridge.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub async fn poll_idle(&mut self, timeout: Duration) -> Result<IdleUpdate, McpError> {
        if self.bridge_closed() || self.client.is_none() {
            return Ok(IdleUpdate {
                closed: true,
                ..Default::default()
            });
        }
        let update = self.client()?.poll_idle(timeout).await;
        let update = update.map_err(|error| self.classify(error))?;
        if update.closed || !self.ping_due() {
            return Ok(update);
        }
        self.pinged = tokio::time::Instant::now();
        // Any JSON-RPC answer, including an error, proves the session is alive.
        match self.client()?.ping(PING_TIMEOUT).await {
            Ok(_) | Err(McpError::Remote(_)) => Ok(update),
            Err(error) => Err(self.classify(error)),
        }
    }

    /// Without a GET stream nothing else proves liveness between calls.
    fn ping_due(&self) -> bool {
        *slot(&self.shared.listening) == Listening::Absent && self.pinged.elapsed() >= PING_EVERY
    }

    pub fn clear_changed(&mut self, change: super::ListChange) {
        if let Some(client) = self.client.as_mut() {
            client.clear_changed(change);
        }
    }

    /// The prompts discovered at startup; later calls return none.
    pub fn take_prompts(&mut self) -> Vec<super::DiscoveredPrompt> {
        std::mem::take(&mut self.prompts)
    }

    pub async fn disconnected(&mut self) -> Result<bool, McpError> {
        if self.bridge_closed() {
            return Ok(true);
        }
        match self.client.as_mut() {
            Some(client) => client.transport_closed().await,
            None => Ok(true),
        }
    }

    pub fn unresolved(&self) -> bool {
        self.client.as_ref().is_some_and(StdioClient::unresolved)
    }

    /// The authorization refusal (HTTP 401/403) that ended this connection, if any.
    pub fn auth_refusal(&self) -> Option<HttpFailure> {
        self.shared
            .failure()
            .filter(|failure| matches!(failure, HttpFailure::Unauthorized { .. }))
    }

    pub fn metadata(&self) -> Option<&Value> {
        self.client.as_ref().and_then(StdioClient::metadata)
    }

    pub async fn discover_tools(
        &mut self,
        name: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
    ) -> Result<Vec<DiscoveredTool>, McpError> {
        let result = self.client()?.discover_tools(name, filter, timeout).await;
        result.map_err(|error| self.classify(error))
    }

    /// Prompts are optional: a rejected or invalid listing leaves none and keeps the
    /// connection, while a transport failure still ends it.
    pub async fn discover_prompts(
        &mut self,
        name: &str,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredPrompt>, McpError> {
        let result = self.client()?.discover_prompts(name, timeout).await;
        let result = result.map_err(|error| self.classify(error));
        super::prompts::optional(name, result, self.unresolved())
    }

    pub async fn get_prompt(
        &mut self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
        timeout: Duration,
    ) -> Result<Vec<super::PromptMessage>, McpError> {
        let result = self.client()?.get_prompt(name, arguments, timeout).await;
        result.map_err(|error| self.classify(error))
    }

    pub async fn call_tool_with_elicitation(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<Value, McpError> {
        let result = self
            .client()?
            .call_tool_with_elicitation(name, arguments, timeout, context)
            .await;
        result.map_err(|error| self.classify(error))
    }

    pub async fn resource(
        &mut self,
        request: super::ResourceRequest<'_>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        let result = self.client()?.resource(request, timeout).await;
        result.map_err(|error| self.classify(error))
    }

    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.client()?.cancel_pending(timeout).await
    }

    /// HTTP has no process group to drain; graceful and immediate shutdown are the same.
    pub async fn shutdown_gracefully(&mut self) -> (bool, StderrCapture) {
        self.shutdown().await
    }

    /// Abort and join the bridge with its in-flight POSTs, then end the remote session.
    /// The acknowledgement covers local tasks only; the caller still stops the proxy.
    pub async fn shutdown(&mut self) -> (bool, StderrCapture) {
        drop(self.client.take());
        self.stop.cancel();
        if let Some(task) = self.bridge.as_mut() {
            if tokio::time::timeout(STOP_ACK, &mut *task).await.is_err() {
                task.abort();
                let _ = (&mut *task).await;
            }
            self.bridge.take();
        }
        if self.shared.failure() != Some(HttpFailure::SessionExpired) {
            self.delete_session().await;
        }
        (true, StderrCapture::default())
    }

    /// DELETE the session under a short deadline; 405 means the server keeps no
    /// client-terminable sessions. Failures are redacted diagnostics only.
    async fn delete_session(&self) {
        let Some(session) = slot(&self.shared.session).take() else {
            return;
        };
        let mut headers = self.shared.headers(&self.target.headers);
        headers.insert(SESSION, session);
        let request = self
            .target
            .client
            .delete(self.target.url.clone())
            .headers(headers)
            .send();
        let problem = match tokio::time::timeout(STOP_ACK, request).await {
            Ok(Ok(response))
                if response.status().is_success() || response.status().as_u16() == 405 =>
            {
                return;
            }
            Ok(Ok(response)) => format!("HTTP status {}", response.status().as_u16()),
            Ok(Err(error)) => error.without_url().to_string(),
            Err(_) => "timed out".into(),
        };
        cyber_core::log::error(
            "mcp",
            &format!("MCP remote session close failed: {problem}"),
            json!({"server":self.name}),
        );
    }
}

impl Drop for HttpConnection {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.bridge.take() {
            task.abort();
        }
    }
}

/// One frame written by the protocol client.
struct Frame {
    bytes: Vec<u8>,
    /// The JSON-RPC id of a client request; `None` for notifications and responses.
    id: Option<Value>,
    request: bool,
    initialize: bool,
    /// `notifications/initialized`: once accepted, the GET event stream may open.
    initialized: bool,
}

impl Frame {
    fn parse(bytes: Vec<u8>) -> Option<Self> {
        let message: Value = serde_json::from_slice(&bytes).ok()?;
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").filter(|_| method.is_some()).cloned();
        Some(Self {
            request: id.is_some(),
            id,
            initialize: method == Some("initialize"),
            initialized: method == Some("notifications/initialized"),
            bytes,
        })
    }
}

/// Cancellation-safe newline framing over the client side of the pipe.
struct Frames {
    reader: ReadHalf<DuplexStream>,
    buffer: Vec<u8>,
    scanned: usize,
}

impl Frames {
    async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(position) = self.buffer[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let mut frame: Vec<u8> = self.buffer.drain(..=self.scanned + position).collect();
                frame.pop();
                self.scanned = 0;
                return Some(frame);
            }
            self.scanned = self.buffer.len();
            if self.buffer.len() > super::LIMIT {
                return None;
            }
            let mut chunk = [0; 8192];
            let count = self.reader.read(&mut chunk).await.ok()?;
            if count == 0 {
                return None;
            }
            self.buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

/// Requests run concurrently so long calls never block notifications or callback
/// responses; notifications and responses are delivered in order before the next frame.
async fn bridge(
    target: Arc<HttpTarget>,
    shared: Arc<Shared>,
    pipe: DuplexStream,
    stop: CancellationToken,
) {
    let (reader, writer) = tokio::io::split(pipe);
    let writer: Writer = Arc::new(tokio::sync::Mutex::new(writer));
    let failed = stop.child_token();
    let mut frames = Frames {
        reader,
        buffer: Vec::new(),
        scanned: 0,
    };
    let mut posts = JoinSet::new();
    let send = |frame| {
        post(
            target.clone(),
            shared.clone(),
            writer.clone(),
            frame,
            failed.clone(),
        )
    };
    loop {
        let frame = tokio::select! {
            biased;
            _ = failed.cancelled() => break,
            Some(_) = posts.join_next(), if !posts.is_empty() => continue,
            frame = frames.next() => frame,
        };
        let Some(frame) = frame.and_then(Frame::parse) else {
            break;
        };
        if frame.request {
            posts.spawn(send(frame));
            continue;
        }
        let opens = frame.initialized;
        tokio::select! {
            biased;
            _ = failed.cancelled() => break,
            _ = send(frame) => {},
        }
        if opens && !failed.is_cancelled() {
            posts.spawn(listen(
                target.clone(),
                shared.clone(),
                writer.clone(),
                failed.clone(),
            ));
        }
    }
    posts.shutdown().await;
    let _ = writer.lock().await.shutdown().await;
}

async fn post(
    target: Arc<HttpTarget>,
    shared: Arc<Shared>,
    writer: Writer,
    frame: Frame,
    failed: CancellationToken,
) {
    if let Err(failure) = exchange(&target, &shared, &writer, frame).await {
        shared.fail(failure);
        failed.cancel();
    }
}

/// The optional server-initiated event stream. Its failure is transport loss.
async fn listen(
    target: Arc<HttpTarget>,
    shared: Arc<Shared>,
    writer: Writer,
    failed: CancellationToken,
) {
    if let Err(failure) = event_stream(&target, &shared, &writer).await {
        shared.fail(failure);
        failed.cancel();
    }
}

/// A stream that ends cleanly (an intermediary idle cutoff, or a server that polls) is
/// reopened with `Last-Event-ID` after the server's `retry` delay. Only a reopen that fails
/// at the transport, is refused or finds the session gone is loss; a server that offers no
/// stream leaves liveness to client pings.
async fn event_stream(
    target: &HttpTarget,
    shared: &Shared,
    writer: &Writer,
) -> Result<(), HttpFailure> {
    let delivery = Delivery {
        writer,
        shared,
        initialize: false,
    };
    let mut cursor = Cursor::default();
    loop {
        let last = cursor.last_id.clone();
        let Some(response) = open_stream(target, shared, last.as_ref()).await? else {
            shared.listen(Listening::Absent);
            return Ok(());
        };
        shared.listen(Listening::Open);
        delivery.events(response, &mut cursor, None).await?;
        tokio::time::sleep(cursor.delay()).await;
    }
}

/// GET the server's event stream, resuming after `last` when set. `None` means the server
/// offers no stream: 405, or another client error that is neither an authorization outcome
/// nor a 404 for the live session.
async fn open_stream(
    target: &HttpTarget,
    shared: &Shared,
    last: Option<&HeaderValue>,
) -> Result<Option<reqwest::Response>, HttpFailure> {
    let session = shared.has_session();
    let mut headers = shared.headers(&target.headers);
    if let Some(last) = last {
        headers.insert(LAST_EVENT_ID, last.clone());
    }
    let response = target
        .client
        .get(target.url.clone())
        .headers(headers)
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(request_failure)?;
    let status = response.status().as_u16();
    match checked(&response, session) {
        Ok(()) => {}
        Err(HttpFailure::Status(400..=499)) => {
            if status != 405 {
                cyber_core::log::error(
                    "mcp",
                    &format!("MCP remote event stream unavailable (HTTP {status})"),
                    json!({}),
                );
            }
            return Ok(None);
        }
        Err(failure) => return Err(failure),
    }
    if !matches!(media(&response), Media::Events) {
        return Err(HttpFailure::ContentType);
    }
    Ok(Some(response))
}

/// Resume a request's event stream that ended before its response (SSE polling): GET with
/// `Last-Event-ID` after the server's `retry` delay. A stream that carried no event id
/// cannot be resumed, and resumptions that bring no new event id are bounded, so the call
/// fails promptly instead of waiting out its timeout.
async fn resume(
    target: &HttpTarget,
    shared: &Shared,
    delivery: &Delivery<'_>,
    mut cursor: Cursor,
    id: Option<&Value>,
) -> Result<(), HttpFailure> {
    let mut stalled = 0;
    while stalled < RESUME_LIMIT {
        let Some(last) = cursor.last_id.clone() else {
            break;
        };
        tokio::time::sleep(cursor.delay()).await;
        let Some(response) = open_stream(target, shared, Some(&last)).await? else {
            break;
        };
        if delivery.events(response, &mut cursor, id).await? {
            return Ok(());
        }
        stalled = if cursor.last_id.as_ref() == Some(&last) {
            stalled + 1
        } else {
            0
        };
    }
    Err(HttpFailure::Request(
        "event stream closed before response".into(),
    ))
}

/// 401/403 are authorization outcomes and 404 for a request carrying the session means
/// the server ended it; any other non-success status is a plain failure.
fn checked(response: &reqwest::Response, session: bool) -> Result<(), HttpFailure> {
    match response.status().as_u16() {
        404 if session => Err(HttpFailure::SessionExpired),
        _ => accepted(response),
    }
}

async fn exchange(
    target: &HttpTarget,
    shared: &Shared,
    writer: &Writer,
    frame: Frame,
) -> Result<(), HttpFailure> {
    let session = shared.has_session();
    let id = frame.id;
    let response = target
        .client
        .post(target.url.clone())
        .headers(shared.headers(&target.headers))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(frame.bytes)
        .send()
        .await
        .map_err(request_failure)?;
    let status = response.status().as_u16();
    checked(&response, session)?;
    match (status, frame.request) {
        (202, false) => return Ok(()),
        (202, true) => return Err(HttpFailure::Status(status)),
        _ => {}
    }
    if frame.initialize {
        shared.capture_session(response.headers());
    }
    let delivery = Delivery {
        writer,
        shared,
        initialize: frame.initialize,
    };
    match media(&response) {
        Media::Json => {
            let body = read_body(response).await?;
            if body.is_empty() && !frame.request {
                return Ok(());
            }
            delivery.deliver(&body).await.map(drop)
        }
        Media::Events => {
            let mut cursor = Cursor::default();
            if delivery.events(response, &mut cursor, id.as_ref()).await? || id.is_none() {
                return Ok(());
            }
            resume(target, shared, &delivery, cursor, id.as_ref()).await
        }
        Media::Other if !frame.request && response.content_length() == Some(0) => Ok(()),
        Media::Other => Err(HttpFailure::ContentType),
    }
}

fn request_failure(error: reqwest::Error) -> HttpFailure {
    HttpFailure::Request(error.without_url().to_string())
}

enum Media {
    Json,
    Events,
    Other,
}

fn media(response: &reqwest::Response) -> Media {
    let essence = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "application/json" => Media::Json,
        "text/event-stream" => Media::Events,
        _ => Media::Other,
    }
}

async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, HttpFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > super::LIMIT as u64)
    {
        return Err(HttpFailure::Oversized);
    }
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(request_failure)?;
        if chunk.len() > super::LIMIT - bytes.len() {
            return Err(HttpFailure::Oversized);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct Delivery<'a> {
    writer: &'a Writer,
    shared: &'a Shared,
    initialize: bool,
}

impl Delivery<'_> {
    /// Deliver every message of one event stream and record where it ended in `cursor`.
    /// Returns whether the stream carried the response to request `id`.
    async fn events(
        &self,
        response: reqwest::Response,
        cursor: &mut Cursor,
        id: Option<&Value>,
    ) -> Result<bool, HttpFailure> {
        let mut events = EventStream::new(response);
        let mut answered = false;
        while let Some(event) = events.next().await? {
            if let Event::Message(data) = event {
                answered |= responds(&self.deliver(&data).await?, id);
            }
        }
        events.parser.save(cursor);
        Ok(answered)
    }

    /// Re-encode one JSON message as a single bounded newline frame.
    async fn deliver(&self, data: &[u8]) -> Result<Value, HttpFailure> {
        let message: Value = serde_json::from_slice(data).map_err(|_| HttpFailure::Body)?;
        if self.initialize {
            self.shared.capture_version(&message);
        }
        let mut bytes = super::stdio::encode(&message).map_err(|_| HttpFailure::Oversized)?;
        bytes.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&bytes)
            .await
            .map_err(|_| HttpFailure::Request("protocol client closed".into()))?;
        writer
            .flush()
            .await
            .map_err(|_| HttpFailure::Request("protocol client closed".into()))?;
        Ok(message)
    }
}

/// Whether `message` is the response to the client request `id`.
fn responds(message: &Value, id: Option<&Value>) -> bool {
    id.is_some_and(|id| message.get("method").is_none() && message.get("id") == Some(id))
}

/// Where a resumable event stream left off: its last event id and the server's `retry`.
#[derive(Default)]
struct Cursor {
    last_id: Option<HeaderValue>,
    retry: Option<Duration>,
}

impl Cursor {
    fn delay(&self) -> Duration {
        self.retry
            .unwrap_or(RETRY_DEFAULT)
            .clamp(RETRY_MIN, RETRY_MAX)
    }
}

/// The legacy HTTP+SSE bridge: one GET event stream whose first event names the POST
/// endpoint. Client frames are POSTed there in order; every reply arrives on the stream.
async fn legacy_bridge(
    target: Arc<HttpTarget>,
    shared: Arc<Shared>,
    pipe: DuplexStream,
    stop: CancellationToken,
) {
    let (reader, writer) = tokio::io::split(pipe);
    let writer: Writer = Arc::new(tokio::sync::Mutex::new(writer));
    let frames = Frames {
        reader,
        buffer: Vec::new(),
        scanned: 0,
    };
    let result = tokio::select! {
        biased;
        _ = stop.cancelled() => Ok(()),
        result = legacy_session(&target, &shared, &writer, frames) => result,
    };
    if let Err(failure) = result {
        shared.fail(failure);
    }
    let _ = writer.lock().await.shutdown().await;
}

async fn legacy_session(
    target: &HttpTarget,
    shared: &Shared,
    writer: &Writer,
    mut frames: Frames,
) -> Result<(), HttpFailure> {
    let response = target
        .client
        .get(target.url.clone())
        .headers(target.headers.current())
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(request_failure)?;
    accepted(&response)?;
    if !matches!(media(&response), Media::Events) {
        return Err(HttpFailure::ContentType);
    }
    let mut events = EventStream::new(response);
    let endpoint = legacy_endpoint(&target.url, &mut events).await?;
    let delivery = Delivery {
        writer,
        shared,
        initialize: false,
    };
    let pump = async {
        while let Some(event) = events.next().await? {
            if let Event::Message(data) = event {
                delivery.deliver(&data).await?;
            }
        }
        Err::<(), _>(HttpFailure::Request("event stream closed".into()))
    };
    let send = async {
        while let Some(frame) = frames.next().await.and_then(Frame::parse) {
            legacy_post(target, &endpoint, frame).await?;
        }
        Ok(())
    };
    tokio::select! {
        result = pump => result,
        result = send => result,
    }
}

/// 401/403 are authorization outcomes carrying any `Bearer` challenge; any other
/// non-success status is a plain failure.
fn accepted(response: &reqwest::Response) -> Result<(), HttpFailure> {
    match response.status().as_u16() {
        status @ (401 | 403) => Err(HttpFailure::Unauthorized {
            status,
            challenge: super::oauth::AuthChallenge::from_headers(response.headers()).map(Box::new),
        }),
        200..=299 => Ok(()),
        status => Err(HttpFailure::Status(status)),
    }
}

/// Wait for the first `endpoint` event and refuse any endpoint outside the configured
/// origin, so headers and messages never reach another scheme, host or port.
async fn legacy_endpoint(
    base: &reqwest::Url,
    events: &mut EventStream,
) -> Result<reqwest::Url, HttpFailure> {
    let data = match events.next().await? {
        Some(Event::Endpoint(data)) => data,
        Some(Event::Message(_)) => return Err(HttpFailure::Body),
        None => return Err(HttpFailure::Request("event stream closed".into())),
    };
    let endpoint = std::str::from_utf8(&data)
        .ok()
        .and_then(|text| base.join(text.trim()).ok())
        .ok_or(HttpFailure::Endpoint)?;
    if endpoint.origin() != base.origin()
        || endpoint.username() != base.username()
        || endpoint.password() != base.password()
    {
        return Err(HttpFailure::Endpoint);
    }
    Ok(endpoint)
}

async fn legacy_post(
    target: &HttpTarget,
    endpoint: &reqwest::Url,
    frame: Frame,
) -> Result<(), HttpFailure> {
    let response = target
        .client
        .post(endpoint.clone())
        .headers(target.headers.current())
        .header(CONTENT_TYPE, "application/json")
        .body(frame.bytes)
        .send()
        .await
        .map_err(request_failure)?;
    // The acknowledgement body (e.g. "Accepted") carries no JSON-RPC and is discarded.
    accepted(&response)
}

/// Bounded events decoded from one `text/event-stream` response body.
struct EventStream {
    chunks: futures::stream::BoxStream<'static, reqwest::Result<Vec<u8>>>,
    parser: EventParser,
    ready: VecDeque<Event>,
}

impl EventStream {
    fn new(response: reqwest::Response) -> Self {
        Self {
            chunks: response
                .bytes_stream()
                .map(|chunk| chunk.map(Vec::from))
                .boxed(),
            parser: EventParser::default(),
            ready: VecDeque::new(),
        }
    }

    async fn next(&mut self) -> Result<Option<Event>, HttpFailure> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Ok(Some(event));
            }
            let Some(chunk) = self.chunks.next().await else {
                return Ok(None);
            };
            let events = self.parser.push(&chunk.map_err(request_failure)?)?;
            self.ready.extend(events);
        }
    }
}

/// A dispatched event that carries meaning for MCP.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    /// A JSON-RPC message (`message` or unnamed events).
    Message(Vec<u8>),
    /// The legacy transport's POST endpoint.
    Endpoint(Vec<u8>),
}

/// Minimal `text/event-stream` decoding: data lines of `message` and `endpoint` events,
/// bounded per line and per event.
#[derive(Default)]
struct EventParser {
    line: Vec<u8>,
    data: Vec<u8>,
    event: Vec<u8>,
    has_data: bool,
    /// The last `id` field: `Some(empty)` resets it, `None` means none was seen.
    last_id: Option<Vec<u8>>,
    /// The last valid `retry` field, in milliseconds.
    retry: Option<u64>,
}

impl EventParser {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Event>, HttpFailure> {
        let mut events = Vec::new();
        let mut rest = chunk;
        while let Some(position) = rest.iter().position(|byte| *byte == b'\n') {
            self.extend(&rest[..position])?;
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            events.extend(self.line(&line)?);
            rest = &rest[position + 1..];
        }
        self.extend(rest)?;
        Ok(events)
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), HttpFailure> {
        // A data line may carry a full message plus its field name and separator.
        if self.line.len() + bytes.len() > super::LIMIT + 16 {
            return Err(HttpFailure::Oversized);
        }
        self.line.extend_from_slice(bytes);
        Ok(())
    }

    fn line(&mut self, line: &[u8]) -> Result<Option<Event>, HttpFailure> {
        if line.is_empty() {
            return Ok(self.dispatch());
        }
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(position) => {
                let value = &line[position + 1..];
                (&line[..position], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &[][..]),
        };
        match field {
            b"data" => self.data(value)?,
            b"event" => self.event = value.to_vec(),
            b"id" if !value.contains(&0) && value.len() <= EVENT_ID_LIMIT => {
                self.last_id = Some(value.to_vec());
            }
            b"retry" if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
                self.retry = std::str::from_utf8(value).ok().and_then(|v| v.parse().ok());
            }
            _ => {}
        }
        Ok(None)
    }

    fn data(&mut self, value: &[u8]) -> Result<(), HttpFailure> {
        if self.has_data {
            self.data.push(b'\n');
        }
        if self.data.len() + value.len() > super::LIMIT {
            return Err(HttpFailure::Oversized);
        }
        self.data.extend_from_slice(value);
        self.has_data = true;
        Ok(())
    }

    /// Carry this stream's last event id and retry delay into `cursor`; fields the stream
    /// never sent keep their earlier values.
    fn save(&self, cursor: &mut Cursor) {
        if let Some(id) = &self.last_id {
            cursor.last_id = HeaderValue::from_bytes(id).ok().filter(|_| !id.is_empty());
        }
        if let Some(retry) = self.retry {
            cursor.retry = Some(Duration::from_millis(retry));
        }
    }

    /// Empty-data priming events and other named events carry no JSON-RPC.
    fn dispatch(&mut self) -> Option<Event> {
        let data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if !std::mem::replace(&mut self.has_data, false) || data.iter().all(u8::is_ascii_whitespace)
        {
            return None;
        }
        match event.as_slice() {
            b"" | b"message" => Some(Event::Message(data)),
            b"endpoint" => Some(Event::Endpoint(data)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_parser_joins_data_lines_skips_comments_priming_and_other_events() {
        let mut parser = EventParser::default();
        let mut events = parser
            .push(b": comment\nid: 1\ndata:\n\nevent: other\ndata: {\"x\":0}\n\n")
            .unwrap();
        assert!(events.is_empty());
        events.extend(parser.push(b"event: message\r\ndata: {\"a\":\r\n").unwrap());
        assert!(events.is_empty());
        events.extend(parser.push(b"data: 1}\r\n\r\ndata:{\"b\":2}\n\n").unwrap());
        events.extend(
            parser
                .push(b"event: endpoint\ndata: /messages?id=1\n\n")
                .unwrap(),
        );
        assert_eq!(
            events,
            vec![
                Event::Message(b"{\"a\":\n1}".to_vec()),
                Event::Message(b"{\"b\":2}".to_vec()),
                Event::Endpoint(b"/messages?id=1".to_vec())
            ]
        );
    }

    #[test]
    fn event_parser_tracks_the_resumption_cursor() {
        let mut cursor = Cursor::default();
        assert_eq!(cursor.delay(), RETRY_DEFAULT);
        let mut parser = EventParser::default();
        assert!(
            parser
                .push(b"id: p-1\nretry: 250\ndata:\n\n")
                .unwrap()
                .is_empty()
        );
        parser.save(&mut cursor);
        assert_eq!(cursor.last_id, Some(HeaderValue::from_static("p-1")));
        assert_eq!(cursor.delay(), Duration::from_millis(250));
        // A stream that sends neither field keeps the earlier cursor.
        EventParser::default().save(&mut cursor);
        assert_eq!(cursor.last_id, Some(HeaderValue::from_static("p-1")));
        let mut parser = EventParser::default();
        parser
            .push(b"id: bad\0id\nretry: soon\nretry: 1\ndata: {}\n\n")
            .unwrap();
        parser.save(&mut cursor);
        assert_eq!(cursor.last_id, Some(HeaderValue::from_static("p-1")));
        assert_eq!(cursor.delay(), RETRY_MIN);
        // An empty id resets the cursor, so no Last-Event-ID is sent.
        let mut parser = EventParser::default();
        parser.push(b"id\nretry: 999999999\n\n").unwrap();
        parser.save(&mut cursor);
        assert_eq!(cursor.last_id, None);
        assert_eq!(cursor.delay(), RETRY_MAX);
    }

    #[test]
    fn unauthorized_display_carries_no_header_values() {
        let failure = HttpFailure::Unauthorized {
            status: 401,
            challenge: super::super::oauth::AuthChallenge::parse(
                r#"Bearer realm="private-realm", resource_metadata="https://private.test/prm", scope="private-scope", error="insufficient_scope""#,
            )
            .map(Box::new),
        };
        assert!(
            matches!(&failure, HttpFailure::Unauthorized { challenge: Some(c), .. } if c.scope.as_deref() == Some("private-scope"))
        );
        let shown = failure.to_string();
        assert_eq!(shown, "authorization required (HTTP 401)");
        // A 401 is never a scope step-up, whatever its error code.
        assert_eq!(failure.insufficient_scope(), None);
        assert!(!format!("{:?}", McpError::Http(failure)).contains("private"));
    }

    #[test]
    fn insufficient_scope_is_a_403_with_that_error_and_keeps_valid_scope_tokens() {
        let refusal = |status, header: &str| HttpFailure::Unauthorized {
            status,
            challenge: super::super::oauth::AuthChallenge::parse(header).map(Box::new),
        };
        let failure = refusal(
            403,
            r#"Bearer error="insufficient_scope", scope="files:read  files:write bad\"quote", error_description="private-detail""#,
        );
        assert_eq!(
            failure.insufficient_scope(),
            Some(vec!["files:read".into(), "files:write".into()])
        );
        assert_eq!(failure.to_string(), "authorization required (HTTP 403)");
        assert!(!format!("{failure:?}").contains("private"));
        // Without a scope it is still a step-up, with nothing to add.
        let bare = refusal(403, r#"Bearer error="insufficient_scope""#);
        assert_eq!(bare.insufficient_scope(), Some(Vec::new()));
        for other in [
            refusal(403, r#"Bearer error="invalid_token", scope="a""#),
            refusal(403, "Basic realm=x"),
            HttpFailure::Status(403),
        ] {
            assert_eq!(other.insufficient_scope(), None, "{other:?}");
        }
    }

    #[test]
    fn event_parser_bounds_lines_and_events() {
        let mut parser = EventParser::default();
        let long = vec![b'x'; super::super::LIMIT + 32];
        assert_eq!(parser.push(&long), Err(HttpFailure::Oversized));
        let mut parser = EventParser::default();
        let half = vec![b'x'; super::super::LIMIT / 2 + 1];
        for _ in 0..2 {
            let mut line = b"data: ".to_vec();
            line.extend_from_slice(&half);
            line.push(b'\n');
            if parser.push(&line).is_err() {
                return;
            }
        }
        panic!("an event over 1 MiB was accepted");
    }
}
