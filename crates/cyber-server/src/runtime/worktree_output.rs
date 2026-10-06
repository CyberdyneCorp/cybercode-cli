//! Live setup output attached to an existing Session; results remain in the journal.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use base64::Engine as _;
use cyber_core::worktrees::{Managed, SetupEvent, SetupSink, SetupStream};
use serde::Serialize;

use super::{LiveEvent, Runtime, RuntimeError};

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SetupChannel {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum SetupUpdate {
    /// A command attempt may reuse a journaled result without starting a process.
    Attempted {
        index: usize,
    },
    Output {
        index: usize,
        stream: SetupChannel,
        base64: String,
    },
    Finished {
        index: usize,
        code: Option<i32>,
    },
    Failed {
        index: Option<usize>,
        message: String,
    },
}

pub struct SessionSetupSink {
    runtime: Runtime,
    session_id: String,
    worktree_id: String,
    call_id: String,
    active: Mutex<Option<usize>>,
}

impl Runtime {
    /// Keep shutdown admission open until setup settles or its owning future is disposed.
    pub async fn own_worktree_setup<T>(
        &self,
        cancel: tokio_util::sync::CancellationToken,
        work: impl std::future::Future<Output = io::Result<T>>,
    ) -> io::Result<T> {
        let _admission = self.inner.open().await.map_err(io::Error::other)?;
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            _ = self.shutting_down() => {
                cancel.cancel();
                tokio::time::timeout(std::time::Duration::from_secs(2), &mut work)
                    .await
                    .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::Interrupted, "Runtime closed before setup acknowledged settlement")))
            }
        }
    }

    pub async fn worktree_setup_sink(
        &self,
        session_id: &str,
        call_id: &str,
        managed: &Managed,
    ) -> Result<SessionSetupSink, RuntimeError> {
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(session_id).await?;
        let directory = handle.state.lock().await.info.directory.clone();
        if !managed.ready || managed.id.is_empty() || call_id.is_empty() {
            return Err(RuntimeError::Invalid(
                "Setup output requires ready ownership and a call ID".into(),
            ));
        }
        let location = Path::new(&directory)
            .canonicalize()
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if location != managed.path {
            return Err(RuntimeError::Invalid(
                "Setup output Session is not located in the owned worktree".into(),
            ));
        }
        Ok(SessionSetupSink {
            runtime: self.clone(),
            session_id: session_id.into(),
            worktree_id: managed.id.clone(),
            call_id: call_id.into(),
            active: Mutex::new(None),
        })
    }
}

impl SessionSetupSink {
    pub fn failed(&self, message: &str) -> io::Result<()> {
        let index = self
            .active
            .lock()
            .map_err(|_| io::Error::other("Setup output state poisoned"))?
            .take();
        self.publish(SetupUpdate::Failed {
            index,
            message: message.into(),
        })
    }

    fn publish(&self, update: SetupUpdate) -> io::Result<()> {
        if self.runtime.inner.closed.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Runtime closed during setup output",
            ));
        }
        self.runtime.inner.bus.publish(LiveEvent::WorktreeSetup {
            session_id: self.session_id.clone(),
            worktree_id: self.worktree_id.clone(),
            call_id: self.call_id.clone(),
            update,
        });
        Ok(())
    }
}

impl SetupSink for SessionSetupSink {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| io::Error::other("Setup output state poisoned"))?;
        match event {
            SetupEvent::Started { index, .. } => {
                if active.is_some() {
                    return Err(io::Error::other("Overlapping setup output commands"));
                }
                *active = Some(index);
                self.publish(SetupUpdate::Attempted { index })
            }
            SetupEvent::Finished { index, code } => {
                if *active != Some(index) {
                    return Err(io::Error::other("Setup output command index mismatch"));
                }
                *active = None;
                self.publish(SetupUpdate::Finished { index, code })
            }
            SetupEvent::Output { stream, bytes } => {
                let index =
                    active.ok_or_else(|| io::Error::other("Setup output has no active command"))?;
                let channel = match stream {
                    SetupStream::Stdout => SetupChannel::Stdout,
                    SetupStream::Stderr => SetupChannel::Stderr,
                };
                for chunk in bytes.chunks(8192) {
                    self.publish(SetupUpdate::Output {
                        index,
                        stream: channel.clone(),
                        base64: base64::engine::general_purpose::STANDARD.encode(chunk),
                    })?;
                }
                Ok(())
            }
        }
    }
}
