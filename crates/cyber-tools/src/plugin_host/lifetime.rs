//! Retain a host's resources until its existing process backend confirms its wait.
//! This does not prove native containment of escaped descendants.
use std::io;
use std::process::ExitStatus;

use cyber_sandbox::proxy::Proxy;
use tokio::sync::oneshot;

use super::launch::Scratch;
use crate::tools::process::{Process, ReadStream, WriteStream};

pub(crate) struct Started {
    held: Option<Held>,
}

struct Resources {
    _scratch: Scratch,
    proxy: Option<Proxy>,
}

struct Held {
    name: String,
    process: Option<Process>,
    resources: Option<Resources>,
    wait_finished: bool,
    wait_failed: bool,
}

impl Started {
    pub(super) fn new(
        process: Process,
        scratch: Scratch,
        proxy: Option<Proxy>,
        name: &str,
    ) -> Self {
        Self {
            held: Some(Held {
                name: name.into(),
                process: Some(process),
                resources: Some(Resources {
                    _scratch: scratch,
                    proxy,
                }),
                wait_finished: false,
                wait_failed: false,
            }),
        }
    }

    pub(super) fn stdin(&mut self) -> Option<WriteStream> {
        self.process().stdin()
    }
    pub(super) fn stdout(&mut self) -> Option<ReadStream> {
        self.process().stdout()
    }
    pub(super) fn stderr(&mut self) -> Option<ReadStream> {
        self.process().stderr()
    }

    fn process(&mut self) -> &mut Process {
        self.held
            .as_mut()
            .expect("owned host")
            .process
            .as_mut()
            .expect("owned process")
    }

    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.held.as_mut().expect("owned host").wait().await
    }

    /// The cleanup task owns resources even if this caller's future is dropped.
    pub(super) async fn finish(mut self) -> io::Result<()> {
        let held = self.held.take().expect("owned host");
        let reply =
            schedule(held).ok_or_else(|| io::Error::other("host cleanup has no runtime"))?;
        reply
            .await
            .map_err(|_| io::Error::other("host cleanup did not finish"))?
    }

    #[cfg(all(test, unix))]
    pub(super) fn scratch(&self) -> &std::path::Path {
        &self
            .held
            .as_ref()
            .expect("owned host")
            .resources
            .as_ref()
            .expect("owned resources")
            ._scratch
            .path
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            let _reply = schedule(held);
        }
    }
}

impl Held {
    fn terminate_if_known(&mut self) {
        if !self.wait_finished && !self.wait_failed {
            self.process.as_mut().expect("owned process").terminate();
        }
    }

    async fn wait(&mut self) -> io::Result<ExitStatus> {
        if self.wait_failed {
            return Err(io::Error::other("host process wait is unconfirmed"));
        }
        let result = self
            .process
            .as_mut()
            .expect("owned process")
            .wait_tree()
            .await;
        self.wait_finished |= result.is_ok();
        self.wait_failed |= result.is_err();
        result
    }

    async fn finish(mut self) -> io::Result<()> {
        if self.wait_failed {
            return Err(io::Error::other("host process wait is unconfirmed"));
        }
        if !self.wait_finished {
            self.wait().await?;
        }
        let proxy = self
            .resources
            .as_mut()
            .and_then(|resources| resources.proxy.take());
        if let Some(proxy) = proxy {
            proxy.shutdown().await?;
        }
        self.resources.take();
        Ok(())
    }
}

fn schedule(mut held: Held) -> Option<oneshot::Receiver<io::Result<()>>> {
    // Revoke execution promptly before the asynchronous waiter is polled. A failed
    // wait cannot authorize signalling a numeric PID whose identity is uncertain.
    held.terminate_if_known();
    if let Some(proxy) = held
        .resources
        .as_mut()
        .and_then(|resources| resources.proxy.as_mut())
    {
        proxy.request_shutdown();
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return None;
    };
    let (reply, received) = oneshot::channel();
    runtime.spawn(async move {
        let result = held.finish().await;
        let _ = reply.send(result);
    });
    Some(received)
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(mut resources) = self.resources.take() {
            // Runtime shutdown, a panic or an unconfirmed wait is not proof that
            // it is safe to remove a namespace a writer might still use.
            // Retain the namespace, not active network authority. Dropping a
            // remaining proxy aborts its owned listener/connection task.
            resources.proxy.take();
            std::mem::forget(resources);
            if self.wait_failed
                && let Some(process) = self.process.take()
            {
                // Process::drop would signal its cached group. After an unknown
                // wait, neither signalling nor releasing that identity is proven.
                std::mem::forget(process);
            }
            cyber_core::log::warn(
                "plugins",
                &format!(
                    "plugin {} resources retained after unconfirmed cleanup",
                    self.name
                ),
                serde_json::json!({"plugin":self.name,"settlement":"unconfirmed"}),
            );
        }
    }
}
