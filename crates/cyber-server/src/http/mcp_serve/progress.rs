//! Waiting calls over SSE (`mcp-server` → Prompt results and waiting): the answer of a
//! waiting `cyber_prompt` or `cyber_prompt_wait` is a `text/event-stream` with a
//! `: keep-alive` comment at least every 15 s, `notifications/progress` while the call
//! carries a `progressToken`, and the JSON-RPC response as its last event.
//!
//! The call ends without a result when it is cancelled (`notifications/cancelled`, the
//! end of the MCP session) or the client closes the stream; only the wait stops, never
//! the Session, its Drain or the prompt.
//!
//! Once the prompt is admitted the stream also carries one `dev.cyber-code/receipt` SSE
//! event `{ session_id, message_id }`, with or without a progress token. MCP clients read
//! only unnamed and `message` events, so it is invisible to them; the stdio bridge uses
//! it to put the receipt into the -32010 error of a wait whose stream drops.

use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::super::jsonrpc;
use super::caller::Caller;
use super::connection::{Active, Connection};
use super::tools;
use crate::runtime::LiveEvent;

/// Comments and keep-alive notifications are sent this often (at most 15 s apart).
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// At most 4 notifications per second.
const MIN_GAP: Duration = Duration::from_millis(250);
const LINE_CHARS: usize = 200;
const PROGRESS: &str = "dev.cyber-code/progress";
const UNTRUSTED: &str = "dev.cyber-code/untrusted";
/// The SSE event name of the admitted prompt's receipt.
pub const RECEIPT_EVENT: &str = "dev.cyber-code/receipt";

/// The progress notifications of one waiting call; a no-op without a progress token or
/// an SSE stream.
pub(super) struct Progress {
    token: Option<Value>,
    tx: Option<mpsc::UnboundedSender<Bytes>>,
    sent: Mutex<Sent>,
}

#[derive(Default)]
struct Sent {
    count: u64,
    last: Option<Instant>,
    message_id: Option<String>,
}

impl Progress {
    pub fn none() -> Self {
        Self {
            token: None,
            tx: None,
            sent: Mutex::default(),
        }
    }

    fn new(token: Option<Value>, tx: mpsc::UnboundedSender<Bytes>) -> Self {
        Self {
            token: token.filter(|t| t.is_string() || t.is_number()),
            tx: Some(tx),
            sent: Mutex::default(),
        }
    }

    /// The prompt was durably admitted: its receipt event, then the first notification,
    /// carrying its id.
    pub fn admitted(&self, session_id: &str, message_id: &str) {
        self.lock().message_id = Some(message_id.to_owned());
        if let Some(tx) = &self.tx {
            let receipt = json!({ "session_id": session_id, "message_id": message_id });
            let _ = tx.send(Bytes::from(format!(
                "event: {RECEIPT_EVENT}\ndata: {receipt}\n\n"
            )));
        }
        self.notify("admitted", "prompt admitted", None);
    }

    /// Re-attaching to an admitted prompt (`cyber_prompt_wait`, a replay).
    pub fn attach(&self, session_id: &str, message_id: &str) {
        self.admitted(session_id, message_id);
    }

    /// A live event of the Session the call waits on.
    pub fn event(&self, event: &LiveEvent) {
        let LiveEvent::Durable { kind, data, .. } = event else {
            return;
        };
        if let Some((kind, line, extra)) = describe(kind, data, self.message_id().as_deref()) {
            self.notify(kind, &line, extra);
        }
    }

    fn keep_alive(&self) {
        self.notify("keep_alive", "still waiting", None);
    }

    fn message_id(&self) -> Option<String> {
        self.lock().message_id.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Sent> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Send one notification unless it would exceed 4 per second; `progress` strictly
    /// increases.
    fn notify(&self, kind: &str, line: &str, extra: Option<(&str, Value)>) {
        let (Some(token), Some(tx)) = (&self.token, &self.tx) else {
            return;
        };
        let mut sent = self.lock();
        let Some(message_id) = sent.message_id.clone() else {
            return;
        };
        let now = Instant::now();
        if sent
            .last
            .is_some_and(|last| now.duration_since(last) < MIN_GAP)
        {
            return;
        }
        sent.count += 1;
        sent.last = Some(now);
        let mut marker = json!({ "v": 1, "kind": kind, "message_id": message_id });
        if let Some((key, value)) = extra {
            marker[key] = value;
        }
        let mut meta = serde_json::Map::new();
        meta.insert(PROGRESS.into(), marker);
        meta.insert(UNTRUSTED.into(), Value::Bool(true));
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": token,
                "progress": sent.count,
                "message": line.chars().take(LINE_CHARS).collect::<String>(),
                "_meta": meta,
            },
        });
        let _ = tx.send(event(&frame));
    }
}

type Described = (&'static str, String, Option<(&'static str, Value)>);

/// The progress kind, status line and extra marker field of a durable event.
fn describe(kind: &str, data: &Value, message_id: Option<&str>) -> Option<Described> {
    let text = |key: &str| data[key].as_str().unwrap_or_default().to_owned();
    match kind {
        "session.prompt.promoted.1" if data["message_id"].as_str() == message_id => {
            Some(("promoted", "prompt promoted into a Turn".into(), None))
        }
        "session.tool.called.1" => Some((
            "tool_started",
            format!("tool {} started", text("name")),
            Some(("tool", data["name"].clone())),
        )),
        "session.tool.settled.1" => Some(("tool_finished", "tool finished".into(), None)),
        "permission.asked.1" | "question.asked.1" => Some((
            "waiting_for_approval",
            "waiting for a human in a Cyber client".into(),
            Some(("request_id", data["id"].clone())),
        )),
        "session.step.ended.1" if data["finish"] == "tool_calls" => None,
        "session.step.ended.1" | "session.step.failed.1" => {
            Some(("turn_finished", "Turn finished".into(), None))
        }
        _ => None,
    }
}

fn event(frame: &Value) -> Bytes {
    Bytes::from(format!("data: {frame}\n\n"))
}

fn comment() -> Bytes {
    Bytes::from_static(b": keep-alive\n\n")
}

/// Answer a waiting `tools/call` as an SSE stream. The call runs on its own task, which
/// holds the MCP session open and stops (ending the wait only) when the call is
/// cancelled or the client goes away.
pub(super) fn respond(active: Active, id: Value, params: Value, caller: Caller) -> Response {
    let (tx, rx) = mpsc::unbounded_channel();
    let connection = Arc::clone(&active.connection);
    tokio::spawn(async move {
        let _active = active;
        run(&connection, id, &params, &caller, tx).await;
    });
    let body = UnboundedReceiverStream::new(rx).map(Ok::<_, Infallible>);
    let mut response = Body::from_stream(body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

async fn run(
    connection: &Arc<Connection>,
    id: Value,
    params: &Value,
    caller: &Caller,
    tx: mpsc::UnboundedSender<Bytes>,
) {
    let tracked = connection.track(&id);
    let progress = Progress::new(params["_meta"].get("progressToken").cloned(), tx.clone());
    let call = tools::call(connection, params, caller, &progress);
    tokio::pin!(call);
    let _ = tx.send(comment());
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + KEEP_ALIVE, KEEP_ALIVE);
    loop {
        tokio::select! {
            reply = &mut call => {
                let frame = match reply {
                    Ok(result) => jsonrpc::result(id, result),
                    Err((code, message)) => jsonrpc::error(id, code, &message, Value::Null),
                };
                let _ = tx.send(event(&frame));
                return;
            }
            () = tracked.token.cancelled() => return,
            () = tx.closed() => return,
            _ = ticks.tick() => {
                let _ = tx.send(comment());
                progress.keep_alive();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The JSON-RPC frames sent, skipping the receipt event.
    fn frames(rx: &mut mpsc::UnboundedReceiver<Bytes>) -> Vec<Value> {
        let mut frames = Vec::new();
        while let Ok(bytes) = rx.try_recv() {
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            if let Some(data) = text.strip_prefix("data: ") {
                frames.push(serde_json::from_str(data.trim()).unwrap());
            }
        }
        frames
    }

    #[test]
    fn admission_sends_the_receipt_event_even_without_a_token() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let progress = Progress::new(None, tx);
        progress.admitted("ses_1", "msg_1");
        let text = String::from_utf8(rx.try_recv().unwrap().to_vec()).unwrap();
        let mut parser = cyber_llm::SseParser::default();
        let events = parser.push(text.as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some(RECEIPT_EVENT));
        let receipt: Value = serde_json::from_str(&events[0].data).unwrap();
        assert_eq!(
            receipt,
            json!({"session_id": "ses_1", "message_id": "msg_1"})
        );
        assert!(rx.try_recv().is_err(), "no progress without a token");
        Progress::none().admitted("ses_1", "msg_1");
    }

    #[test]
    fn notifications_need_a_token_and_an_admitted_message() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let progress = Progress::new(None, tx);
        progress.admitted("ses_1", "msg_1");
        assert!(frames(&mut rx).is_empty());

        let (tx, mut rx) = mpsc::unbounded_channel();
        let progress = Progress::new(Some(json!("t")), tx);
        progress.keep_alive();
        assert!(frames(&mut rx).is_empty(), "nothing before admission");
        progress.admitted("ses_1", "msg_1");
        let sent = frames(&mut rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["params"]["progress"], 1);
        assert_eq!(sent[0]["params"]["_meta"][PROGRESS]["kind"], "admitted");
        assert_eq!(sent[0]["params"]["_meta"][PROGRESS]["message_id"], "msg_1");
        assert_eq!(sent[0]["params"]["_meta"][UNTRUSTED], true);
    }

    #[test]
    fn at_most_four_per_second_strictly_increasing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let progress = Progress::new(Some(json!(7)), tx);
        progress.admitted("ses_1", "msg_1");
        progress.keep_alive();
        assert_eq!(frames(&mut rx).len(), 1, "rate limited");
        std::thread::sleep(MIN_GAP);
        progress.keep_alive();
        let sent = frames(&mut rx);
        assert_eq!(sent[0]["params"]["progress"], 2);
        assert_eq!(sent[0]["params"]["progressToken"], 7);
    }

    #[test]
    fn durable_events_map_to_kinds_and_short_lines() {
        let called = json!({"name": "x".repeat(400), "call_id": "c1"});
        let (kind, line, extra) = describe("session.tool.called.1", &called, None).unwrap();
        assert_eq!(kind, "tool_started");
        assert!(line.len() > LINE_CHARS, "cut when sent");
        assert_eq!(extra.unwrap().0, "tool");
        let promoted = json!({"message_id": "msg_1"});
        assert!(describe("session.prompt.promoted.1", &promoted, Some("msg_2")).is_none());
        assert_eq!(
            describe("session.prompt.promoted.1", &promoted, Some("msg_1"))
                .unwrap()
                .0,
            "promoted"
        );
        let step = json!({"finish": "tool_calls"});
        assert!(describe("session.step.ended.1", &step, None).is_none());
        let asked = json!({"id": "per_1"});
        let (kind, _, extra) = describe("permission.asked.1", &asked, None).unwrap();
        assert_eq!(kind, "waiting_for_approval");
        assert_eq!(extra.unwrap().1, "per_1");
    }
}
