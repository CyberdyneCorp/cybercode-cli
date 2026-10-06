//! Foreground command ownership shared by shell tools.

use std::io;
use std::path::Path;
use std::process::ExitStatus;

use tokio::process::{ChildStderr, ChildStdout, Command};

pub(crate) struct Process {
    #[cfg(windows)]
    child: cyber_sandbox::windows_process::OwnedChild,
    #[cfg(not(windows))]
    child: tokio::process::Child,
}

impl Process {
    pub(crate) async fn spawn(
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

    pub(crate) fn stdout(&mut self) -> Option<ChildStdout> {
        self.raw().stdout.take()
    }

    pub(crate) fn stderr(&mut self) -> Option<ChildStderr> {
        self.raw().stderr.take()
    }

    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Retain the Unix leader's PID until its descendants have been terminated.
    /// Windows wait already settles the owned Job Object.
    pub(crate) async fn wait_tree(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            while !exited_without_reaping(pid)? {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            self.terminate();
        }
        self.wait().await
    }

    pub(crate) fn terminate(&mut self) {
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
fn exited_without_reaping(pid: u32) -> io::Result<bool> {
    // WNOWAIT leaves the exited leader unreaped, preventing PID/group reuse.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let result = libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        if result == 0 {
            return Ok(info.si_pid() != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        Err(error)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
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
