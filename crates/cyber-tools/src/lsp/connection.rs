use std::{path::Path, time::Duration};

use serde_json::Value;
use tokio::{io::AsyncReadExt, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{LspError, StdioClient};
use crate::{
    HookCommandProcess,
    tools::process::{ReadStream, WriteStream},
};

const INITIALIZE: Duration = Duration::from_secs(45);
const GRACE: Duration = Duration::from_secs(3);
const ACK: Duration = Duration::from_secs(2);
type StderrCapture = (Vec<u8>, bool);

#[derive(Debug, Default)]
pub struct Shutdown {
    pub graceful: bool,
    pub acknowledged: bool,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("LSP connection failed (native termination acknowledged: {acknowledged})", acknowledged = .shutdown.acknowledged)]
pub struct ConnectionError {
    #[source]
    pub error: LspError,
    pub shutdown: Shutdown,
    retained: Option<Box<StdioConnection>>,
}

impl ConnectionError {
    /// An unavailable acknowledgement retains the same native owner for retry.
    pub async fn retry_shutdown(&mut self) -> bool {
        if let Some(owner) = self.retained.as_mut() {
            let mut shutdown = owner.settle(Duration::ZERO).await;
            if shutdown.stderr.is_empty() {
                shutdown.stderr = std::mem::take(&mut self.shutdown.stderr);
                shutdown.stderr_truncated |= self.shutdown.stderr_truncated;
            }
            self.shutdown = shutdown;
            if self.shutdown.acknowledged {
                self.retained.take();
            }
        }
        self.shutdown.acknowledged
    }
}

/// Consumes an already-authorized sandbox process. Drop terminates but supplies no acknowledgement.
pub struct StdioConnection {
    process: HookCommandProcess,
    client: Option<StdioClient<ReadStream, WriteStream>>,
    stderr: Option<JoinHandle<std::io::Result<StderrCapture>>>,
}

impl std::fmt::Debug for StdioConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StdioConnection")
            .finish_non_exhaustive()
    }
}

impl StdioConnection {
    pub fn connect(
        process: HookCommandProcess,
        root: &Path,
        options: Value,
    ) -> impl std::future::Future<Output = Result<Self, ConnectionError>> + '_ {
        Self::connect_with_timeout(process, root, options, INITIALIZE)
    }

    pub fn connect_with_timeout(
        process: HookCommandProcess,
        root: &Path,
        options: Value,
        timeout: Duration,
    ) -> impl std::future::Future<Output = Result<Self, ConnectionError>> + '_ {
        Self::connect_with_cancellation(process, root, options, timeout, CancellationToken::new())
    }

    pub fn connect_with_cancellation(
        process: HookCommandProcess,
        root: &Path,
        options: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = Result<Self, ConnectionError>> + '_ {
        // Establish the tree guard before returning a future, including before its first poll.
        let mut owner = Self {
            process,
            client: None,
            stderr: None,
        };
        async move {
            let initialized = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(LspError::Protocol("startup cancelled")),
                result = owner.initialize(root, options, timeout) => result,
            };
            if let Err(error) = initialized {
                let shutdown = owner.settle(Duration::ZERO).await;
                let retained = (!shutdown.acknowledged).then(|| Box::new(owner));
                return Err(ConnectionError {
                    error,
                    shutdown,
                    retained,
                });
            }
            Ok(owner)
        }
    }

    async fn initialize(
        &mut self,
        root: &Path,
        options: Value,
        timeout: Duration,
    ) -> Result<(), LspError> {
        let stdin = self
            .process
            .stdin()
            .ok_or(LspError::Protocol("stdin unavailable"))?;
        let stdout = self
            .process
            .stdout()
            .ok_or(LspError::Protocol("stdout unavailable"))?;
        let stderr = self
            .process
            .stderr()
            .ok_or(LspError::Protocol("stderr unavailable"))?;
        self.stderr = Some(tokio::spawn(drain(stderr)));
        self.client = Some(StdioClient::new(stdout, stdin, root)?);
        self.client
            .as_mut()
            .unwrap()
            .initialize(options, timeout)
            .await
    }

    pub fn capabilities(&self) -> Option<&Value> {
        self.client.as_ref().and_then(StdioClient::capabilities)
    }

    pub(crate) fn leader_exited(&mut self) -> Result<bool, LspError> {
        self.process
            .leader_exited()
            .map_err(|error| LspError::Transport(error.into()))
    }

    pub fn take_notifications(&mut self) -> Vec<Value> {
        self.client
            .as_mut()
            .map(StdioClient::take_notifications)
            .unwrap_or_default()
    }

    pub(crate) async fn next_idle(&mut self) -> Result<Value, LspError> {
        self.client
            .as_mut()
            .ok_or(LspError::Protocol("transport closed"))?
            .next_idle()
            .await
    }

    pub(crate) async fn handle_idle(&mut self, message: Value) -> Result<(), LspError> {
        let result = self
            .client
            .as_mut()
            .ok_or(LspError::Protocol("transport closed"))?
            .handle_idle(message)
            .await;
        if result.is_err() {
            self.process.terminate();
        }
        result
    }

    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let result = self
            .client
            .as_mut()
            .ok_or(LspError::Protocol("transport closed"))?
            .request(method, params, timeout)
            .await;
        if result.is_err() && !matches!(result, Err(LspError::Remote(_))) {
            self.process.terminate();
        }
        result
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        let result = self
            .client
            .as_mut()
            .ok_or(LspError::Protocol("transport closed"))?
            .notify(method, params)
            .await;
        if result.is_err() {
            self.process.terminate();
        }
        result
    }

    /// Try shutdown/exit for at most three seconds, then force and acknowledge the native tree.
    pub async fn shutdown(&mut self) -> Shutdown {
        self.settle(GRACE).await
    }

    async fn settle(&mut self, grace: Duration) -> Shutdown {
        let graceful = if grace.is_zero() {
            false
        } else {
            matches!(
                tokio::time::timeout(grace, async {
                    self.client
                        .as_mut()
                        .ok_or(LspError::Protocol("transport closed"))?
                        .shutdown_protocol(grace)
                        .await?;
                    self.process
                        .wait_tree()
                        .await
                        .map_err(|e| LspError::Transport(e.into()))?;
                    Ok::<_, LspError>(())
                })
                .await,
                Ok(Ok(()))
            )
        };
        let acknowledged = if graceful {
            true
        } else {
            self.process.terminate();
            matches!(
                tokio::time::timeout(ACK, self.process.wait_tree()).await,
                Ok(Ok(_))
            )
        };
        if let Some(client) = self.client.as_mut() {
            client.stop_reader().await;
        }
        drop(self.client.take());
        let (stderr, stderr_truncated) = self.finish_stderr().await;
        Shutdown {
            graceful,
            acknowledged,
            stderr,
            stderr_truncated,
        }
    }

    async fn finish_stderr(&mut self) -> (Vec<u8>, bool) {
        let Some(mut task) = self.stderr.take() else {
            return (Vec::new(), false);
        };
        match tokio::time::timeout(ACK, &mut task).await {
            Ok(Ok(Ok(capture))) => capture,
            Err(_) => {
                task.abort();
                let _ = task.await;
                (Vec::new(), true)
            }
            Ok(_) => (Vec::new(), true),
        }
    }
}

#[cfg(all(test, unix))]
#[path = "connection_tests.rs"]
pub(crate) mod tests;

impl Drop for StdioConnection {
    fn drop(&mut self) {
        self.process.terminate();
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
    }
}

async fn drain(mut stream: ReadStream) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok((bytes, truncated));
        }
        let keep = count.min(1024 * 1024 - bytes.len());
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}
