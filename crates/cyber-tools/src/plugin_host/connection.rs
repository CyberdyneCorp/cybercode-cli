//! JSON-RPC 2.0 over a plugin host's stdio (`plugins-marketplace` → Host protocol): one
//! message per line in each direction. Requests from the host to the core
//! (`secrets/get`, `permission/ask`, `session/prompt`, `log`) go to a [`Handler`]; a host
//! that exits or closes stdout fails every request in flight as crashed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::tools::process::{ReadStream, WriteStream};

/// The longest line accepted from a host: larger messages end the connection.
const MAX_LINE: usize = 8 * 1024 * 1024;

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn to_value(&self) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }

    fn of(value: &Value) -> Self {
        Self {
            code: value["code"].as_i64().unwrap_or(-32603),
            message: value["message"].as_str().unwrap_or("error").to_owned(),
            data: value.get("data").cloned(),
        }
    }
}

/// Why a request to the host did not produce a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CallError {
    /// The host exited or closed its output before answering.
    Crashed,
    Rpc(RpcError),
    Cancelled,
    TimedOut,
}

/// Answers the host's requests and notifications: `(method, params)`.
pub(crate) type Handler =
    Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>;

pub(crate) struct Connection {
    writer: tokio::sync::Mutex<WriteStream>,
    pending: Arc<Pending>,
    next: AtomicU64,
    /// Fires once the host can no longer answer.
    pub(crate) closed: CancellationToken,
}

impl Connection {
    /// Start reading `stdout`; the reader stops when `closed` fires.
    pub(crate) fn start(stdin: WriteStream, stdout: ReadStream, handler: Handler) -> Arc<Self> {
        let connection = Arc::new(Self {
            writer: tokio::sync::Mutex::new(stdin),
            pending: Arc::default(),
            next: AtomicU64::new(1),
            closed: CancellationToken::new(),
        });
        let reader = Arc::downgrade(&connection);
        let closed = connection.closed.clone();
        let pending = Arc::clone(&connection.pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            loop {
                let line = tokio::select! {
                    _ = closed.cancelled() => break,
                    line = read_line(&mut lines) => line,
                };
                let Some(line) = line else { break };
                let Some(connection) = reader.upgrade() else {
                    break;
                };
                connection.dispatch(&line, &handler);
            }
            closed.cancel();
            // Every request still waiting fails as crashed.
            pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        });
        connection
    }

    fn dispatch(self: &Arc<Self>, line: &str, handler: &Handler) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            cyber_core::log::warn("plugins", "plugin host sent invalid JSON", json!({}));
            return;
        };
        if let Some(method) = message["method"].as_str() {
            let method = method.to_owned();
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let id = message.get("id").cloned();
            let handler = Arc::clone(handler);
            let connection = Arc::clone(self);
            tokio::spawn(async move {
                let result = handler(method, params).await;
                if let Some(id) = id {
                    let reply = match result {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error.to_value()}),
                    };
                    let _ = connection.send(&reply).await;
                }
            });
            return;
        }
        let Some(id) = message["id"].as_u64() else {
            return;
        };
        let Some(waiter) = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
        else {
            return;
        };
        let outcome = match message.get("error") {
            Some(error) => Err(RpcError::of(error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = waiter.send(outcome);
    }

    async fn send(&self, message: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(&line).await?;
        writer.flush().await
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), CallError> {
        if self.closed.is_cancelled() {
            return Err(CallError::Crashed);
        }
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
            .map_err(|_| CallError::Crashed)
    }

    /// Send a request and wait for its answer, the host's exit, `cancel` or `timeout`.
    /// A cancelled request is announced with `$/cancelRequest`.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, CallError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        if self.closed.is_cancelled() || self.send(&message).await.is_err() {
            self.forget(id);
            return Err(CallError::Crashed);
        }
        let deadline = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        let outcome = tokio::select! {
            // A reply read just before the host closed its output is delivered before the
            // close; take it rather than report a crash.
            biased;
            reply = receiver => reply.map_err(|_| CallError::Crashed)?.map_err(CallError::Rpc),
            _ = self.closed.cancelled() => Err(CallError::Crashed),
            _ = cancel.cancelled() => {
                let _ = self.notify("$/cancelRequest", json!({"id": id})).await;
                Err(CallError::Cancelled)
            }
            _ = deadline => Err(CallError::TimedOut),
        };
        self.forget(id);
        outcome
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

/// One line without its terminator; `None` at EOF, on a read error or an oversized line.
async fn read_line(reader: &mut BufReader<ReadStream>) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await.ok()?;
        if available.is_empty() {
            return (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned());
        }
        let (used, done) = match available.iter().position(|b| *b == b'\n') {
            Some(end) => {
                bytes.extend_from_slice(&available[..end]);
                (end + 1, true)
            }
            None => {
                bytes.extend_from_slice(available);
                (available.len(), false)
            }
        };
        reader.consume(used);
        if bytes.len() > MAX_LINE {
            return None;
        }
        if done {
            return Some(
                String::from_utf8_lossy(&bytes)
                    .trim_end_matches('\r')
                    .to_owned(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipe() -> (WriteStream, ReadStream) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (Box::new(a), Box::new(b))
    }

    #[tokio::test]
    async fn requests_round_trip_and_host_requests_reach_the_handler() {
        let (core_out, host_in) = pipe();
        let (host_out, core_in) = pipe();
        let handler: Handler = Arc::new(|method, params| {
            Box::pin(async move {
                if method == "secrets/get" {
                    Ok(json!({"value": format!("secret-{}", params["key"].as_str().unwrap())}))
                } else {
                    Err(RpcError::new(-32601, "unknown"))
                }
            })
        });
        let connection = Connection::start(core_out, core_in, handler);
        // A scripted host: answers one request after asking the core for a secret.
        let host = tokio::spawn(async move {
            let mut reader = BufReader::new(host_in);
            let mut writer = host_out;
            let request: Value =
                serde_json::from_str(&read_line(&mut reader).await.unwrap()).unwrap();
            writer
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"h1\",\"method\":\"secrets/get\",\"params\":{\"key\":\"token\"}}\n")
                .await
                .unwrap();
            let answer: Value =
                serde_json::from_str(&read_line(&mut reader).await.unwrap()).unwrap();
            let reply = json!({"jsonrpc":"2.0","id":request["id"],"result":{"echo":request["params"],"secret":answer["result"]["value"]}});
            writer
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
            drop(writer);
        });
        let result = connection
            .request(
                "tool/execute",
                json!({"name":"x"}),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result["echo"], json!({"name":"x"}));
        assert_eq!(result["secret"], "secret-token");
        host.await.unwrap();
        connection.closed.cancelled().await;
        let after = connection
            .request("tool/execute", json!({}), &CancellationToken::new(), None)
            .await;
        assert_eq!(after, Err(CallError::Crashed));
    }

    /// Regression: a reply followed at once by the host closing its output could be
    /// reported as a crash, when the close and the reply were both ready at once.
    #[tokio::test]
    async fn a_reply_just_before_the_host_closes_is_delivered() {
        for _ in 0..20 {
            let (core_out, host_in) = pipe();
            let (host_out, core_in) = pipe();
            let handler: Handler = Arc::new(|_, _| Box::pin(async { Ok(Value::Null) }));
            let connection = Connection::start(core_out, core_in, handler);
            let host = tokio::spawn(async move {
                let mut reader = BufReader::new(host_in);
                let request: Value =
                    serde_json::from_str(&read_line(&mut reader).await.unwrap()).unwrap();
                let mut writer = host_out;
                let reply = json!({"jsonrpc":"2.0","id":request["id"],"result":"done"});
                writer
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            });
            let cancel = CancellationToken::new();
            let call = connection.request("tool/execute", json!({}), &cancel, None);
            tokio::pin!(call);
            // Send the request, then let the reply and the close both land before the
            // call looks again.
            assert!(futures::poll!(&mut call).is_pending());
            host.await.unwrap();
            connection.closed.cancelled().await;
            assert_eq!(call.await, Ok(json!("done")));
        }
    }

    #[tokio::test]
    async fn a_host_that_closes_mid_request_fails_it_as_crashed() {
        let (core_out, host_in) = pipe();
        let (host_out, core_in) = pipe();
        let handler: Handler = Arc::new(|_, _| Box::pin(async { Ok(Value::Null) }));
        let connection = Connection::start(core_out, core_in, handler);
        let host = tokio::spawn(async move {
            let mut reader = BufReader::new(host_in);
            read_line(&mut reader).await.unwrap();
            drop(host_out);
        });
        let outcome = connection
            .request("tool/execute", json!({}), &CancellationToken::new(), None)
            .await;
        assert_eq!(outcome, Err(CallError::Crashed));
        host.await.unwrap();
    }
}
