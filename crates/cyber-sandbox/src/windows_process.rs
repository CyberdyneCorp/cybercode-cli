//! Parent-owned Windows command trees. This manages lifetime, not confinement.
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::ptr::null;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

/// Private stdin permit used only after the server assigns the trusted helper.
pub(crate) const START: &[u8] = b"CYBER-JOB-START\n";

struct Job(OwnedHandle);

impl Job {
    fn new() -> io::Result<Self> {
        // Neither a public name nor an inheritable handle can keep the job alive.
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The successful creation transfers one valid handle to this owner.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // The initialized structure stays alive throughout this synchronous call.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        let raw = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("Windows helper exited before job assignment"))?;
        // Tokio owns this live process handle. Only trusted handshake code is running.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// A trusted helper launch. The helper must be the installed cyber-sandbox-exec binary.
/// The user command cannot start until successful parent-owned job assignment.
pub struct OwnedCommand {
    command: Command,
}

impl OwnedCommand {
    pub fn new(
        helper: &Path,
        program: impl AsRef<OsStr>,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Self {
        let mut command = Command::new(helper);
        command.args(["--parent-job", "--"]).arg(program).args(args);
        Self { command }
    }

    /// Configure environment, working directory and output. Stdin is reserved for
    /// the launch handshake; the eventual user command receives null stdin.
    pub fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    pub async fn spawn(&mut self) -> io::Result<OwnedChild> {
        let job = Job::new()?;
        self.command.stdin(Stdio::piped()).kill_on_drop(true);
        let child = self.command.spawn()?;
        let mut owned = OwnedChild {
            child,
            job: Some(job),
        };
        owned.job.as_ref().unwrap().assign(&owned.child)?;
        let mut permit = owned
            .child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Windows helper launch pipe is missing"))?;
        permit.write_all(START).await?;
        drop(permit);
        Ok(owned)
    }
}

/// Retains the parent's sole job handle until command settlement or cancellation.
/// Dropping the owner, including during an aborted future, terminates the tree.
pub struct OwnedChild {
    child: Child,
    job: Option<Job>,
}

impl OwnedChild {
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Terminate the whole tree without relying on a reusable PID.
    pub fn terminate(&mut self) {
        drop(self.job.take());
        let _ = self.child.start_kill();
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().await;
        self.terminate();
        status
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}
