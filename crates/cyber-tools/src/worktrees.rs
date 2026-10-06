//! Sandboxed setup execution for a Session already located in an owned worktree.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use cyber_core::worktrees::{
    GitExecution, GitFuture, Managed, Repository, Settings, SetupEvent, SetupExecution,
    SetupFuture, SetupOutcome, SetupSink, SetupStream,
};
use cyber_server::runtime::Invocation;
use cyber_server::worktrees::{CommandDecision, CommandResult, SetupJournal};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::host::{BuiltinHost, Ctx};
use crate::permissions::Request;
use crate::tools::process::Process;

impl BuiltinHost {
    /// The lifecycle owner supplies the Session's streaming sink and settles this
    /// explicit setup attempt durably; interrupted commands must not be blindly retried.
    pub async fn setup_worktree(
        &self,
        inv: &Invocation,
        cancel: CancellationToken,
        repository: &Repository,
        managed: &Managed,
        sink: &dyn SetupSink,
    ) -> io::Result<SetupOutcome> {
        if Path::new(&inv.directory).canonicalize()? != managed.path {
            return Err(io::Error::other(
                "Session is not located in the owned worktree",
            ));
        }
        let ctx = Ctx {
            host: self,
            inv,
            policy: self.policy(inv),
            location: managed.path.clone(),
            cancel,
        };
        let (config, _) = (self.opts.config)(&ctx.location).map_err(io::Error::other)?;
        let settings = Settings::from_config(&config).map_err(io::Error::other)?;
        ctx.authorize(
            Request {
                action: "worktree".into(),
                resources: vec![managed.name.clone()],
                mutates: vec![managed.path.clone()],
                ..Request::default()
            },
            vec![managed.name.clone()],
            serde_json::json!({"operation": "setup", "path": managed.path}),
        )
        .await
        .map_err(tool_error)?;
        let execution = Execution {
            ctx: &ctx,
            journal: SetupJournal::new(
                Arc::clone(&self.opts.store),
                managed,
                &settings.setup,
                &inv.session_id,
            )?,
            next_command: AtomicUsize::new(0),
        };
        execution.journal.validate()?;
        repository
            .setup(&execution, &execution, managed, &settings, sink)
            .await
    }
}

struct Execution<'a> {
    ctx: &'a Ctx<'a>,
    journal: SetupJournal,
    next_command: AtomicUsize,
}

impl SetupExecution for Execution<'_> {
    fn run<'a>(
        &'a self,
        directory: &'a Path,
        command: &'a str,
        sink: &'a dyn SetupSink,
    ) -> SetupFuture<'a> {
        Box::pin(async move {
            let index = self.next_command.fetch_add(1, Ordering::Relaxed);
            if let CommandDecision::Recorded(result) = self.journal.start(index)? {
                return match result {
                    CommandResult::Exited { code } => Ok(code),
                    CommandResult::Failed { message } => Err(io::Error::other(message)),
                };
            }
            let result = self.execute_setup(directory, command, sink).await;
            let saved = match &result {
                Ok(code) => CommandResult::Exited { code: *code },
                Err(error) => CommandResult::Failed {
                    message: error.to_string(),
                },
            };
            self.journal.finish(index, saved)?;
            result
        })
    }
}

impl Execution<'_> {
    async fn execute_setup(
        &self,
        directory: &Path,
        command: &str,
        sink: &dyn SetupSink,
    ) -> io::Result<Option<i32>> {
        #[cfg(not(windows))]
        let prepared = crate::sandboxing::prepare_worktree_command(
            self.ctx,
            &crate::tools::bash::shell(&self.ctx.host.opts.shell),
            &["-c".into(), command.into()],
        )
        .await;
        #[cfg(windows)]
        let prepared = {
            let program = crate::tools::powershell::installed(&self.ctx.location)
                .ok_or_else(|| io::Error::other("PowerShell is required for Windows setup"))?;
            crate::sandboxing::prepare_worktree_command(
                self.ctx,
                &program.display().to_string(),
                &crate::tools::powershell::arguments(command),
            )
            .await
        };
        let status = run(self.ctx, prepared.map_err(tool_error)?, directory, sink).await?;
        Ok(status.code())
    }
}

impl GitExecution for Execution<'_> {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            let mut arguments = vec![
                "-c".into(),
                "core.hooksPath=".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
            ];
            for arg in args {
                arguments.push(
                    arg.to_str()
                        .ok_or_else(|| io::Error::other("Git argument is not Unicode"))?
                        .into(),
                );
            }
            let mut prepared =
                crate::sandboxing::prepare_worktree_command(self.ctx, "git", &arguments)
                    .await
                    .map_err(tool_error)?;
            prepared
                .env
                .retain(|(key, _)| !key.to_ascii_uppercase().starts_with("GIT_"));
            prepared.env.extend([
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
                ),
            ]);
            let capture = Capture::default();
            let status = run(self.ctx, prepared, directory, &capture).await?;
            let (stdout, stderr) = capture
                .0
                .into_inner()
                .map_err(|_| io::Error::other("Git capture poisoned"))?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        })
    }
}

#[derive(Default)]
struct Capture(Mutex<(Vec<u8>, Vec<u8>)>);

impl SetupSink for Capture {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
        if let SetupEvent::Output { stream, bytes } = event {
            let mut capture = self
                .0
                .lock()
                .map_err(|_| io::Error::other("Git capture poisoned"))?;
            if capture.0.len() + capture.1.len() + bytes.len() > 1024 * 1024 {
                return Err(io::Error::other("Git verification output exceeds 1 MiB"));
            }
            match stream {
                SetupStream::Stdout => capture.0.extend_from_slice(bytes),
                SetupStream::Stderr => capture.1.extend_from_slice(bytes),
            }
        }
        Ok(())
    }
}

async fn run(
    ctx: &Ctx<'_>,
    mut prepared: crate::sandboxing::Prepared,
    directory: &Path,
    sink: &dyn SetupSink,
) -> io::Result<ExitStatus> {
    if ctx.cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Setup cancelled",
        ));
    }
    let mut child = Process::spawn(
        &prepared.program,
        &prepared.args,
        ctx.host.opts.sandbox_helper.as_deref(),
        |command| {
            command
                .env_clear()
                .envs(prepared.env.iter().map(|(k, v)| (k, v)))
                .current_dir(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .env("CYBER", "1")
                .env("CYBER_SESSION_ID", &ctx.inv.session_id)
                .env("CYBER_PROJECT_DIR", &ctx.location);
        },
    )
    .await?;
    let stdout = child
        .stdout()
        .ok_or_else(|| io::Error::other("Missing setup stdout"))?;
    let stderr = child
        .stderr()
        .ok_or_else(|| io::Error::other("Missing setup stderr"))?;
    let streams = async {
        tokio::try_join!(
            pump(stdout, SetupStream::Stdout, sink),
            pump(stderr, SetupStream::Stderr, sink)
        )?;
        Ok::<_, io::Error>(())
    };
    let result = {
        let work = async {
            let (status, ()) = tokio::try_join!(child.wait_tree(), streams)?;
            Ok::<_, io::Error>(status)
        };
        tokio::pin!(work);
        let deadline =
            tokio::time::sleep(Duration::from_millis(crate::tools::bash::MAX_TIMEOUT_MS));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                result = &mut work => break result,
                _ = ctx.cancel.cancelled() => break Err(io::Error::new(io::ErrorKind::Interrupted, "Setup cancelled")),
                _ = &mut deadline => break Err(io::Error::new(io::ErrorKind::TimedOut, "Setup timed out")),
                Some((host, reply)) = next_ask(&mut prepared.asks) => { let _ = reply.send(crate::sandboxing::answer(ctx, host).await); }
            }
        }
    };
    if result.is_err() {
        child.terminate();
        let _ = child.wait().await;
    }
    result
}

async fn next_ask(
    asks: &mut Option<tokio::sync::mpsc::Receiver<crate::sandboxing::NetworkAsk>>,
) -> Option<crate::sandboxing::NetworkAsk> {
    match asks {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn pump(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    channel: SetupStream,
    sink: &dyn SetupSink,
) -> io::Result<()> {
    let mut bytes = [0; 8192];
    loop {
        let count = stream.read(&mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        sink.emit(SetupEvent::Output {
            stream: channel,
            bytes: &bytes[..count],
        })?;
    }
}

fn tool_error(error: crate::tools::ToolError) -> io::Error {
    match error {
        crate::tools::ToolError::Failed(message) => io::Error::other(message),
        crate::tools::ToolError::Aborted => {
            io::Error::new(io::ErrorKind::Interrupted, "Setup cancelled")
        }
    }
}
