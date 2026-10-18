//! Pseudo-terminals for PTY Jobs (`background-tasks` → PTY sessions), Unix only: the
//! program runs on the terminal's slave side as a new session leader with it as its
//! controlling terminal, and the master side is read and written without blocking.

use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::process::Stdio;

use tokio::io::unix::AsyncFd;
use tokio::process::Command;

use super::process::Process;

/// The master side of a pseudo-terminal.
pub(crate) struct Master {
    fd: AsyncFd<OwnedFd>,
}

/// Open a terminal of `cols` × `rows` and start `program` on it.
pub(crate) fn spawn(
    program: &str,
    args: &[String],
    (cols, rows): (u16, u16),
    configure: impl FnOnce(&mut Command),
) -> io::Result<(Process, Master)> {
    let (master, slave) = open(cols, rows)?;
    let mut command = Command::new(program);
    command.args(args);
    configure(&mut command);
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    become_session_leader(&mut command);
    let child = command.spawn()?;
    // The parent's copies of the slave side close with the command.
    drop(command);
    Ok((
        Process::from_child(child),
        Master {
            fd: AsyncFd::new(master)?,
        },
    ))
}

#[allow(unsafe_code)]
fn open(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the out-pointers are valid for the call; name and termios may be null.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            // `*const winsize` on Linux and `*mut winsize` on macOS; a raw pointer fits both.
            &raw mut size,
        )
    };
    if opened != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two fresh descriptors this function now owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    set_flags(&master, 0)?;
    set_flags(&slave, 0)?;
    set_flags(&master, libc::O_NONBLOCK)?;
    Ok((master, slave))
}

/// Close-on-exec, plus the status `flags` when non-zero.
#[allow(unsafe_code)]
fn set_flags(fd: &OwnedFd, flags: libc::c_int) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: `raw` is an open descriptor for the duration of the calls.
    let ok = unsafe {
        if flags == 0 {
            libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) != -1
        } else {
            let current = libc::fcntl(raw, libc::F_GETFL);
            current != -1 && libc::fcntl(raw, libc::F_SETFL, current | flags) != -1
        }
    };
    if ok {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// In the child: a new session whose controlling terminal is its stdin. The session
/// leader's process group is the one a stop signals.
#[allow(unsafe_code)]
fn become_session_leader(command: &mut Command) {
    // SAFETY: only async-signal-safe calls run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

impl Master {
    /// Read output; `Ok(0)` once every slave descriptor is closed.
    #[allow(unsafe_code)]
    pub(crate) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            // SAFETY: `buf` is valid for `buf.len()` bytes and the descriptor is open.
            let read = guard.try_io(|fd| {
                let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n >= 0 {
                    Ok(n.unsigned_abs())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
            match read {
                Ok(Ok(n)) => return Ok(n),
                // Linux reports a hung-up terminal as EIO.
                Ok(Err(error)) if error.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => {}
            }
        }
    }

    /// Write all of `bytes` as input.
    #[allow(unsafe_code)]
    pub(crate) async fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut guard = self.fd.writable().await?;
            // SAFETY: `bytes` is valid for its length and the descriptor is open.
            let written = guard.try_io(|fd| {
                let n = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
                if n >= 0 {
                    Ok(n.unsigned_abs())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
            match written {
                Ok(Ok(n)) => bytes = &bytes[n..],
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => {}
            }
        }
        Ok(())
    }
}
