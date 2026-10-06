//! Foreground command ownership shared by shell tools.

use std::io;
use std::path::Path;
use std::process::ExitStatus;

use tokio::process::{ChildStderr, ChildStdout, Command};

pub(super) struct Process {
    #[cfg(windows)]
    child: cyber_sandbox::windows_process::OwnedChild,
    #[cfg(not(windows))]
    child: tokio::process::Child,
}

impl Process {
    pub(super) async fn spawn(
        program: &str,
        args: &[String],
        _helper: Option<&Path>,
        configure: impl FnOnce(&mut Command),
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let helper = _helper.ok_or_else(|| {
                io::Error::other("cyber-sandbox-exec.exe is required for Windows process ownership")
            })?;
            let mut command =
                cyber_sandbox::windows_process::OwnedCommand::new(helper, program, args);
            configure(command.command_mut());
            Ok(Self {
                child: command.spawn().await?,
            })
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new(program);
            command.args(args);
            configure(&mut command);
            #[cfg(unix)]
            command.process_group(0);
            Ok(Self {
                child: command.spawn()?,
            })
        }
    }

    fn raw(&mut self) -> &mut tokio::process::Child {
        #[cfg(windows)]
        return self.child.child_mut();
        #[cfg(not(windows))]
        return &mut self.child;
    }

    pub(super) fn stdout(&mut self) -> Option<ChildStdout> {
        self.raw().stdout.take()
    }

    pub(super) fn stderr(&mut self) -> Option<ChildStderr> {
        self.raw().stderr.take()
    }

    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    pub(super) fn terminate(&mut self) {
        #[cfg(windows)]
        self.child.terminate();
        #[cfg(unix)]
        kill_group(self.child.id());
        #[cfg(not(any(unix, windows)))]
        let _ = self.child.start_kill();
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) else {
        return;
    };
    // The process group was created at spawn; the unreaped child retains its identity.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}
