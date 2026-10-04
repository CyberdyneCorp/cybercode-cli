//! JSON-RPC 2.0 over WebSocket and stdio (`server-api` → WebSocket transport, Stdio
//! transport). Each method is an OpenAPI operation ID; calls run through the same router.
//!
//! Request params: `{ path?: {sessionID, ...}, query?: {...}, body?: ..., idempotencyKey? }`.
//! `v1.event.subscribe` and `v1.session.events` start a subscription and stream
//! `{"method": "event", "params": {"subscription": <id>, "event": <EventEnvelope>}}`
//! notifications; `v1.rpc.unsubscribe` `{ subscription }` stops one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use cyber_llm::SseParser;
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::envelope::percent_decode;
use super::remote_tools::RemoteTools;
use super::{AppState, EmbeddedClient};

/// One JSON-RPC session: requests in, responses and notifications out.
pub struct RpcSession {
    client: EmbeddedClient,
    out: mpsc::Sender<Value>,
    subscriptions: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    tools: Arc<RemoteTools>,
    owner: u64,
    /// Built-in tool names, which clients may not shadow.
    reserved: Vec<String>,
}

impl Drop for RpcSession {
    fn drop(&mut self) {
        self.tools.release(self.owner);
        for (_, task) in self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
        {
            task.abort();
        }
    }
}

impl RpcSession {
    pub fn new(state: &AppState, out: mpsc::Sender<Value>) -> Self {
        let tools = Arc::clone(&state.remote_tools);
        let turn = crate::runtime::TurnContext {
            session_id: String::new(),
            directory: state.options.default_directory.display().to_string(),
            agent: "build".into(),
            mode: "bypass".into(),
            prefers_apply_patch: false,
            rules: Value::Null,
        };
        // Built-in names only: client tools may be re-registered by their owner.
        let remote: Vec<String> = tools
            .definitions()
            .into_iter()
            .map(|d| d.spec.name)
            .collect();
        let reserved = state
            .services
            .tools(&turn)
            .into_iter()
            .map(|d| d.spec.name)
            .filter(|n| !remote.contains(n))
            .collect();
        Self {
            client: EmbeddedClient::new(super::router(state.clone())),
            out,
            subscriptions: Arc::default(),
            owner: tools.owner(),
            tools,
            reserved,
        }
    }

    /// Handle one incoming frame; the reply (if any) goes to `out`.
    pub async fn handle(&self, text: &str) {
        let reply = match serde_json::from_str::<Value>(text) {
            Ok(req) => self.dispatch(req).await,
            Err(e) => Some(error(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
                Value::Null,
            )),
        };
        if let Some(reply) = reply {
            let _ = self.out.send(reply).await;
        }
    }

    async fn dispatch(&self, req: Value) -> Option<Value> {
        if req.get("method").is_none() {
            // A response to a server-initiated request (`tool.execute`).
            self.tools.resolve(&req);
            return None;
        }
        let id = req.get("id").cloned();
        let method = req["method"].as_str().unwrap_or_default().to_string();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let result = match method.as_str() {
            "v1.event.subscribe" | "v1.session.events" => self.subscribe(&method, &params).await,
            "v1.rpc.unsubscribe" => Ok(self.unsubscribe(&params)),
            "v1.tool.register" => self
                .tools
                .register(self.owner, self.out.clone(), &params, &self.reserved)
                .map_err(|m| (-32602, m, Value::Null)),
            "v1.tool.unregister" => Ok(self.unregister(&params)),
            _ => self.call(&method, &params).await,
        };
        let id = id?;
        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err((code, message, data)) => error(id, code, &message, data),
        })
    }

    async fn call(&self, method: &str, params: &Value) -> Result<Value, (i64, String, Value)> {
        let req = request(method, params, "GET")?;
        let res = self.client.request(req).await;
        let status = res.status().as_u16();
        let bytes = res
            .into_body()
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            let tag = body["_tag"].as_str().unwrap_or("UnknownError").to_string();
            Err((
                -32000 - i64::from(status),
                format!("{tag}: {}", body["message"].as_str().unwrap_or_default()),
                body,
            ))
        }
    }

    async fn subscribe(&self, method: &str, params: &Value) -> Result<Value, (i64, String, Value)> {
        let req = request(method, params, "GET")?;
        let res = self.client.request(req).await;
        if !res.status().is_success() {
            let bytes = res
                .into_body()
                .collect()
                .await
                .map(|b| b.to_bytes())
                .unwrap_or_default();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err((
                -32000,
                body["message"]
                    .as_str()
                    .unwrap_or("subscription refused")
                    .to_string(),
                body,
            ));
        }
        let subscription = cyber_core::ids::new_id("sub");
        let (out, sub) = (self.out.clone(), subscription.clone());
        let task = tokio::spawn(async move {
            let mut parser = SseParser::default();
            let mut body = res.into_body().into_data_stream();
            while let Some(Ok(chunk)) = body.next().await {
                for event in parser.push(&chunk) {
                    let Ok(envelope) = serde_json::from_str::<Value>(&event.data) else {
                        continue;
                    };
                    let note = json!({ "jsonrpc": "2.0", "method": "event", "params": { "subscription": sub, "event": envelope } });
                    if out.send(note).await.is_err() {
                        return;
                    }
                }
            }
        });
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(subscription.clone(), task);
        Ok(json!({ "subscription": subscription }))
    }

    fn unregister(&self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let owned = self.tools.definitions().iter().any(|d| d.spec.name == name);
        if owned {
            self.tools.release_one(self.owner, name);
        }
        json!({ "unregistered": owned })
    }

    fn unsubscribe(&self, params: &Value) -> Value {
        let id = params["subscription"].as_str().unwrap_or_default();
        let task = self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        if let Some(task) = &task {
            task.abort();
        }
        json!({ "unsubscribed": task.is_some() })
    }
}

fn error(id: Value, code: i64, message: &str, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message, "data": data } })
}

/// The HTTP request for an operation ID and its params.
fn request(
    method: &str,
    params: &Value,
    _default: &str,
) -> Result<axum::http::Request<Body>, (i64, String, Value)> {
    let Some((verb, template)) = super::openapi::lookup(method) else {
        return Err((-32601, format!("unknown method {method}"), Value::Null));
    };
    let mut path = template.to_string();
    for (name, value) in params["path"].as_object().into_iter().flatten() {
        path = path.replace(
            &format!("{{{name}}}"),
            &encode(value.as_str().unwrap_or_default()),
        );
    }
    if path.contains('{') {
        return Err((
            -32602,
            format!("missing path parameter in {template}"),
            Value::Null,
        ));
    }
    let query = query_string(params["query"].as_object());
    let mut req = axum::http::Request::builder()
        .method(verb.to_uppercase().as_str())
        .uri(format!("http://cyber.internal/api/v1{path}{query}"));
    if let Some(dir) = params["directory"].as_str() {
        req = req.header("x-cyber-directory", encode(dir));
    }
    if let Some(key) = params["idempotencyKey"].as_str() {
        req = req.header("idempotency-key", key);
    }
    let body = match params.get("body") {
        Some(b) if !b.is_null() => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        _ => Body::empty(),
    };
    req.body(body)
        .map_err(|e| (-32602, e.to_string(), Value::Null))
}

fn query_string(q: Option<&Map<String, Value>>) -> String {
    let pairs: Vec<String> = q
        .into_iter()
        .flatten()
        .map(|(k, v)| {
            format!(
                "{}={}",
                encode(k),
                encode(&v.as_str().map_or_else(|| v.to_string(), str::to_string))
            )
        })
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

fn encode(s: &str) -> String {
    let decoded = percent_decode(s);
    decoded
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `GET /api/v1/ws`: the WebSocket transport.
pub async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve_socket(state, socket))
}

async fn serve_socket(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Value>(256);
    let session = Arc::new(RpcSession::new(&state, tx));
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.handle(&text).await });
    }
    writer.abort();
}
