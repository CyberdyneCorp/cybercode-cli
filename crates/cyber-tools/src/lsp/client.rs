use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite, Empty},
    sync::mpsc,
    task::JoinHandle,
};

use super::{Framed, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("LSP protocol error: {0}")]
    Protocol(&'static str),
    #[error("LSP request timed out; completion is unverified")]
    Timeout,
    #[error("LSP server rejected request (code {0})")]
    Remote(i64),
}

/// Single-request protocol ownership. Timeout/cancellation leaves pending ownership fenced.
pub struct StdioClient<R, W> {
    transport: Framed<Empty, W>,
    reader: Option<R>,
    incoming: Option<mpsc::Receiver<Result<Value, LspError>>>,
    read_task: Option<JoinHandle<()>>,
    root_uri: String,
    next_id: u64,
    pending: bool,
    started: bool,
    ready: bool,
    closing: bool,
    notifications: Vec<Value>,
    notification_bytes: usize,
    capabilities: Option<Value>,
}

impl<R: AsyncRead + Unpin + Send + 'static, W: AsyncWrite + Unpin> StdioClient<R, W> {
    pub fn new(reader: R, writer: W, root: &Path) -> Result<Self, LspError> {
        let root = root
            .canonicalize()
            .map_err(|_| LspError::Protocol("invalid root"))?;
        if !root.is_dir() {
            return Err(LspError::Protocol("root is not a directory"));
        }
        let uri = reqwest::Url::from_directory_path(root)
            .map_err(|_| LspError::Protocol("invalid root URI"))?;
        Ok(Self {
            transport: Framed::new(tokio::io::empty(), writer),
            reader: Some(reader),
            incoming: None,
            read_task: None,
            root_uri: uri.into(),
            next_id: 0,
            pending: false,
            started: false,
            ready: false,
            closing: false,
            notifications: Vec::new(),
            notification_bytes: 0,
            capabilities: None,
        })
    }

    pub fn capabilities(&self) -> Option<&Value> {
        self.capabilities.as_ref()
    }

    /// Untrusted server messages; consumers must validate paths and document versions.
    pub fn take_notifications(&mut self) -> Vec<Value> {
        self.notification_bytes = 0;
        std::mem::take(&mut self.notifications)
    }

    pub async fn initialize(&mut self, options: Value, timeout: Duration) -> Result<(), LspError> {
        if self.started {
            return Err(LspError::Protocol("initialization already started"));
        }
        self.started = true;
        let reader = self
            .reader
            .take()
            .ok_or(LspError::Protocol("reader unavailable"))?;
        let (sender, incoming) = mpsc::channel(4);
        self.incoming = Some(incoming);
        self.read_task = Some(tokio::spawn(read_messages(reader, sender)));
        let params = json!({
            "processId":std::process::id(),
            "clientInfo":{"name":"cyber","version":env!("CARGO_PKG_VERSION")},
            "rootUri":self.root_uri,
            "capabilities":{"workspace":{"workspaceFolders":true}},
            "workspaceFolders":[{"uri":self.root_uri,"name":"workspace"}],
            "initializationOptions":options,
        });
        // One deadline includes both the response and initialized notification.
        tokio::time::timeout(timeout, async {
            let result = self.rpc("initialize", params, timeout).await?;
            let capabilities = result
                .get("capabilities")
                .filter(|v| v.is_object())
                .ok_or(LspError::Protocol("invalid initialize result"))?;
            self.transport
                .write(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                .await?;
            self.capabilities = Some(capabilities.clone());
            self.ready = true;
            Ok(())
        })
        .await
        .map_err(|_| LspError::Timeout)?
    }

    fn require_ready(&self) -> Result<(), LspError> {
        if !self.ready || self.closing || self.pending {
            return Err(LspError::Protocol("connection is not available"));
        }
        Ok(())
    }

    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        self.require_ready()?;
        check_method(method)?;
        self.rpc(method, params, timeout).await
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        self.require_ready()?;
        check_method(method)?;
        self.transport.write(&message(method, params)?).await?;
        Ok(())
    }

    /// Cancelling this wait consumes no message and never interrupts framing reads.
    pub async fn next_idle(&mut self) -> Result<Value, LspError> {
        self.require_ready()?;
        let result = self.next_message().await;
        if result.is_err() {
            self.pending = true;
        }
        result
    }

    /// A received idle message remains owned until its handling finishes.
    pub async fn handle_idle(&mut self, message: Value) -> Result<(), LspError> {
        self.require_ready()?;
        let result = if message.get("method").is_none() {
            Err(LspError::Protocol("unexpected idle response"))
        } else {
            self.server_message(message).await
        };
        if result.is_err() {
            self.pending = true;
        }
        result
    }

    pub(crate) async fn stop_reader(&mut self) {
        self.ready = false;
        self.closing = true;
        if let Some(task) = self.read_task.as_mut() {
            task.abort();
            let _ = task.await;
        }
        self.read_task.take();
        self.incoming.take();
    }

    async fn next_message(&mut self) -> Result<Value, LspError> {
        self.incoming
            .as_mut()
            .ok_or(LspError::Protocol("reader unavailable"))?
            .recv()
            .await
            .ok_or(LspError::Protocol("server closed transport"))?
    }

    pub(crate) async fn shutdown_protocol(&mut self, timeout: Duration) -> Result<(), LspError> {
        self.require_ready()?;
        self.closing = true;
        let result = self.rpc("shutdown", Value::Null, timeout).await?;
        if !result.is_null() {
            return Err(LspError::Protocol("invalid shutdown result"));
        }
        self.transport
            .write(&json!({"jsonrpc":"2.0","method":"exit"}))
            .await?;
        self.ready = false;
        Ok(())
    }

    async fn rpc(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        check_params(&params)?;
        if self.pending {
            return Err(LspError::Protocol("request ownership is pending"));
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or(LspError::Protocol("request ID exhausted"))?;
        if id > i32::MAX as u64 {
            return Err(LspError::Protocol("request ID exhausted"));
        }
        self.next_id = id;
        self.pending = true;
        let result = tokio::time::timeout(timeout, self.exchange(id, method, params))
            .await
            .map_err(|_| LspError::Timeout)?;
        if result.is_ok() || matches!(result, Err(LspError::Remote(_))) {
            self.pending = false;
        }
        result
    }

    async fn exchange(&mut self, id: u64, method: &str, params: Value) -> Result<Value, LspError> {
        let mut request = message(method, params)?;
        request["id"] = json!(id);
        self.transport.write(&request).await?;
        loop {
            let message = self.next_message().await?;
            if message.get("method").is_some() {
                self.server_message(message).await?;
            } else {
                return response(message, id);
            }
        }
    }

    async fn server_message(&mut self, message: Value) -> Result<(), LspError> {
        if message.get("result").is_some() || message.get("error").is_some() {
            return Err(LspError::Protocol("invalid server message envelope"));
        }
        check_params(message.get("params").unwrap_or(&Value::Null))?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .ok_or(LspError::Protocol("invalid server method"))?;
        if let Some(id) = message.get("id") {
            if !id.is_string() && !id.is_i64() {
                return Err(LspError::Protocol("invalid server request ID"));
            }
            let reply = match method {
                "window/showMessageRequest" => json!({"jsonrpc":"2.0","id":id,"result":null}),
                "workspace/workspaceFolders" if self.ready => {
                    json!({"jsonrpc":"2.0","id":id,"result":[{"uri":self.root_uri,"name":"workspace"}]})
                }
                _ => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                }
            };
            self.transport.write(&reply).await?;
            return Ok(());
        }
        let bytes = serde_json::to_vec(&message)
            .map_err(|_| LspError::Protocol("invalid notification"))?
            .len();
        if self.notifications.len() >= 128 || bytes > 1024 * 1024 - self.notification_bytes {
            return Err(LspError::Protocol("notification limit exceeded"));
        }
        self.notification_bytes += bytes;
        self.notifications.push(message);
        Ok(())
    }
}

impl<R, W> Drop for StdioClient<R, W> {
    fn drop(&mut self) {
        if let Some(task) = self.read_task.take() {
            task.abort();
        }
    }
}

async fn read_messages<R: AsyncRead + Unpin>(
    reader: R,
    sender: mpsc::Sender<Result<Value, LspError>>,
) {
    let mut frames = Framed::new(reader, tokio::io::sink());
    loop {
        let message = match frames.read().await {
            Ok(Some(message)) => Ok(message),
            Ok(None) => Err(LspError::Protocol("server closed transport")),
            Err(error) => Err(error.into()),
        };
        let closed = message.is_err();
        if sender.send(message).await.is_err() || closed {
            break;
        }
    }
}

fn check_method(method: &str) -> Result<(), LspError> {
    if method.is_empty() || matches!(method, "initialize" | "initialized" | "shutdown" | "exit") {
        return Err(LspError::Protocol("reserved or empty method"));
    }
    Ok(())
}

fn check_params(params: &Value) -> Result<(), LspError> {
    if !params.is_null() && !params.is_object() && !params.is_array() {
        return Err(LspError::Protocol("invalid request parameters"));
    }
    Ok(())
}

fn message(method: &str, params: Value) -> Result<Value, LspError> {
    check_params(&params)?;
    let mut message = json!({"jsonrpc":"2.0","method":method});
    if !params.is_null() {
        message["params"] = params;
    }
    Ok(message)
}

fn response(message: Value, id: u64) -> Result<Value, LspError> {
    if message.get("id").and_then(Value::as_u64) != Some(id) {
        return Err(LspError::Protocol("unexpected response ID"));
    }
    match (message.get("result"), message.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .ok_or(LspError::Protocol("invalid response error"))?;
            if error.get("message").and_then(Value::as_str).is_none() {
                return Err(LspError::Protocol("invalid response error"));
            }
            Err(LspError::Remote(code))
        }
        _ => Err(LspError::Protocol("invalid response envelope")),
    }
}
