//! Newline-framed JSON-RPC, initialization and bounded single-request ownership.
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const VERSION: &str = "2025-11-25";
const VERSIONS: &[&str] = &[VERSION, "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP protocol error: {0}")]
    Protocol(&'static str),
    #[error("MCP transport failed")]
    Io(#[source] std::io::Error),
    #[error("MCP request timed out; completion is unverified")]
    Timeout,
    #[error("MCP server rejected request (code {0})")]
    Remote(i64),
    #[error("Stale tool call: {0}")]
    StaleTool(String),
}
impl From<std::io::Error> for McpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

struct Pending {
    id: String,
    method: String,
    sent: bool,
}

/// Own the IO until native stop/settlement; disposing a future does not prove server stop.
pub struct StdioClient<R, W> {
    reader: BufReader<R>,
    writer: W,
    pending: Option<Pending>,
    initialized: bool,
    initialization_started: bool,
    tools: bool,
    protocol_version: Option<String>,
    metadata: Option<Value>,
}
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> StdioClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            pending: None,
            initialized: false,
            initialization_started: false,
            tools: false,
            protocol_version: None,
            metadata: None,
        }
    }
    pub fn unresolved(&self) -> bool {
        self.pending.is_some() || (self.initialization_started && !self.initialized)
    }

    pub fn protocol_version(&self) -> Option<&str> {
        self.protocol_version.as_deref()
    }

    pub fn metadata(&self) -> Option<&Value> {
        self.metadata.as_ref()
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.metadata
            .as_ref()
            .is_some_and(|value| value["capabilities"][capability].is_object())
    }

    /// Signal cancellation without asserting remote effect/transport settlement.
    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        let Some(pending) = &self.pending else {
            return Ok(());
        };
        if pending.method == "initialize" {
            return Err(McpError::Protocol("initialize cannot be cancelled"));
        }
        if !pending.sent {
            return Err(McpError::Protocol(
                "incomplete request framing requires transport close",
            ));
        }
        let params = json!({"requestId":pending.id,"reason":"Client requested cancellation"});
        self.notify("notifications/cancelled", Some(params), timeout)
            .await
    }

    pub async fn initialize(&mut self, timeout: Duration) -> Result<Value, McpError> {
        if self.initialization_started {
            return Err(McpError::Protocol("already initialized"));
        }
        self.initialization_started = true;
        let result = self.request("initialize", json!({"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"cyber","version":env!("CARGO_PKG_VERSION")}}), timeout).await?;
        let version = result["protocolVersion"]
            .as_str()
            .filter(|version| VERSIONS.contains(version))
            .ok_or(McpError::Protocol("unsupported negotiated version"))?;
        if !result["capabilities"].is_object()
            || !result["serverInfo"]["name"].is_string()
            || !result["serverInfo"]["version"].is_string()
            || ["tools", "resources", "prompts"].iter().any(|capability| {
                result["capabilities"]
                    .get(capability)
                    .is_some_and(|value| !value.is_object())
            })
            || result
                .get("instructions")
                .is_some_and(|value| !value.is_string())
        {
            return Err(McpError::Protocol("invalid initialization result"));
        }
        self.notify("notifications/initialized", None, timeout)
            .await?;
        self.protocol_version = Some(version.into());
        self.tools = result["capabilities"]["tools"].is_object();
        self.initialized = true;
        self.metadata = Some(result.clone());
        Ok(result)
    }

    pub async fn list_tools(
        &mut self,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        if !self.initialized || !self.tools {
            return Err(McpError::Protocol("server tools are unavailable"));
        }
        let params = cursor
            .map(|cursor| json!({"cursor":cursor}))
            .unwrap_or_else(|| json!({}));
        self.request("tools/list", params, timeout).await
    }

    pub async fn discover_tools(
        &mut self,
        server: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredTool>, McpError> {
        super::discovery::discover(self, server, filter, timeout).await
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        if !self.initialized || !self.tools {
            return Err(McpError::Protocol("server tools are unavailable"));
        }
        if name.trim().is_empty() || !arguments.is_object() {
            return Err(McpError::Protocol("invalid tool call"));
        }
        self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            timeout,
        )
        .await
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<(), McpError> {
        let mut message = json!({"jsonrpc":"2.0","method":method});
        if let Some(params) = params {
            message["params"] = params;
        }
        tokio::time::timeout(timeout, self.write(&message))
            .await
            .map_err(|_| McpError::Timeout)?
    }

    async fn request(
        &mut self,
        method: &str,
        mut params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        if self.pending.is_some() {
            return Err(McpError::Protocol("previous request is unresolved"));
        }
        let id = cyber_core::ids::new_id("rpc");
        params["_meta"] = json!({"progressToken":id});
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let bytes = encode(&message)?;
        self.pending = Some(Pending {
            id: id.clone(),
            method: method.into(),
            sent: false,
        });
        tokio::time::timeout(timeout, self.send(bytes))
            .await
            .map_err(|_| McpError::Timeout)??;
        if let Some(pending) = &mut self.pending {
            pending.sent = true;
        }
        let mut deadline = tokio::time::Instant::now() + timeout;
        loop {
            let message = tokio::time::timeout_at(deadline, self.read())
                .await
                .map_err(|_| McpError::Timeout)??;
            if message["jsonrpc"] != "2.0" {
                return Err(McpError::Protocol("invalid JSON-RPC version"));
            }
            if let Some(method) = message.get("method") {
                let method = method
                    .as_str()
                    .ok_or(McpError::Protocol("invalid server method"))?;
                if message.get("result").is_some() || message.get("error").is_some() {
                    return Err(McpError::Protocol("invalid server request"));
                }
                if method == "notifications/progress"
                    && message["params"]["progressToken"] == id
                    && message["params"]["progress"].is_number()
                {
                    deadline = tokio::time::Instant::now() + timeout;
                }
                tokio::time::timeout_at(deadline, self.server_message(&message, timeout))
                    .await
                    .map_err(|_| McpError::Timeout)??;
                continue;
            }
            if message["id"] != id {
                return Err(McpError::Protocol("response identity mismatch"));
            }
            let result = response(message)?;
            self.pending = None;
            return result;
        }
    }

    async fn server_message(&mut self, message: &Value, timeout: Duration) -> Result<(), McpError> {
        let Some(id) = message.get("id") else {
            return Ok(());
        };
        if !id.is_string() && !id.is_number() {
            return Err(McpError::Protocol("invalid server request identity"));
        }
        let response = if message["method"] == "ping" {
            json!({"jsonrpc":"2.0","id":id,"result":{}})
        } else {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported by this client"}})
        };
        tokio::time::timeout(timeout, self.write(&response))
            .await
            .map_err(|_| McpError::Timeout)?
    }

    async fn write(&mut self, message: &Value) -> Result<(), McpError> {
        self.send(encode(message)?).await
    }
    async fn send(&mut self, mut bytes: Vec<u8>) -> Result<(), McpError> {
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await?;
        self.writer.flush().await?;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value, McpError> {
        let mut bytes = Vec::new();
        (&mut self.reader)
            .take((super::LIMIT + 2) as u64)
            .read_until(b'\n', &mut bytes)
            .await?;
        if bytes.is_empty() || bytes.last() != Some(&b'\n') || bytes.len() > super::LIMIT + 1 {
            return Err(McpError::Protocol("invalid or oversized message framing"));
        }
        serde_json::from_slice(&bytes).map_err(|_| McpError::Protocol("invalid incoming JSON"))
    }
}

fn response(message: Value) -> Result<Result<Value, McpError>, McpError> {
    match (message.get("result"), message.get("error")) {
        (Some(result), None) => Ok(Ok(result.clone())),
        (None, Some(error)) => {
            let code = error["code"]
                .as_i64()
                .ok_or(McpError::Protocol("invalid JSON-RPC error"))?;
            if !error["message"].is_string() {
                return Err(McpError::Protocol("invalid JSON-RPC error"));
            }
            Ok(Err(McpError::Remote(code)))
        }
        _ => Err(McpError::Protocol("response must contain result or error")),
    }
}

struct Bounded(Vec<u8>);
impl std::io::Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > super::LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("MCP message limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode(message: &Value) -> Result<Vec<u8>, McpError> {
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer(&mut writer, message)
        .map_err(|_| McpError::Protocol("message exceeds 1 MiB"))?;
    Ok(writer.0)
}
