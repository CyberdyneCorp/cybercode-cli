//! Process lifetime for an already authorized MCP stdio launch.
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::task::JoinHandle;

use super::{McpError, StdioClient};
use crate::HookCommandProcess;
use crate::tools::process::{ReadStream, WriteStream};

const STOP_ACK: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct StderrCapture {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("MCP connection failed (native termination acknowledged: {acknowledged})")]
pub struct ConnectionError {
    #[source]
    pub error: McpError,
    pub acknowledged: bool,
    pub stderr: StderrCapture,
}

/// Launch authorization, sandboxing and durable admission belong to the caller.
/// Dropping this owner terminates the tree but supplies no settlement acknowledgement.
pub struct StdioConnection {
    process: HookCommandProcess,
    client: Option<StdioClient<ReadStream, WriteStream>>,
    stderr: Option<JoinHandle<std::io::Result<StderrCapture>>>,
    #[cfg(unix)]
    grace_deadline: Option<tokio::time::Instant>,
}

impl StdioConnection {
    /// One absolute deadline covers initialization and every initial listing page.
    pub async fn connect_with_tools(
        process: HookCommandProcess,
        name: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
    ) -> Result<(Self, Vec<super::DiscoveredTool>), ConnectionError> {
        Self::connect_with_tools_and_roots(process, name, filter, timeout, None).await
    }

    pub async fn connect_with_tools_and_roots(
        process: HookCommandProcess,
        name: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
        roots: Option<super::McpRoots>,
    ) -> Result<(Self, Vec<super::DiscoveredTool>), ConnectionError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut connection = Self::connect_with_roots(process, timeout, roots).await?;
        match connection
            .discover_tools(
                name,
                filter,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
        {
            Ok(tools) => Ok((connection, tools)),
            Err(error) => {
                let (acknowledged, stderr) = connection.shutdown().await;
                Err(ConnectionError {
                    error,
                    acknowledged,
                    stderr,
                })
            }
        }
    }

    /// Consume an authorized process, retain its streams and complete initialization.
    pub async fn connect(
        process: HookCommandProcess,
        timeout: Duration,
    ) -> Result<Self, ConnectionError> {
        Self::connect_with_roots(process, timeout, None).await
    }

    pub async fn connect_with_roots(
        process: HookCommandProcess,
        timeout: Duration,
        roots: Option<super::McpRoots>,
    ) -> Result<Self, ConnectionError> {
        let mut connection = Self {
            process,
            client: None,
            stderr: None,
            #[cfg(unix)]
            grace_deadline: None,
        };
        if let Err(error) = connection.initialize(timeout, roots).await {
            let (acknowledged, stderr) = connection.shutdown().await;
            return Err(ConnectionError {
                error,
                acknowledged,
                stderr,
            });
        }
        Ok(connection)
    }

    async fn initialize(
        &mut self,
        timeout: Duration,
        roots: Option<super::McpRoots>,
    ) -> Result<(), McpError> {
        let stdin = self
            .process
            .stdin()
            .ok_or(McpError::Protocol("MCP stdin unavailable"))?;
        let stdout = self
            .process
            .stdout()
            .ok_or(McpError::Protocol("MCP stdout unavailable"))?;
        let stderr = self
            .process
            .stderr()
            .ok_or(McpError::Protocol("MCP stderr unavailable"))?;
        self.stderr = Some(tokio::spawn(drain(stderr)));
        let client = StdioClient::new(stdout, stdin);
        self.client = Some(match roots {
            Some(roots) => client.with_roots(roots)?,
            None => client,
        });
        if timeout.is_zero() {
            return Err(McpError::Timeout);
        }
        tokio::time::timeout(timeout, self.client.as_mut().unwrap().initialize(timeout))
            .await
            .map_err(|_| McpError::Timeout)??;
        Ok(())
    }

    pub async fn poll_idle(&mut self, timeout: Duration) -> Result<super::IdleUpdate, McpError> {
        if self.process.leader_exited()? {
            return Ok(super::IdleUpdate {
                closed: true,
                tools_changed: false,
            });
        }
        match self.client.as_mut() {
            Some(client) => client.poll_idle(timeout).await,
            None => Ok(super::IdleUpdate {
                closed: true,
                tools_changed: false,
            }),
        }
    }

    pub fn clear_tools_changed(&mut self) {
        if let Some(client) = self.client.as_mut() {
            client.clear_tools_changed();
        }
    }

    pub async fn disconnected(&mut self) -> Result<bool, McpError> {
        if self.process.leader_exited()? {
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

    pub fn metadata(&self) -> Option<&Value> {
        self.client.as_ref().and_then(StdioClient::metadata)
    }

    pub async fn discover_tools(
        &mut self,
        name: &str,
        filter: &cyber_core::config::McpToolFilter,
        timeout: Duration,
    ) -> Result<Vec<super::DiscoveredTool>, McpError> {
        let client = self
            .client
            .as_mut()
            .ok_or(McpError::Protocol("MCP transport is closed"))?;
        client.discover_tools(name, filter, timeout).await
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.client
            .as_mut()
            .ok_or(McpError::Protocol("MCP transport is closed"))?
            .call_tool(name, arguments, timeout)
            .await
    }

    pub async fn cancel_pending(&mut self, timeout: Duration) -> Result<(), McpError> {
        self.client
            .as_mut()
            .ok_or(McpError::Protocol("MCP transport is closed"))?
            .cancel_pending(timeout)
            .await
    }

    /// Give the retained POSIX process group five seconds after SIGTERM.
    /// Keep the leader unreaped throughout, including when it exits before descendants.
    pub async fn shutdown_gracefully(&mut self) -> (bool, StderrCapture) {
        #[cfg(unix)]
        {
            drop(self.client.take());
            if self.grace_deadline.is_none()
                && self.process.request_graceful_stop().unwrap_or(false)
            {
                self.grace_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(5));
            }
            if let Some(deadline) = self.grace_deadline {
                tokio::time::sleep_until(deadline).await;
            }
        }
        self.shutdown().await
    }

    /// Acknowledge native tree termination separately from remote RPC/effect outcomes.
    /// Capture is transient; callers decide whether and where to persist server logs.
    pub async fn shutdown(&mut self) -> (bool, StderrCapture) {
        drop(self.client.take());
        self.process.terminate();
        let acknowledged = matches!(
            tokio::time::timeout(STOP_ACK, self.process.wait_tree()).await,
            Ok(Ok(_))
        );
        let capture = match self.stderr.as_mut() {
            Some(task) => match tokio::time::timeout(STOP_ACK, &mut *task).await {
                Ok(Ok(Ok(capture))) => capture,
                Err(_) => {
                    task.abort();
                    let _ = (&mut *task).await;
                    StderrCapture {
                        bytes: Vec::new(),
                        truncated: true,
                    }
                }
                Ok(_) => StderrCapture {
                    bytes: Vec::new(),
                    truncated: true,
                },
            },
            None => StderrCapture::default(),
        };
        drop(self.stderr.take());
        (acknowledged, capture)
    }
}

impl Drop for StdioConnection {
    fn drop(&mut self) {
        self.process.terminate();
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
    }
}

async fn drain(mut stderr: ReadStream) -> std::io::Result<StderrCapture> {
    let mut capture = StderrCapture::default();
    let mut buffer = [0; 8192];
    loop {
        let count = stderr.read(&mut buffer).await?;
        if count == 0 {
            return Ok(capture);
        }
        let kept = count.min(super::LIMIT - capture.bytes.len());
        capture.bytes.extend_from_slice(&buffer[..kept]);
        capture.truncated |= kept < count;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::Stdio;

    const SERVER: &str = r#"
import json, sys, subprocess, pathlib
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    else:
        sys.stderr.write('E' * (1024 * 1024 + 100)); sys.stderr.flush()
        args = request['params']['arguments']
        if 'marker' in args:
            subprocess.Popen(['python3','-c','import time, pathlib, sys; time.sleep(0.5); pathlib.Path(sys.argv[1]).write_text("escaped")',args['marker']])
            pathlib.Path(args['marker'] + '.started').write_text('started')
        if request['params']['name'] == 'hang': continue
        result = {'content':[{'type':'text','text':json.dumps(args)}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
"#;

    async fn process(script: &str) -> HookCommandProcess {
        HookCommandProcess::spawn_with_stdin(
            "python3",
            &["-u".into(), "-c".into(), script.into()],
            None,
            |command| {
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            },
        )
        .await
        .unwrap()
    }

    async fn connect() -> StdioConnection {
        StdioConnection::connect(process(SERVER).await, Duration::from_secs(3))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn failed_initialization_returns_native_stop_evidence_and_stderr() {
        let script = r#"
import json, sys, time
request = json.loads(sys.stdin.readline())
sys.stderr.write('fixture initialization rejected'); sys.stderr.flush()
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{}}), flush=True)
time.sleep(30)
"#;
        let result = StdioConnection::connect(process(script).await, Duration::from_secs(3)).await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("invalid initialization accepted"),
        };
        assert!(matches!(error.error, McpError::Protocol(_)));
        assert!(error.acknowledged);
        assert_eq!(error.stderr.bytes, b"fixture initialization rejected");
    }

    #[tokio::test]
    async fn interrupted_graceful_shutdown_retains_the_original_deadline_and_owner() {
        let mut connection = connect().await;
        for _ in 0..2 {
            assert!(
                tokio::time::timeout(Duration::from_millis(100), connection.shutdown_gracefully())
                    .await
                    .is_err()
            );
            assert!(connection.grace_deadline.is_some());
        }
        let deadline = connection.grace_deadline.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), connection.shutdown_gracefully())
                .await
                .is_err()
        );
        assert_eq!(connection.grace_deadline, Some(deadline));
        // Explicit immediate cleanup still consumes native termination proof.
        assert!(connection.shutdown().await.0);
    }

    #[tokio::test]
    async fn interrupted_shutdown_retains_stderr_task_for_owner_disposal() {
        let mut connection = connect().await;
        let task = connection.stderr.take().unwrap();
        task.abort();
        let _ = task.await;
        connection.stderr = Some(tokio::spawn(std::future::pending()));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), connection.shutdown())
                .await
                .is_err()
        );
        let task = connection.stderr.as_ref().expect("drain must remain owned");
        let abort = task.abort_handle();
        assert!(!abort.is_finished());
        drop(connection);
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn actual_stdio_calls_drain_bounded_stderr_and_acknowledge_shutdown() {
        let mut connection = connect().await;
        let result = connection
            .call_tool("echo", json!({"text":"Δ"}), Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            json!({"text":"Δ"})
        );
        assert!(!connection.unresolved());
        let (acknowledged, capture) = connection.shutdown().await;
        assert!(acknowledged);
        assert_eq!(capture.bytes, vec![b'E'; super::super::LIMIT]);
        assert!(capture.truncated);
        assert!(
            connection
                .call_tool("echo", json!({}), Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn timeout_retains_identity_until_native_tree_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("escaped");
        let mut connection = connect().await;
        let error = connection
            .call_tool("hang", json!({"marker":marker}), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(error, McpError::Timeout));
        assert!(marker.with_extension("started").exists());
        assert!(connection.unresolved());
        assert!(connection.shutdown().await.0);
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn disposal_terminates_descendants_without_claiming_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("escaped");
        let mut connection = connect().await;
        connection
            .call_tool("echo", json!({"marker":marker}), Duration::from_secs(3))
            .await
            .unwrap();
        drop(connection);
        assert!(marker.with_extension("started").exists());
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert!(!marker.exists());
    }
}
