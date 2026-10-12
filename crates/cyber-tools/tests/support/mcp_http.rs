//! In-process Streamable HTTP (and optional legacy HTTP+SSE) MCP fixture bound to 127.0.0.1.
//! Shared by the tools, app and CLI suites through `#[path]`, so it depends only on axum,
//! futures, serde_json and tokio.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

pub const SESSION: &str = "fixture-session";

/// One request the fixture received, with lowercased header names.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub status: u16,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
    pub fn rpc(&self) -> Option<&str> {
        self.body["method"].as_str()
    }
}

#[derive(Clone, Default)]
pub struct Behavior {
    /// Answer `tools/call` with text/event-stream: a progress notification, then the result.
    pub sse: bool,
    /// Delay before each SSE event.
    pub delay: Duration,
    /// Answer `tools/list` with a JSON body over 1 MiB.
    pub oversized: bool,
    /// Answer every POST with this status and no body.
    pub status: Option<u16>,
    /// Answer every POST with 302 to this location.
    pub redirect: Option<String>,
    /// Status for DELETE (default 200).
    pub delete_status: Option<u16>,
    /// Serve the legacy HTTP+SSE transport: GET `/mcp` streams an `endpoint` event, and
    /// POSTs to that endpoint are answered with 202 while replies arrive on the stream.
    pub legacy: bool,
    /// Override the legacy `endpoint` event data (default `/messages?sessionId=legacy`).
    pub endpoint: Option<String>,
    /// Delay before a `status` reply and before the legacy `endpoint` event.
    pub slow: Duration,
    /// Serve the optional Streamable HTTP GET event stream (default: GET answers 405).
    pub stream: bool,
    /// Never answer client `ping` requests.
    pub ping_hang: bool,
    /// Answer every GET `/mcp` with this status and no body.
    pub get_status: Option<u16>,
    /// Answer every legacy endpoint POST with this status and no body.
    pub message_status: Option<u16>,
    /// Issue no `Mcp-Session-Id` from initialize.
    pub sessionless: bool,
    /// Answer `tools/call` with an event stream holding only a priming event (an id and a
    /// 100 ms `retry`), then deliver the result to a GET resuming with that `Last-Event-ID`.
    pub poll: bool,
    /// Answer `tools/call` with an event stream that ends without an event id or a result.
    pub truncate: bool,
}

/// State tests change while the fixture runs.
#[derive(Default)]
struct Control {
    /// Tool names `tools/list` returns (default: `echo`).
    tools: Option<Vec<String>>,
    /// Answer every POST and GET with this status and no body.
    fail: Option<u16>,
    /// The live session; requests carrying any other session get 404.
    session: Option<String>,
    sessions: u32,
    /// Answer every POST and GET without exactly this `Authorization` value with 401.
    authorization: Option<String>,
    /// A polled `tools/call` result awaiting its resuming GET.
    pending: Option<String>,
    /// Answer every later `initialize` POST with this status and no body.
    initialize: Option<u16>,
}

type Stream = futures::channel::mpsc::UnboundedSender<String>;

#[derive(Clone)]
struct Shared {
    behavior: Behavior,
    seen: Arc<Mutex<Vec<Seen>>>,
    stream: Arc<Mutex<Option<Stream>>>,
    control: Arc<Mutex<Control>>,
}

pub struct McpHttp {
    /// The MCP endpoint, `http://127.0.0.1:PORT/mcp`.
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    state: Shared,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl McpHttp {
    pub async fn start(behavior: Behavior) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Shared {
            behavior,
            seen: seen.clone(),
            stream: Arc::default(),
            control: Arc::default(),
        };
        let router = axum::Router::new()
            .route("/mcp", axum::routing::any(handle))
            .route("/messages", axum::routing::post(legacy_message))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            url,
            seen,
            state,
            task: Some(task),
        }
    }

    /// Whether a GET event stream is currently open.
    pub fn stream_open(&self) -> bool {
        self.state.stream.lock().unwrap().is_some()
    }

    /// Send one JSON-RPC message on the open GET event stream.
    pub fn push(&self, message: Value) -> bool {
        self.state
            .stream
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|sender| sender.unbounded_send(message.to_string()).is_ok())
    }

    /// End the open GET event stream.
    pub fn drop_stream(&self) {
        self.state.stream.lock().unwrap().take();
    }

    /// Replace the tool names later `tools/list` requests return.
    pub fn set_tools(&self, names: &[&str]) {
        self.state.control.lock().unwrap().tools =
            Some(names.iter().map(|name| (*name).to_string()).collect());
    }

    /// Answer every later POST and GET with `code` (or stop failing with `None`).
    pub fn fail_with(&self, code: Option<u16>) {
        self.state.control.lock().unwrap().fail = code;
    }

    /// Require exactly this `Authorization` header on later POSTs and GETs (`None` stops).
    pub fn require_authorization(&self, value: Option<&str>) {
        self.state.control.lock().unwrap().authorization = value.map(str::to_owned);
    }

    /// Answer later `initialize` POSTs with `code` (or stop with `None`); other requests
    /// still reach the live session, so a reconnect can be refused on its own.
    pub fn fail_initialize(&self, code: Option<u16>) {
        self.state.control.lock().unwrap().initialize = code;
    }

    /// Forget the live session: requests carrying it now get 404.
    pub fn expire_session(&self) {
        self.state.control.lock().unwrap().session = None;
    }

    /// Every client message POSTed to `/mcp` whose JSON-RPC `id` is `id`.
    pub fn replies(&self, id: &str) -> Vec<Seen> {
        self.requests()
            .into_iter()
            .filter(|seen| seen.method == "POST" && seen.body["id"] == id)
            .collect()
    }

    /// Stop accepting connections; later requests fail to connect.
    pub async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn posts(&self, rpc: &str) -> Vec<Seen> {
        self.requests()
            .into_iter()
            .filter(|seen| seen.method == "POST" && seen.rpc() == Some(rpc))
            .collect()
    }

    pub fn gets(&self) -> Vec<Seen> {
        self.requests()
            .into_iter()
            .filter(|seen| seen.method == "GET")
            .collect()
    }

    pub fn deletes(&self) -> Vec<Seen> {
        self.requests()
            .into_iter()
            .filter(|seen| seen.method == "DELETE")
            .collect()
    }
}

impl Drop for McpHttp {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// An unused loopback URL: the port was bound and released.
pub fn unreachable(path_and_query: &str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}{path_and_query}")
}

fn record(
    state: &Shared,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: Value,
    status: u16,
) {
    let headers = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    state.seen.lock().unwrap().push(Seen {
        method: method.to_string(),
        path: path.into(),
        headers,
        body,
        status,
    });
}

async fn handle(
    State(state): State<Shared>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if method == Method::POST && state.behavior.status.is_some() {
        tokio::time::sleep(state.behavior.slow).await;
    }
    if state.behavior.ping_hang && body["method"] == "ping" && body.get("id").is_some() {
        record(&state, &method, "/mcp", &headers, body, 0);
        tokio::time::sleep(Duration::from_secs(600)).await;
        return status(504);
    }
    let response = if let Some(code) = refused(&state, &method, &headers) {
        status(code)
    } else if method == Method::GET
        && let Some(resumed) = resumed(&state, &headers)
    {
        resumed
    } else if method == Method::GET
        && let Some(code) = state.behavior.get_status
    {
        status(code)
    } else if method == Method::GET && state.behavior.legacy {
        legacy_stream(&state)
    } else if method == Method::GET && state.behavior.stream {
        event_stream(&state, None)
    } else {
        respond(&state, &method, &body)
    };
    record(
        &state,
        &method,
        "/mcp",
        &headers,
        body,
        response.status().as_u16(),
    );
    response
}

/// A runtime failure status, or 404 for a request carrying a session that is not live.
fn refused(state: &Shared, method: &Method, headers: &HeaderMap) -> Option<u16> {
    if method == Method::DELETE {
        return None;
    }
    let control = state.control.lock().unwrap();
    if control.fail.is_some() {
        return control.fail;
    }
    if let Some(required) = &control.authorization {
        let sent = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        if sent != Some(required.as_str()) {
            return Some(401);
        }
    }
    let sent = headers.get("mcp-session-id")?.to_str().unwrap_or_default();
    (control.session.as_deref() != Some(sent)).then_some(404)
}

fn respond(state: &Shared, method: &Method, body: &Value) -> Response {
    let behavior = &state.behavior;
    if method == Method::DELETE {
        return status(behavior.delete_status.unwrap_or(200));
    }
    if method != Method::POST {
        return status(405);
    }
    if let Some(code) = behavior.status {
        return status(code);
    }
    if let Some(location) = &behavior.redirect {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header("location", location)
            .body(Body::empty())
            .unwrap();
    }
    // Notifications and client responses are accepted without a body.
    let (Some(id), Some(_)) = (body.get("id"), body.get("method")) else {
        return status(202);
    };
    match body["method"].as_str() {
        Some("initialize") => {
            let mut control = state.control.lock().unwrap();
            if let Some(code) = control.initialize {
                return status(code);
            }
            control.sessions += 1;
            if behavior.sessionless {
                drop(control);
                return json_response(reply(state, body).unwrap(), None);
            }
            let session = match control.sessions {
                1 => SESSION.to_string(),
                count => format!("{SESSION}-{count}"),
            };
            control.session = Some(session.clone());
            drop(control);
            json_response(reply(state, body).unwrap(), Some(&session))
        }
        Some("tools/list") if behavior.oversized => json_response(
            json!({"jsonrpc":"2.0","id":id,"result":{"tools":[],"pad":"x".repeat(1024 * 1024 + 16)}}),
            None,
        ),
        Some("tools/call") if behavior.sse => sse_call(id, body, behavior.delay),
        Some("tools/call") if behavior.poll => {
            state.control.lock().unwrap().pending = Some(call_result(id, body).to_string());
            events("id: call-1\nretry: 100\ndata:\n\n".into())
        }
        Some("tools/call") if behavior.truncate => events(": no result\n\n".into()),
        _ => json_response(reply(state, body).unwrap(), None),
    }
}

/// The JSON-RPC reply to one client request; notifications and responses get none.
fn reply(state: &Shared, body: &Value) -> Option<Value> {
    let id = body.get("id")?;
    body.get("method")?;
    Some(match body["method"].as_str() {
        Some("initialize") => {
            json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"remote-fixture","version":"1"}}})
        }
        Some("tools/list") => {
            let names = state
                .control
                .lock()
                .unwrap()
                .tools
                .clone()
                .unwrap_or_else(|| vec!["echo".into()]);
            let tools: Vec<_> = names
                .iter()
                .map(|name| json!({"name":name,"inputSchema":{"type":"object","properties":{"text":{"type":"string"}}},"annotations":{"readOnlyHint":true}}))
                .collect();
            json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools}})
        }
        Some("ping") => json!({"jsonrpc":"2.0","id":id,"result":{}}),
        Some("tools/call") => call_result(id, body),
        _ => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported"}}),
    })
}

/// The legacy GET stream: an `endpoint` event, then every queued reply as a `message`.
fn legacy_stream(state: &Shared) -> Response {
    let endpoint = state
        .behavior
        .endpoint
        .clone()
        .unwrap_or_else(|| "/messages?sessionId=legacy".into());
    event_stream(
        state,
        Some(format!(": legacy\nevent: endpoint\ndata: {endpoint}\n\n")),
    )
}

/// A GET event stream whose messages come from `push`; `first` defaults to a comment.
fn event_stream(state: &Shared, first: Option<String>) -> Response {
    let (sender, receiver) = futures::channel::mpsc::unbounded::<String>();
    *state.stream.lock().unwrap() = Some(sender);
    let slow = state.behavior.slow;
    let first = futures::stream::once(async move {
        tokio::time::sleep(slow).await;
        first.unwrap_or_else(|| ": stream\n\n".into())
    });
    let messages = futures::StreamExt::map(
        futures::StreamExt::enumerate(receiver),
        |(index, message)| format!("id: ev-{}\nevent: message\ndata: {message}\n\n", index + 1),
    );
    let stream = futures::StreamExt::map(futures::StreamExt::chain(first, messages), |event| {
        Ok::<_, std::convert::Infallible>(Bytes::from(event))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// The polled `tools/call` result for a GET resuming after its priming event.
fn resumed(state: &Shared, headers: &HeaderMap) -> Option<Response> {
    if headers.get("last-event-id")?.to_str().ok()? != "call-1" {
        return None;
    }
    let result = state.control.lock().unwrap().pending.take()?;
    Some(events(format!("id: call-2\ndata: {result}\n\n")))
}

/// A complete `text/event-stream` body.
fn events(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

async fn legacy_message(
    State(state): State<Shared>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let sender = state.stream.lock().unwrap().clone();
    let refusal = refused(&state, &method, &headers).or(state.behavior.message_status);
    let response = match (sender, reply(&state, &body)) {
        _ if refusal.is_some() => status(refusal.unwrap_or_default()),
        (None, _) => status(404),
        (Some(sender), Some(reply)) => {
            let _ = sender.unbounded_send(reply.to_string());
            accepted()
        }
        (Some(_), None) => accepted(),
    };
    record(
        &state,
        &method,
        "/messages",
        &headers,
        body,
        response.status().as_u16(),
    );
    response
}

fn accepted() -> Response {
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .header("content-type", "text/plain")
        .body(Body::from("Accepted"))
        .unwrap()
}

fn call_result(id: &Value, body: &Value) -> Value {
    let text = body["params"]["arguments"]["text"]
        .as_str()
        .unwrap_or_default();
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}})
}

fn status(code: u16) -> Response {
    Response::builder()
        .status(code)
        .body(Body::empty())
        .unwrap()
}

fn json_response(value: Value, session: Option<&str>) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    // Pretty printing proves the client reframes multi-line JSON bodies.
    builder
        .body(Body::from(serde_json::to_vec_pretty(&value).unwrap()))
        .unwrap()
}

fn sse_call(id: &Value, body: &Value, delay: Duration) -> Response {
    let token = body["params"]["_meta"]["progressToken"].clone();
    let progress = json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":token,"progress":1}});
    let events = vec![
        format!(": keepalive\nevent: message\ndata: {progress}\n\n"),
        format!("id: 2\ndata: {}\r\n\r\n", call_result(id, body)),
    ];
    let stream = futures::stream::unfold(events.into_iter(), move |mut events| async move {
        let event = events.next()?;
        tokio::time::sleep(delay).await;
        Some((
            Ok::<_, std::convert::Infallible>(Bytes::from(event)),
            events,
        ))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
