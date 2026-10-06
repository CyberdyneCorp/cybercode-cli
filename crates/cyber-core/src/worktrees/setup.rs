use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;

use super::{GitExecution, Managed, Name, Repository, RepositoryLock, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStream {
    Stdout,
    Stderr,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetupEvent<'a> {
    Started {
        index: usize,
        command: &'a str,
    },
    Output {
        stream: SetupStream,
        bytes: &'a [u8],
    },
    Finished {
        index: usize,
        code: Option<i32>,
    },
}

/// The Session adapter must deliver chunks as they arrive, without collecting output.
pub trait SetupSink: Send + Sync {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()>;
}

pub type SetupFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Option<i32>>> + Send + 'a>>;

/// The runtime supplies sandboxed commands, a worktree-scoped working directory,
/// filtered credentials and cancellation-owned process trees. There is no host launcher.
/// Emit only Output events; return the exit code after streams and process settlement.
/// A signal termination returns None. Sink failures must stop and settle the process.
pub trait SetupExecution: Send + Sync {
    fn run<'a>(
        &'a self,
        directory: &'a Path,
        command: &'a str,
        sink: &'a dyn SetupSink,
    ) -> SetupFuture<'a>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Completed,
    Failed { index: usize, code: Option<i32> },
}

impl Repository {
    /// Run explicitly requested setup using settings resolved after workspace trust.
    /// This is not a retry operation: callers must settle interrupted setup before
    /// rerunning commands, which may have side effects. No failure removes user files.
    pub async fn setup(
        &self,
        git: &dyn GitExecution,
        execution: &dyn SetupExecution,
        managed: &Managed,
        settings: &Settings,
        sink: &dyn SetupSink,
    ) -> io::Result<SetupOutcome> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
        })?;
        let name = Name::parse(&managed.name).map_err(io::Error::other)?;
        let record = self
            .common_dir
            .join("cyber-worktrees")
            .join(format!("{}.json", name.as_str()));
        let owned: Managed =
            serde_json::from_slice(&std::fs::read(record)?).map_err(io::Error::other)?;
        if !owned.ready || owned != *managed || owned.common_dir != self.common_dir {
            return Err(io::Error::other("Setup requires matching ready ownership"));
        }
        if settings
            .setup
            .iter()
            .any(|command| command.trim().is_empty())
        {
            return Err(io::Error::other("Setup commands must not be empty"));
        }
        self.verify(git, managed).await?;
        for (index, command) in settings.setup.iter().enumerate() {
            sink.emit(SetupEvent::Started { index, command })?;
            let code = execution.run(&managed.path, command, sink).await?;
            sink.emit(SetupEvent::Finished { index, code })?;
            if code != Some(0) {
                return Ok(SetupOutcome::Failed { index, code });
            }
        }
        Ok(SetupOutcome::Completed)
    }
}
