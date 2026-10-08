//! Foreground command ownership shared by shell tools.

use std::io;
use std::path::Path;
use std::process::ExitStatus;

use tokio::process::Command;

/// A captured stream from either the ordinary or AppContainer command backend.
pub type ReadStream = Box<dyn tokio::io::AsyncRead + Send + Unpin>;

#[cfg(windows)]
enum WindowsChild {
    Ordinary(Box<cyber_sandbox::windows_process::OwnedChild>),
    Container(Box<cyber_sandbox::windows_streams::PipedChild>),
}

pub struct Process {
    #[cfg(windows)]
    child: WindowsChild,
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
                child: WindowsChild::Ordinary(Box::new(command.spawn().await?)),
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

    /// Adapt an already authorized native container into the foreground command loop.
    /// Shell tools have no stdin producer, so close that endpoint immediately.
    #[cfg(windows)]
    pub fn from_container(mut child: cyber_sandbox::windows_streams::PipedChild) -> Self {
        drop(child.take_stdin());
        Self {
            child: WindowsChild::Container(Box::new(child)),
        }
    }

    pub fn stdout(&mut self) -> Option<ReadStream> {
        #[cfg(windows)]
        return match &mut self.child {
            WindowsChild::Ordinary(child) => child
                .child_mut()
                .stdout
                .take()
                .map(|s| Box::new(s) as ReadStream),
            WindowsChild::Container(child) => {
                child.take_stdout().map(|s| Box::new(s) as ReadStream)
            }
        };
        #[cfg(not(windows))]
        self.child.stdout.take().map(|s| Box::new(s) as ReadStream)
    }

    pub fn stderr(&mut self) -> Option<ReadStream> {
        #[cfg(windows)]
        return match &mut self.child {
            WindowsChild::Ordinary(child) => child
                .child_mut()
                .stderr
                .take()
                .map(|s| Box::new(s) as ReadStream),
            WindowsChild::Container(child) => {
                child.take_stderr().map(|s| Box::new(s) as ReadStream)
            }
        };
        #[cfg(not(windows))]
        self.child.stderr.take().map(|s| Box::new(s) as ReadStream)
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            match &mut self.child {
                WindowsChild::Ordinary(child) => child.wait().await,
                WindowsChild::Container(child) => child.wait().await.map(ExitStatus::from_raw),
            }
        }
        #[cfg(not(windows))]
        self.child.wait().await
    }

    /// Retain the Unix leader's PID until its descendants have been terminated.
    /// Windows wait already settles the owned Job Object.
    pub async fn wait_tree(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            while !exited_without_reaping(pid)? {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            self.terminate();
        }
        self.wait().await
    }

    pub fn terminate(&mut self) {
        #[cfg(windows)]
        match &mut self.child {
            WindowsChild::Ordinary(child) => child.terminate(),
            WindowsChild::Container(child) => child.terminate(),
        }
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
