//! The stdio bridge: one JSON-RPC frame per stdin line, each POSTed to the endpoint, and
//! each JSON or SSE answer written to stdout as one line. Requests run concurrently so a
//! waiting call never blocks `ping` or `notifications/cancelled`.
//!
//! The MCP session survives a server restart: a request answered `404` for its MCP
//! session re-sends the cached `initialize` and `notifications/initialized` (writing
//! nothing to stdout) and is retried once; requests arriving meanwhile wait for it.

use std::collections::HashSet;
use std::io::{BufRead as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use cyber_client::StreamingResponse;
use cyber_llm::SseParser;
use futures::StreamExt as _;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use super::link::Link;
use super::{ENDPOINT, Failure, Log, derive_label};
use crate::context::Context;
use crate::error::CliError;

/// How long stdin EOF waits for the `DELETE` of the MCP session.
const DELETE_TIMEOUT: Duration = Duration::from_secs(5);

/// The MCP session the bridge holds for its client, with what re-creating it needs.
#[derive(Default)]
struct McpSession {
    id: Option<String>,
    /// The client's `initialize` and `notifications/initialized` lines, replayed after
    /// the server forgot the MCP session.
    initialize: Option<String>,
    initialized: Option<String>,
}

struct Bridge {
    link: Arc<Link>,
    log: Arc<Log>,
    /// `--client`, else derived from `clientInfo.name` at `initialize`.
    label: StdMutex<Option<String>>,
    /// The negotiated `MCP-Protocol-Version`.
    version: StdMutex<Option<String>>,
    /// `X-Cyber-MCP-Depth` and `X-Cyber-MCP-Caller` from the process environment.
    markers: Vec<(String, String)>,
    session: Mutex<McpSession>,
    /// Set at stdin EOF: the MCP session is being deleted and is never re-created.
    closing: AtomicBool,
    /// Requests the client cancelled: their streams end without a response, and the
    /// bridge sends none either.
    cancelled: StdMutex<HashSet<String>>,
}

/// A server answer: still streaming, or already read.
enum Reply {
    Stream(StreamingResponse),
    Read(u16, Vec<u8>),
}

/// What `initialize` produced.
struct Opened {
    session: Option<String>,
    status: u16,
    bytes: Vec<u8>,
}

pub(super) async fn run(ctx: &Context, client: Option<String>, log: Log) -> Result<(), CliError> {
    let log = Arc::new(log);
    let info = super::super::serve::start(ctx).await?;
    log.info(
        "connected",
        json!({ "url": info.registration.url, "socket": info.registration.socket }),
    );
    let bridge = Arc::new(Bridge {
        link: Link::new(ctx.paths.clone(), &info, Arc::clone(&log)).await,
        log,
        label: StdMutex::new(client),
        version: StdMutex::default(),
        markers: markers(
            std::env::var("CYBER_MCP_DEPTH").ok(),
            std::env::var("CYBER_SESSION_ID").ok(),
        ),
        session: Mutex::default(),
        closing: AtomicBool::new(false),
        cancelled: StdMutex::default(),
    });
    let mut frames = stdin_frames();
    loop {
        tokio::select! {
            frame = frames.recv() => {
                let Some(frame) = frame else {
                    bridge.end().await;
                    return Ok(());
                };
                bridge.accept(frame).await;
            }
            () = bridge.link.gone() => {
                return Err(CliError::runtime("the Cyber server stayed unreachable for 30 s"));
            }
        }
    }
}

/// Lines of stdin, read on a plain thread so a blocked read never holds the runtime.
fn stdin_frames() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            let line = line.trim_end();
            if !line.is_empty() && tx.send(line.to_owned()).is_err() {
                return;
            }
        }
    });
    rx
}

/// Recursion markers forwarded to the server (enforced there).
fn markers(depth: Option<String>, caller: Option<String>) -> Vec<(String, String)> {
    let mut markers = Vec::new();
    if let Some(depth) = depth.and_then(|d| d.trim().parse::<u32>().ok()) {
        markers.push(("x-cyber-mcp-depth".into(), depth.to_string()));
    }
    if let Some(caller) =
        caller.filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_graphic()))
    {
        markers.push(("x-cyber-mcp-caller".into(), caller));
    }
    markers
}

/// Write one frame to stdout as one line.
fn emit(frame: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{frame}");
    let _ = out.flush();
}

/// The id of a JSON-RPC request; notifications and responses have none.
fn request_id(frame: Option<&Value>) -> Option<Value> {
    let frame = frame.filter(|f| f.get("method").is_some())?;
    frame.get("id").filter(|id| !id.is_null()).cloned()
}

fn unknown_session(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|body| body["reason"] == "mcp_session")
}

/// Relay a read answer: JSON-RPC bodies go to stdout, `server-api` errors become a
/// [`Failure`].
fn relay_body(status: u16, bytes: &[u8]) -> Result<(), Failure> {
    let ok = (200..300).contains(&status);
    let body: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    if body.get("jsonrpc").is_some() {
        emit(&body);
        return Ok(());
    }
    if ok {
        return Ok(());
    }
    Err(Failure::from_body(status, &body))
}

async fn read(response: StreamingResponse) -> Result<(u16, Vec<u8>), Failure> {
    let status = response.status;
    let bytes = response.bytes().await.map_err(|_| Failure::unreachable())?;
    Ok((status, bytes))
}

impl Bridge {
    async fn accept(self: &Arc<Self>, line: String) {
        let frame: Option<Value> = serde_json::from_str(&line).ok();
        let initialize = frame
            .as_ref()
            .is_some_and(|f| f["method"] == "initialize" && request_id(Some(f)).is_some());
        if initialize {
            // Nothing else is valid before the client has its `InitializeResult`.
            self.initialize(line, frame.unwrap_or_default()).await;
            return;
        }
        let bridge = Arc::clone(self);
        tokio::spawn(async move { bridge.forward(line, frame).await });
    }

    async fn initialize(&self, line: String, frame: Value) {
        let id = frame["id"].clone();
        let label = {
            let mut label = self.label.lock().unwrap_or_else(PoisonError::into_inner);
            label
                .get_or_insert_with(|| derive_label(frame["params"]["clientInfo"]["name"].as_str()))
                .clone()
        };
        let mut session = self.session.lock().await;
        session.initialized = None;
        session.id = None;
        let result = match self.open(&line).await {
            Ok(opened) => {
                session.id = opened.session.clone();
                self.remember_version(&opened.bytes);
                self.log.info(
                    "initialized",
                    json!({ "mcp_session": opened.session, "client": label }),
                );
                relay_body(opened.status, &opened.bytes)
            }
            Err(failure) => Err(failure),
        };
        session.initialize = Some(line);
        if let Err(failure) = result {
            emit(&failure.frame(&id));
        }
    }

    fn remember_version(&self, bytes: &[u8]) {
        let body: Value = serde_json::from_slice(bytes).unwrap_or_default();
        if let Some(version) = body["result"]["protocolVersion"].as_str() {
            *self.version.lock().unwrap_or_else(PoisonError::into_inner) = Some(version.into());
        }
    }

    async fn forward(&self, line: String, frame: Option<Value>) {
        let id = request_id(frame.as_ref());
        if frame
            .as_ref()
            .is_some_and(|f| f["method"] == "notifications/initialized")
        {
            self.session.lock().await.initialized = Some(line.clone());
        }
        if let Some(cancelled) = frame
            .as_ref()
            .filter(|f| f["method"] == "notifications/cancelled")
        {
            self.cancelled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(cancelled["params"]["requestId"].to_string());
        }
        let result = match self.exchange(line.as_bytes()).await {
            Ok(reply) => self.relay(reply, id.as_ref()).await,
            Err(failure) => Err(failure),
        };
        let Err(failure) = result else {
            if let Some(id) = &id {
                self.was_cancelled(id);
            }
            return;
        };
        let Some(id) = &id else {
            self.log.warn(
                "message not delivered",
                json!({ "error": failure.message, "data": failure.data }),
            );
            return;
        };
        let cancelled = self.was_cancelled(id);
        if let Some(frame) =
            failure_answer(self.closing.load(Ordering::SeqCst), cancelled, id, &failure)
        {
            emit(&frame);
        }
    }

    fn was_cancelled(&self, id: &Value) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id.to_string())
    }

    /// POST a frame on the current MCP session, re-initializing once when the server no
    /// longer knows it.
    async fn exchange(&self, body: &[u8]) -> Result<Reply, Failure> {
        let stale = self.session.lock().await.id.clone();
        let response = self.post(body, stale.as_deref()).await?;
        if response.status != 404 || stale.is_none() {
            return Ok(Reply::Stream(response));
        }
        let (status, bytes) = read(response).await?;
        if !unknown_session(&bytes) {
            return Ok(Reply::Read(status, bytes));
        }
        self.reinitialize(stale).await?;
        let fresh = self.session.lock().await.id.clone();
        Ok(Reply::Stream(self.post(body, fresh.as_deref()).await?))
    }

    /// Start a new MCP session from the cached `initialize`, unless another request
    /// already replaced `stale`. Nothing reaches stdout.
    async fn reinitialize(&self, stale: Option<String>) -> Result<(), Failure> {
        let mut session = self.session.lock().await;
        if session.id != stale {
            return Ok(());
        }
        if self.closing.load(Ordering::SeqCst) {
            return Err(Failure::unreachable());
        }
        let Some(initialize) = session.initialize.clone() else {
            return Err(Failure::from_body(
                404,
                &json!({ "_tag": "NotFoundError", "reason": "mcp_session" }),
            ));
        };
        let opened = self.open(&initialize).await?;
        let Some(fresh) = opened.session.filter(|_| opened.status == 200) else {
            let body = serde_json::from_slice(&opened.bytes).unwrap_or_default();
            return Err(Failure::from_body(opened.status, &body));
        };
        session.id = Some(fresh.clone());
        if let Some(note) = session.initialized.clone() {
            let _ = self.post(note.as_bytes(), Some(&fresh)).await;
        }
        self.log
            .info("reinitialized", json!({ "mcp_session": fresh }));
        Ok(())
    }

    /// POST an `initialize` and read its answer.
    async fn open(&self, line: &str) -> Result<Opened, Failure> {
        let response = self.post(line.as_bytes(), None).await?;
        let session = response.header("mcp-session-id").map(str::to_owned);
        let (status, bytes) = read(response).await?;
        Ok(Opened {
            session,
            status,
            bytes,
        })
    }

    async fn relay(&self, reply: Reply, id: Option<&Value>) -> Result<(), Failure> {
        let (status, bytes) = match reply {
            Reply::Read(status, bytes) => (status, bytes),
            Reply::Stream(response)
                if response
                    .header("content-type")
                    .is_some_and(|t| t.starts_with("text/event-stream")) =>
            {
                return relay_events(response, id, &self.closing).await;
            }
            Reply::Stream(response) => read(response).await?,
        };
        relay_body(status, &bytes)
    }

    async fn post(&self, body: &[u8], session: Option<&str>) -> Result<StreamingResponse, Failure> {
        let (client, epoch) = self.link.client().await.ok_or_else(Failure::unreachable)?;
        let headers = self.headers(session);
        client
            .send_streaming("POST", ENDPOINT, &headers, Some(body.to_vec()))
            .await
            .map_err(|error| {
                self.link.failed(epoch, &error.to_string());
                Failure::unreachable()
            })
    }

    fn headers(&self, session: Option<&str>) -> Vec<(String, String)> {
        let mut headers = vec![
            ("content-type".into(), "application/json".into()),
            (
                "accept".into(),
                "application/json, text/event-stream".into(),
            ),
        ];
        if let Some(session) = session {
            headers.push(("mcp-session-id".into(), session.into()));
            let version = self.version.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(version) = version.as_ref() {
                headers.push(("mcp-protocol-version".into(), version.clone()));
            }
        }
        let label = self.label.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(label) = label.as_ref() {
            headers.push(("x-cyber-mcp-client".into(), label.clone()));
        }
        headers.extend(self.markers.iter().cloned());
        headers
    }

    /// Stdin EOF: end the MCP session (its waits end; Sessions keep running).
    async fn end(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let session = tokio::time::timeout(DELETE_TIMEOUT, self.session.lock())
            .await
            .ok()
            .and_then(|session| session.id.clone());
        let (Some(session), Some(client)) = (session, self.link.current()) else {
            self.log.info("stdin closed", json!({}));
            return;
        };
        let headers = self.headers(Some(&session));
        let deleted = tokio::time::timeout(
            DELETE_TIMEOUT,
            client.send_streaming("DELETE", ENDPOINT, &headers, None),
        )
        .await;
        let status = deleted.ok().and_then(Result::ok).map(|r| r.status);
        self.log.info(
            "stdin closed; MCP session deleted",
            json!({ "deleted": session, "status": status }),
        );
    }
}

/// The server's SSE event carrying an admitted prompt's receipt (not an MCP frame).
const RECEIPT_EVENT: &str = "dev.cyber-code/receipt";

/// The answer to a request whose exchange failed. None after stdin EOF: the bridge's own
/// `DELETE` ended the stream, and requests still in flight get no answer. None also when
/// the client cancelled the request.
fn failure_answer(closing: bool, cancelled: bool, id: &Value, failure: &Failure) -> Option<Value> {
    (!closing && !cancelled).then(|| failure.frame(id))
}

/// Relay every MCP SSE event as one frame; a stream that ends before the response to `id`
/// fails it, with the receipt of a prompt it admitted. Nothing is relayed after stdin EOF.
async fn relay_events(
    response: StreamingResponse,
    id: Option<&Value>,
    closing: &AtomicBool,
) -> Result<(), Failure> {
    let mut parser = SseParser::default();
    let mut body = response.body;
    let mut answered = false;
    let mut receipt = None;
    while let Some(Ok(chunk)) = body.next().await {
        for event in parser.push(&chunk) {
            let Ok(frame) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            if closing.load(Ordering::SeqCst) {
                continue;
            }
            match event.event.as_deref() {
                None | Some("message") => {
                    answered |= id.is_some_and(|id| frame.get("id") == Some(id));
                    emit(&frame);
                }
                Some(RECEIPT_EVENT) => receipt = Some(frame),
                Some(_) => {}
            }
        }
    }
    if answered || id.is_none() {
        Ok(())
    } else {
        Err(Failure::unreachable().with_receipt(receipt.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursion_markers_pass_through_when_well_formed() {
        assert_eq!(
            markers(Some("1".into()), Some("ses_abc".into())),
            vec![
                ("x-cyber-mcp-depth".to_owned(), "1".to_owned()),
                ("x-cyber-mcp-caller".to_owned(), "ses_abc".to_owned()),
            ]
        );
        assert!(markers(Some("deep".into()), Some("bad id".into())).is_empty());
        assert!(markers(None, Some(String::new())).is_empty());
    }

    #[test]
    fn only_requests_have_ids() {
        let request = json!({"jsonrpc": "2.0", "id": 3, "method": "ping"});
        assert_eq!(request_id(Some(&request)), Some(json!(3)));
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert_eq!(request_id(Some(&note)), None);
        let response = json!({"jsonrpc": "2.0", "id": 3, "result": {}});
        assert_eq!(request_id(Some(&response)), None);
        assert_eq!(request_id(None), None);
    }

    #[test]
    fn a_dropped_wait_fails_with_its_receipt() {
        let receipt = json!({"session_id": "ses_1", "message_id": "msg_1", "x": 1});
        let failure = Failure::unreachable().with_receipt(Some(&receipt));
        assert_eq!(
            failure.frame(&json!(4))["error"]["data"],
            json!({
                "_tag": "ServiceUnavailableError",
                "reason": "unreachable",
                "session_id": "ses_1",
                "message_id": "msg_1",
            })
        );
        let plain = Failure::unreachable().with_receipt(None);
        assert!(plain.data.get("session_id").is_none());
    }

    #[test]
    fn requests_in_flight_at_stdin_eof_get_no_answer() {
        // The bridge's own `DELETE` ends the waiting stream; that is not an unreachable
        // server, and the spec gives in-flight requests no answer.
        let failure = Failure::unreachable();
        let id = json!(4);
        assert_eq!(failure_answer(true, false, &id, &failure), None);
        assert_eq!(failure_answer(false, true, &id, &failure), None);
        assert_eq!(
            failure_answer(false, false, &id, &failure),
            Some(failure.frame(&id))
        );
    }

    #[test]
    fn only_the_mcp_session_reason_triggers_a_reinitialize() {
        assert!(unknown_session(
            br#"{"_tag":"NotFoundError","reason":"mcp_session"}"#
        ));
        assert!(!unknown_session(br#"{"_tag":"SessionNotFoundError"}"#));
        assert!(!unknown_session(b"not json"));
    }
}
