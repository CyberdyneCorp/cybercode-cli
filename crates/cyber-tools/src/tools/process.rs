//! Foreground command ownership shared by shell tools.

use std::io;
use std::path::Path;
use std::process::ExitStatus;

use tokio::process::Command;

/// A captured stream from either the ordinary or AppContainer command backend.
pub type ReadStream = Box<dyn tokio::io::AsyncRead + Send + Unpin>;
pub type WriteStream = Box<dyn tokio::io::AsyncWrite + Send + Unpin>;

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
    /// The leader is bubblewrap, which runs the command in a new session and dies on
    /// SIGTERM (taking the sandbox with it), so a graceful stop signals the command's own
    /// process groups instead.
    #[cfg(target_os = "linux")]
    bubblewrap: bool,
}

impl Process {
    /// Launch an already-authorized, sandbox-wrapped command with native tree ownership.
    /// Callers must enforce permissions and select the sandbox before calling this port.
    pub async fn spawn(
        program: &str,
        args: &[String],
        _helper: Option<&Path>,
        configure: impl FnOnce(&mut Command),
    ) -> io::Result<Self> {
        Self::spawn_inner(program, args, _helper, configure, false).await
    }

    pub(crate) async fn spawn_with_stdin(
        program: &str,
        args: &[String],
        helper: Option<&Path>,
        configure: impl FnOnce(&mut Command),
    ) -> io::Result<Self> {
        Self::spawn_inner(program, args, helper, configure, true).await
    }

    async fn spawn_inner(
        program: &str,
        args: &[String],
        _helper: Option<&Path>,
        configure: impl FnOnce(&mut Command),
        _event_stdin: bool,
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let helper = _helper.ok_or_else(|| {
                io::Error::other("cyber-sandbox-exec.exe is required for Windows process ownership")
            })?;
            let mut command = if _event_stdin {
                cyber_sandbox::windows_process::OwnedCommand::with_event_stdin(
                    helper, program, args,
                )
            } else {
                cyber_sandbox::windows_process::OwnedCommand::new(helper, program, args)
            };
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
                #[cfg(target_os = "linux")]
                bubblewrap: Path::new(program).file_name() == Some("bwrap".as_ref()),
            })
        }
    }

    /// Own a child started elsewhere, such as on a pseudo-terminal, that leads its own
    /// process group.
    #[cfg(unix)]
    pub(crate) fn from_child(child: tokio::process::Child) -> Self {
        Self {
            child,
            #[cfg(target_os = "linux")]
            bubblewrap: false,
        }
    }

    /// Adapt an already authorized native container into the foreground command loop.
    /// Shell tools have no stdin producer, so close that endpoint immediately.
    #[cfg(windows)]
    pub fn from_container(mut child: cyber_sandbox::windows_streams::PipedChild) -> Self {
        drop(child.take_stdin());
        Self::from_container_with_stdin(child)
    }

    /// Hook commands retain stdin so the owner can supply the event and close it.
    #[cfg(windows)]
    pub fn from_container_with_stdin(child: cyber_sandbox::windows_streams::PipedChild) -> Self {
        Self {
            child: WindowsChild::Container(Box::new(child)),
        }
    }

    pub fn stdin(&mut self) -> Option<WriteStream> {
        #[cfg(windows)]
        return match &mut self.child {
            WindowsChild::Ordinary(child) => child
                .child_mut()
                .stdin
                .take()
                .map(|s| Box::new(s) as WriteStream),
            WindowsChild::Container(child) => {
                child.take_stdin().map(|s| Box::new(s) as WriteStream)
            }
        };
        #[cfg(not(windows))]
        self.child.stdin.take().map(|s| Box::new(s) as WriteStream)
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

    /// Observe leader exit without consuming Unix process-group identity or Windows jobs.
    pub fn leader_exited(&mut self) -> io::Result<bool> {
        #[cfg(unix)]
        return self.child.id().map_or(Ok(true), exited_without_reaping);
        #[cfg(windows)]
        return match &mut self.child {
            WindowsChild::Ordinary(child) => {
                child.child_mut().try_wait().map(|status| status.is_some())
            }
            WindowsChild::Container(child) => container_exited(child),
        };
        #[cfg(not(any(unix, windows)))]
        self.child.try_wait().map(|status| status.is_some())
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

    #[cfg(unix)]
    pub(crate) fn request_graceful_stop(&mut self) -> io::Result<bool> {
        #[cfg(target_os = "linux")]
        if self.bubblewrap
            && let Some(leader) = self.child.id()
        {
            let groups = sandboxed_groups(leader);
            if !groups.is_empty() {
                for group in groups {
                    let _ = signal_group(Some(group), libc::SIGTERM);
                }
                return Ok(true);
            }
        }
        signal_group(self.child.id(), libc::SIGTERM)
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

#[cfg(windows)]
#[allow(unsafe_code)]
fn container_exited(child: &cyber_sandbox::windows_streams::PipedChild) -> io::Result<bool> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    // The borrowed native handle remains owned throughout this zero-duration observation.
    match unsafe { WaitForSingleObject(child.as_handle().as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// The process groups of `leader`'s descendants other than its own: under bubblewrap,
/// the sandboxed command's (it runs in a new session) and any its children made.
#[cfg(target_os = "linux")]
fn sandboxed_groups(leader: u32) -> Vec<u32> {
    // (pid, parent, group) of every visible process, from /proc/<pid>/stat.
    let processes: Vec<(u32, u32, u32)> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            // Fields after the parenthesized command: state, ppid, pgrp, ...
            let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
            let parent = fields.nth(1)?.parse().ok()?;
            let group = fields.next()?.parse().ok()?;
            Some((pid, parent, group))
        })
        .collect();
    let mut tree = vec![leader];
    let mut groups = Vec::new();
    while let Some(parent) = tree.pop() {
        for &(pid, _, group) in processes.iter().filter(|p| p.1 == parent) {
            tree.push(pid);
            if group != leader && !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
    groups
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    let _ = signal_group(pid, libc::SIGKILL);
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn signal_group(pid: Option<u32>, signal: i32) -> io::Result<bool> {
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return Ok(false);
    };
    // The unreaped child retains the identity of the group created at spawn.
    if unsafe { libc::killpg(pid, signal) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn exit_observation_retains_unreaped_process_group_identity() {
        let mut process = Process::spawn(
            "/bin/sh",
            &["-c".into(), "sleep 3600 & exit 0".into()],
            None,
            |command| {
                command.kill_on_drop(true);
            },
        )
        .await
        .unwrap();
        let leader = process.child.id().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !process.leader_exited().unwrap() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(process.child.id(), Some(leader));
        assert!(process.leader_exited().unwrap());
        assert_eq!(process.child.id(), Some(leader));
        process.terminate();
        tokio::time::timeout(Duration::from_secs(2), process.wait_tree())
            .await
            .unwrap()
            .unwrap();
    }
}
