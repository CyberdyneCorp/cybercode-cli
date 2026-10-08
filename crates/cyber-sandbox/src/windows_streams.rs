//! Invocation-owned asynchronous standard streams for the native AppContainer launcher.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::path::Path;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

use crate::windows_container::Profile;
use crate::windows_launch::{ContainerChild, StandardStreams, spawn_with_stdio};

/// Streams can be taken by the tool loop; this owner retains the entire native process tree.
/// Dropping it, including an unpolled wait future, terminates that tree.
pub struct PipedChild {
    child: ContainerChild,
    stdin: Option<NamedPipeServer>,
    stdout: Option<NamedPipeServer>,
    stderr: Option<NamedPipeServer>,
}

/// Connect host-owned pipes before allowing native user code to run. Only their three
/// synchronous client handles are inherited; reactor/server handles remain in the host.
pub async fn spawn_piped(
    profile: &Profile,
    program: &Path,
    args: &[OsString],
    environment: &BTreeMap<String, String>,
    directory: &Path,
) -> io::Result<PipedChild> {
    let (stdin, input) = stream(false).await?;
    let (stdout, output) = stream(true).await?;
    let (stderr, error) = stream(true).await?;
    let child = spawn_with_stdio(
        profile,
        program,
        args,
        environment,
        directory,
        StandardStreams {
            stdin: input.as_handle(),
            stdout: output.as_handle(),
            stderr: error.as_handle(),
        },
    )?;
    // The launcher duplicated these handles. Keeping a host copy would delay pipe EOF.
    drop((input, output, error));
    Ok(PipedChild {
        child,
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

async fn stream(read_from_child: bool) -> io::Result<(NamedPipeServer, File)> {
    let name = format!(r"\\.\pipe\cyber-{}", cyber_core::ids::new_id("pipe"));
    let server = ServerOptions::new()
        .access_inbound(read_from_child)
        .access_outbound(!read_from_child)
        .first_pipe_instance(true)
        .max_instances(1)
        .reject_remote_clients(true)
        .create(&name)?;
    let client = OpenOptions::new()
        .read(!read_from_child)
        .write(read_from_child)
        .open(&name)?;
    server.connect().await?;
    let mut process = 0;
    // The connected server handle remains owned here throughout the query.
    if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut process) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if process != std::process::id() {
        return Err(io::Error::other(
            "Standard-stream pipe has a foreign client",
        ));
    }
    Ok((server, client))
}

impl AsHandle for PipedChild {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.child.as_handle()
    }
}

impl PipedChild {
    pub fn take_stdin(&mut self) -> Option<NamedPipeServer> {
        self.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<NamedPipeServer> {
        self.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<NamedPipeServer> {
        self.stderr.take()
    }

    pub fn temporary_directory(&self) -> &Path {
        self.child.temporary_directory()
    }

    pub fn terminate(&mut self) {
        self.child.terminate();
    }

    /// Unused stdin closes before waiting. Take and drain stdout/stderr concurrently
    /// with this wait so pipe backpressure cannot prevent the child from exiting.
    pub async fn wait_owned(mut self) -> io::Result<u32> {
        drop(self.stdin.take());
        self.child.wait_owned().await
    }
}
