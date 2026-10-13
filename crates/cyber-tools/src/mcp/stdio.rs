//! Newline-framed JSON-RPC, initialization and bounded single-request ownership.
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub(crate) const VERSION: &str = "2025-11-25";
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
    #[error("MCP remote transport failed: {0}")]
    Http(super::HttpFailure),
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

/// What an idle drain observed; the `*_changed` flags stay set until cleared.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IdleUpdate {
    pub closed: bool,
    pub tools_changed: bool,
    pub prompts_changed: bool,
    pub resources_changed: bool,
}

impl IdleUpdate {
    /// Any `list_changed` notification is pending.
    pub fn changed(&self) -> bool {
        self.tools_changed || self.prompts_changed || self.resources_changed
    }
}

/// A server listing announced by `notifications/<list>/list_changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChange {
    Tools,
    Prompts,
    Resources,
}

enum FrameRead {
    Pending,
    Closed,
    Message(Value),
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
    incoming: Vec<u8>,
    changed: IdleUpdate,
    roots: Option<super::McpRoots>,
    form_server: Option<String>,
    sampling: bool,
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
            incoming: Vec::new(),
            changed: IdleUpdate::default(),
            roots: None,
            form_server: None,
            sampling: false,
        }
    }
    /// Install roots before initialization; the connection snapshot stays immutable.
    pub fn with_roots(mut self, roots: super::McpRoots) -> Result<Self, McpError> {
        if self.initialization_started {
            return Err(McpError::Protocol(
                "roots must be configured before initialization",
            ));
        }
        self.roots = Some(roots);
        Ok(self)
    }

    pub fn with_form_elicitation(mut self, server: &str) -> Result<Self, McpError> {
        if self.initialization_started
            || server.is_empty()
            || server.len() > 48
            || !server
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(McpError::Protocol("invalid elicitation configuration"));
        }
        self.form_server = Some(server.into());
        Ok(self)
    }

    pub fn with_sampling(mut self, enabled: bool) -> Result<Self, McpError> {
        if self.initialization_started {
            return Err(McpError::Protocol(
                "sampling must be configured before initialization",
            ));
        }
        self.sampling = enabled;
        Ok(self)
    }

    /// Observe a ready EOF without consuming buffered protocol data or sending requests.
    pub async fn transport_closed(&mut self) -> Result<bool, McpError> {
        let peek = self.reader.fill_buf();
        tokio::pin!(peek);
        match futures::poll!(peek.as_mut()) {
            std::task::Poll::Ready(Ok(bytes)) => Ok(bytes.is_empty()),
            std::task::Poll::Ready(Err(error)) => Err(error.into()),
            std::task::Poll::Pending => Ok(false),
        }
    }

    /// Drain bounded ready messages while preserving incomplete frames across cancellation.
    pub async fn poll_idle(&mut self, timeout: Duration) -> Result<IdleUpdate, McpError> {
        if self.pending.is_some() {
            return Err(McpError::Protocol("previous request is unresolved"));
        }
        for _ in 0..32 {
            match self.read_frame(false).await? {
                FrameRead::Pending => break,
                FrameRead::Closed => {
                    return Ok(IdleUpdate {
                        closed: true,
                        ..self.changed
                    });
                }
                FrameRead::Message(message) => {
                    validate_server_message(&message)?;
                    self.server_message(&message, timeout, None).await?;
                }
            }
        }
        Ok(self.changed)
    }

    pub fn clear_tools_changed(&mut self) {
        self.clear_changed(ListChange::Tools);
    }

    pub fn clear_changed(&mut self, change: ListChange) {
        match change {
            ListChange::Tools => self.changed.tools_changed = false,
            ListChange::Prompts => self.changed.prompts_changed = false,
            ListChange::Resources => self.changed.resources_changed = false,
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
        let mut capabilities = if self.roots.is_some() {
            json!({"roots":{"listChanged":false}})
        } else {
            json!({})
        };
        if self.form_server.is_some() {
            capabilities["elicitation"] = json!({"form":{}});
        }
        if self.sampling {
            capabilities["sampling"] = json!({});
        }
        let result = self.request("initialize", json!({"protocolVersion":VERSION,"capabilities":capabilities,"clientInfo":{"name":"cyber","version":env!("CARGO_PKG_VERSION")}}), timeout).await?;
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

    /// A client liveness probe; any response, including a JSON-RPC error, is an answer.
    pub async fn ping(&mut self, timeout: Duration) -> Result<Value, McpError> {
        if !self.initialized {
            return Err(McpError::Protocol("server is not initialized"));
        }
        self.request("ping", json!({}), timeout).await
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

    pub async fn list_resources(
        &mut self,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        self.resource(super::ResourceRequest::List(cursor), timeout)
            .await
    }

    pub async fn list_resource_templates(
        &mut self,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        self.resource(super::ResourceRequest::Templates(cursor), timeout)
            .await
    }

    pub async fn read_resource(
        &mut self,
        uri: &str,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        self.resource(super::ResourceRequest::Read(uri), timeout)
            .await
    }

    /// One resource RPC, sent only when the server advertised `resources`; the reply is
    /// validated before it leaves the protocol layer.
    pub async fn resource(
        &mut self,
        request: super::ResourceRequest<'_>,
        timeout: Duration,
    ) -> Result<super::ResourceReply, McpError> {
        if !self.initialized || !self.supports("resources") {
            return Err(McpError::Protocol("server resources are unavailable"));
        }
        let result = self
            .request(request.method(), request.params(), timeout)
            .await?;
        super::resources::validate(request, &result)
    }

    pub async fn discover_tools(
        &mut self,
        server: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredTool>, McpError> {
        super::discovery::discover(self, server, filter, timeout).await
    }

    /// One raw `prompts/list` page, sent only when the server advertised `prompts`.
    pub async fn list_prompts(
        &mut self,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        if !self.initialized || !self.supports("prompts") {
            return Err(McpError::Protocol("server prompts are unavailable"));
        }
        let params = cursor
            .map(|cursor| json!({"cursor":cursor}))
            .unwrap_or_else(|| json!({}));
        self.request("prompts/list", params, timeout).await
    }

    /// Every valid prompt, under one deadline; empty when `prompts` is not advertised.
    pub async fn discover_prompts(
        &mut self,
        server: &str,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredPrompt>, McpError> {
        if !self.initialized {
            return Err(McpError::Protocol("MCP server is not initialized"));
        }
        super::prompts::discover(self, server, timeout).await
    }

    /// `prompts/get` with string arguments; the reply is validated before it is returned.
    pub async fn get_prompt(
        &mut self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
        timeout: Duration,
    ) -> Result<Vec<super::PromptMessage>, McpError> {
        if !self.initialized || !self.supports("prompts") {
            return Err(McpError::Protocol("server prompts are unavailable"));
        }
        if !super::prompts::valid_name(name) || arguments.values().any(|value| !value.is_string()) {
            return Err(McpError::Protocol("invalid prompt request"));
        }
        let params = super::prompts::get_params(name, arguments);
        let result = self.request("prompts/get", params, timeout).await?;
        super::prompts::messages(&result)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.call_tool_with_elicitation(name, arguments, timeout, None)
            .await
    }

    pub async fn call_tool_with_elicitation(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<Value, McpError> {
        if !self.initialized || !self.tools {
            return Err(McpError::Protocol("server tools are unavailable"));
        }
        if name.trim().is_empty() || !arguments.is_object() {
            return Err(McpError::Protocol("invalid tool call"));
        }
        self.request_with_elicitation(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            timeout,
            context,
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
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.request_with_elicitation(method, params, timeout, None)
            .await
    }

    async fn request_with_elicitation(
        &mut self,
        method: &str,
        mut params: Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
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
        let mut progress = None;
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
                    && let Some(value) = advancing_progress(&message, &id, progress)
                {
                    progress = Some(value);
                    deadline = tokio::time::Instant::now() + timeout;
                }
                if ((method == "elicitation/create" && self.form_server.is_some())
                    || (method == "sampling/createMessage" && self.sampling))
                    && message.get("id").is_some()
                {
                    // Human questions and owned nested sampling have their own wait/timeout rules.
                    let started = tokio::time::Instant::now();
                    if self.server_message(&message, timeout, context).await? {
                        deadline += started.elapsed();
                    }
                } else {
                    tokio::time::timeout_at(deadline, self.server_message(&message, timeout, None))
                        .await
                        .map_err(|_| McpError::Timeout)??;
                }
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

    async fn server_message(
        &mut self,
        message: &Value,
        timeout: Duration,
        context: Option<&super::ElicitationContext<'_>>,
    ) -> Result<bool, McpError> {
        let Some(id) = message.get("id") else {
            match message["method"].as_str() {
                Some("notifications/tools/list_changed") => self.changed.tools_changed = true,
                Some("notifications/prompts/list_changed") => self.changed.prompts_changed = true,
                Some("notifications/resources/list_changed") => {
                    self.changed.resources_changed = true;
                }
                _ => {}
            }
            return Ok(false);
        };
        if !id.is_string() && !id.is_number() {
            return Err(McpError::Protocol("invalid server request identity"));
        }
        if message["method"] == "sampling/createMessage" && self.sampling {
            let handler = context.and_then(|context| context.sampling);
            let (mut response, interacted) = match handler {
                Some(handler) => handler.sample(&message["params"]).await,
                None => (
                    json!({"error":{"code":-32603,"message":"Sampling requires an active tool call owner"}}),
                    false,
                ),
            };
            response["jsonrpc"] = json!("2.0");
            response["id"] = id.clone();
            tokio::time::timeout(timeout, self.write(&response))
                .await
                .map_err(|_| McpError::Timeout)??;
            return Ok(interacted);
        }
        if message["method"] == "elicitation/create"
            && let Some(server) = self.form_server.as_deref()
        {
            let (mut response, interacted) =
                super::elicitation::reply(server, &message["params"], context).await;
            response["jsonrpc"] = json!("2.0");
            response["id"] = id.clone();
            tokio::time::timeout(timeout, self.write(&response))
                .await
                .map_err(|_| McpError::Timeout)??;
            return Ok(interacted);
        }
        let response = match (message["method"].as_str(), self.roots.as_ref()) {
            (Some("roots/list"), Some(roots)) if self.initialization_started => {
                if message
                    .get("params")
                    .is_some_and(|params| !params.is_object())
                {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid roots/list parameters"}})
                } else {
                    json!({"jsonrpc":"2.0","id":id,"result":roots.result()})
                }
            }
            (Some("ping"), _) => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported by this client"}})
            }
        };
        tokio::time::timeout(timeout, self.write(&response))
            .await
            .map_err(|_| McpError::Timeout)??;
        Ok(false)
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
        match self.read_frame(true).await? {
            FrameRead::Message(message) => Ok(message),
            _ => Err(McpError::Protocol("invalid or oversized message framing")),
        }
    }

    async fn read_frame(&mut self, wait: bool) -> Result<FrameRead, McpError> {
        loop {
            let bytes = if wait {
                self.reader.fill_buf().await?
            } else {
                let peek = self.reader.fill_buf();
                tokio::pin!(peek);
                match futures::poll!(peek.as_mut()) {
                    std::task::Poll::Ready(result) => result?,
                    std::task::Poll::Pending => return Ok(FrameRead::Pending),
                }
            };
            if bytes.is_empty() {
                return if self.incoming.is_empty() {
                    Ok(FrameRead::Closed)
                } else {
                    Err(McpError::Protocol("invalid or oversized message framing"))
                };
            }
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let length = newline.map_or(bytes.len(), |position| position + 1);
            if self.incoming.len() + length > super::LIMIT + usize::from(newline.is_some()) {
                return Err(McpError::Protocol("invalid or oversized message framing"));
            }
            self.incoming.extend_from_slice(&bytes[..length]);
            self.reader.consume(length);
            if newline.is_some() {
                let bytes = std::mem::take(&mut self.incoming);
                return serde_json::from_slice(&bytes)
                    .map(FrameRead::Message)
                    .map_err(|_| McpError::Protocol("invalid incoming JSON"));
            }
        }
    }
}

fn advancing_progress(message: &Value, token: &str, previous: Option<f64>) -> Option<f64> {
    if message.get("id").is_some() || message["params"]["progressToken"] != token {
        return None;
    }
    let params = message.get("params")?.as_object()?;
    let progress = params
        .get("progress")?
        .as_f64()
        .filter(|value| value.is_finite())?;
    if previous.is_some_and(|previous| progress <= previous)
        || params
            .get("total")
            .is_some_and(|total| total.as_f64().is_none_or(|value| !value.is_finite()))
        || params
            .get("message")
            .is_some_and(|message| !message.is_string())
    {
        return None;
    }
    Some(progress)
}

fn validate_server_message(message: &Value) -> Result<(), McpError> {
    if message["jsonrpc"] != "2.0"
        || !message["method"].is_string()
        || message.get("result").is_some()
        || message.get("error").is_some()
    {
        return Err(McpError::Protocol("invalid idle server message"));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn passive_probe_preserves_buffered_frames_and_observes_eof() {
        let (reader, mut peer) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, tokio::io::sink());
        assert!(!client.transport_closed().await.unwrap());
        peer.write_all(b"{\"jsonrpc\":").await.unwrap();
        assert!(!client.transport_closed().await.unwrap());
        peer.write_all(b"\"2.0\",\"method\":\"notice\"}\n")
            .await
            .unwrap();
        drop(peer);
        assert!(!client.transport_closed().await.unwrap());
        assert_eq!(
            client.read().await.unwrap(),
            json!({"jsonrpc":"2.0","method":"notice"})
        );
        assert!(client.transport_closed().await.unwrap());
    }
    #[tokio::test]
    async fn idle_pump_retains_partial_frames_and_drains_notifications_before_eof() {
        let (reader, mut peer) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, tokio::io::sink());
        peer.write_all(b"{\"jsonrpc\":").await.unwrap();
        assert!(
            !client
                .poll_idle(Duration::from_secs(1))
                .await
                .unwrap()
                .closed
        );
        peer.write_all(b"\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n")
            .await
            .unwrap();
        drop(peer);
        let update = client.poll_idle(Duration::from_secs(1)).await.unwrap();
        assert!(update.closed && update.tools_changed);
        client.clear_tools_changed();
        assert!(
            !client
                .poll_idle(Duration::from_secs(1))
                .await
                .unwrap()
                .tools_changed
        );
    }

    #[tokio::test]
    async fn idle_pump_limits_each_batch_and_answers_server_ping() {
        let (reader, mut peer) = tokio::io::duplex(8192);
        let (writer, output) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, writer);
        for _ in 0..32 {
            peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notice\"}\n")
                .await
                .unwrap();
        }
        peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":7}\n")
            .await
            .unwrap();
        drop(peer);
        assert!(
            !client
                .poll_idle(Duration::from_secs(1))
                .await
                .unwrap()
                .closed
        );
        assert!(
            client
                .poll_idle(Duration::from_secs(1))
                .await
                .unwrap()
                .closed
        );
        let mut line = String::new();
        BufReader::new(output).read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"jsonrpc":"2.0","id":7,"result":{}})
        );
    }

    #[tokio::test]
    async fn idle_pump_refuses_unsolicited_responses_and_incomplete_eof() {
        for bytes in [
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n".as_slice(),
            b"{\"jsonrpc\":".as_slice(),
        ] {
            let (reader, mut peer) = tokio::io::duplex(1024);
            peer.write_all(bytes).await.unwrap();
            drop(peer);
            let mut client = StdioClient::new(reader, tokio::io::sink());
            assert!(matches!(
                client.poll_idle(Duration::from_secs(1)).await,
                Err(McpError::Protocol(_))
            ));
        }
    }
    #[tokio::test]
    async fn cancelled_frame_read_preserves_consumed_prefix() {
        let (reader, mut peer) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, tokio::io::sink());
        peer.write_all(b"{\"jsonrpc\":").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), client.read())
                .await
                .is_err()
        );
        assert!(!client.incoming.is_empty());
        peer.write_all(b"\"2.0\",\"method\":\"notice\"}\n")
            .await
            .unwrap();
        assert_eq!(
            client.read().await.unwrap(),
            json!({"jsonrpc":"2.0","method":"notice"})
        );
    }

    fn initialized<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        client: &mut StdioClient<R, W>,
        capabilities: Value,
    ) {
        client.initialization_started = true;
        client.initialized = true;
        client.metadata = Some(json!({"capabilities":capabilities}));
    }

    #[tokio::test]
    async fn resource_requests_require_the_advertised_capability() {
        let (reader, _peer) = tokio::io::duplex(1024);
        let (writer, mut output) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, writer);
        initialized(&mut client, json!({"tools":{}}));
        let timeout = Duration::from_secs(1);
        assert!(matches!(
            client.list_resources(None, timeout).await,
            Err(McpError::Protocol(_))
        ));
        assert!(matches!(
            client.list_resource_templates(Some("c"), timeout).await,
            Err(McpError::Protocol(_))
        ));
        assert!(matches!(
            client.read_resource("file:///x", timeout).await,
            Err(McpError::Protocol(_))
        ));
        assert!(!client.unresolved());
        drop(client);
        let mut written = Vec::new();
        output.read_to_end(&mut written).await.unwrap();
        assert!(written.is_empty(), "{}", String::from_utf8_lossy(&written));
    }

    /// Answer the next request on `peer` with `result`, returning the request.
    async fn answer(
        requests: &mut BufReader<tokio::io::DuplexStream>,
        peer: &mut tokio::io::DuplexStream,
        result: Value,
    ) -> Value {
        let mut line = String::new();
        requests.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let reply = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
        peer.write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
        request
    }

    #[tokio::test]
    async fn read_resource_validates_contents_and_keeps_the_connection() {
        let (reader, mut peer) = tokio::io::duplex(64 * 1024);
        let (writer, output) = tokio::io::duplex(64 * 1024);
        let mut requests = BufReader::new(output);
        let mut client = StdioClient::new(reader, writer);
        initialized(&mut client, json!({"resources":{}}));
        let timeout = Duration::from_secs(5);
        for contents in [
            json!([{"text":"missing uri"}]),
            json!([{"uri":"file:///x","text":"a","blob":"YQ=="}]),
            json!([{"uri":"file:///x","blob":"%%%"}]),
        ] {
            let (result, request) = tokio::join!(
                client.read_resource("file:///x", timeout),
                answer(&mut requests, &mut peer, json!({"contents":contents}))
            );
            assert!(matches!(result, Err(McpError::Protocol(_))), "{contents}");
            assert_eq!(request["method"], "resources/read");
            assert_eq!(request["params"]["uri"], "file:///x");
            assert!(!client.unresolved());
        }
        let (result, request) = tokio::join!(
            client.list_resources(Some("next"), timeout),
            answer(&mut requests, &mut peer, json!({"resources":[]}))
        );
        assert_eq!(request["params"]["cursor"], "next");
        assert_eq!(
            result.unwrap(),
            super::super::ResourceReply::Listing(json!({"resources":[]}))
        );
    }

    #[tokio::test]
    async fn list_changed_notifications_set_their_own_flags() {
        let (reader, mut peer) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, tokio::io::sink());
        peer.write_all(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/prompts/list_changed\"}\n",
        )
        .await
        .unwrap();
        let update = client.poll_idle(Duration::from_secs(1)).await.unwrap();
        assert!(update.prompts_changed && !update.tools_changed && !update.resources_changed);
        assert!(update.changed());
        peer.write_all(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/resources/list_changed\"}\n",
        )
        .await
        .unwrap();
        let update = client.poll_idle(Duration::from_secs(1)).await.unwrap();
        assert!(update.prompts_changed && update.resources_changed);
        client.clear_changed(ListChange::Prompts);
        client.clear_changed(ListChange::Resources);
        let update = client.poll_idle(Duration::from_secs(1)).await.unwrap();
        assert!(!update.changed(), "{update:?}");
    }

    #[tokio::test]
    async fn prompt_requests_require_the_advertised_capability() {
        let (reader, _peer) = tokio::io::duplex(1024);
        let (writer, mut output) = tokio::io::duplex(1024);
        let mut client = StdioClient::new(reader, writer);
        initialized(&mut client, json!({"tools":{},"resources":{}}));
        let timeout = Duration::from_secs(1);
        assert!(matches!(
            client.list_prompts(None, timeout).await,
            Err(McpError::Protocol(_))
        ));
        assert!(matches!(
            client
                .get_prompt("review_pr", &serde_json::Map::new(), timeout)
                .await,
            Err(McpError::Protocol(_))
        ));
        assert_eq!(
            client.discover_prompts("srv", timeout).await.unwrap(),
            Vec::new()
        );
        assert!(!client.unresolved());
        drop(client);
        let mut written = Vec::new();
        output.read_to_end(&mut written).await.unwrap();
        assert!(written.is_empty(), "{}", String::from_utf8_lossy(&written));
    }

    #[tokio::test]
    async fn get_prompt_sends_string_arguments_and_validates_messages() {
        let (reader, mut peer) = tokio::io::duplex(64 * 1024);
        let (writer, output) = tokio::io::duplex(64 * 1024);
        let mut requests = BufReader::new(output);
        let mut client = StdioClient::new(reader, writer);
        initialized(&mut client, json!({"prompts":{}}));
        let timeout = Duration::from_secs(5);
        let mut arguments = serde_json::Map::new();
        arguments.insert("number".into(), json!("42"));
        let (result, request) = tokio::join!(
            client.get_prompt("review_pr", &arguments, timeout),
            answer(
                &mut requests,
                &mut peer,
                json!({"messages":[{"role":"user","content":{"type":"text","text":"Review 42"}}]})
            )
        );
        assert_eq!(request["method"], "prompts/get");
        assert_eq!(request["params"]["name"], "review_pr");
        assert_eq!(request["params"]["arguments"], json!({"number":"42"}));
        assert_eq!(
            result.unwrap(),
            [super::super::PromptMessage {
                role: super::super::PromptRole::User,
                content: super::super::PromptContent::Text("Review 42".into()),
            }]
        );
        let (result, _) = tokio::join!(
            client.get_prompt("review_pr", &arguments, timeout),
            answer(
                &mut requests,
                &mut peer,
                json!({"messages":[{"role":"system"}]})
            )
        );
        assert!(matches!(result, Err(McpError::Protocol(_))));
        assert!(!client.unresolved());
        arguments.insert("n".into(), json!(1));
        assert!(
            client
                .get_prompt("review_pr", &arguments, timeout)
                .await
                .is_err()
        );
        assert!(
            client
                .get_prompt("a b", &serde_json::Map::new(), timeout)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn idle_pump_bounds_partial_frame_before_allocating_past_limit() {
        let reader = std::io::Cursor::new(vec![b'x'; super::super::LIMIT + 1]);
        let mut client = StdioClient::new(reader, tokio::io::sink());
        assert!(matches!(
            client.poll_idle(Duration::from_secs(1)).await,
            Err(McpError::Protocol(_))
        ));
        assert!(client.incoming.len() <= super::super::LIMIT);
    }
}
