//! Unix socket bridge and terminal owner used by the Linux sandbox helper.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, ExitCode};

/// `--forward <port> <socket> -- <program>…` bridges the proxy port to its socket;
/// `--tty -- <program>…` starts the program as the leader of a new session whose
/// controlling terminal is stdin. Either way the program runs as a child until it exits.
pub(super) fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(split) = args.iter().position(|a| a == "--") else {
        return usage();
    };
    let (options, command) = (&args[..split], &args[split + 1..]);
    let terminal = match options.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--forward", port, socket] => {
            listen(port, socket);
            false
        }
        ["--tty"] => true,
        _ => return usage(),
    };
    let Some(program) = command.first() else {
        return usage();
    };
    let mut child = Command::new(program);
    child.args(&command[1..]);
    if terminal {
        take_terminal(&mut child);
    }
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("cyber-sandbox-exec: {program}: {e}");
            return ExitCode::from(127);
        }
    };
    outlive_termination();
    match child.wait() {
        Ok(status) => ExitCode::from(exit_code(status)),
        Err(e) => {
            eprintln!("cyber-sandbox-exec: {program}: {e}");
            ExitCode::from(1)
        }
    }
}

/// A stop sends SIGTERM to the command's process group, which includes this bridge.
/// Dying then would end the sandbox, killing the command before it handles the signal,
/// so the bridge ignores it and exits with the command. Set after the spawn, so the
/// command keeps the default disposition.
#[allow(unsafe_code)]
fn outlive_termination() {
    // SAFETY: SIG_IGN installs no handler code; only this process's disposition changes.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

fn listen(port: &str, socket: &str) {
    match TcpListener::bind(("127.0.0.1", port.parse().unwrap_or(3128))) {
        Ok(listener) => {
            let socket = socket.to_string();
            std::thread::spawn(move || forward(listener, &socket));
        }
        Err(e) => eprintln!("cyber-sandbox-exec: cannot listen for the proxy bridge: {e}"),
    }
}

/// A PTY Job's program inside bubblewrap: bubblewrap's own session cannot hold the
/// terminal (it would get ctrl-c's SIGINT and take the sandbox down), so the program
/// leads a session of its own with the terminal, and ctrl-c reaches it alone.
#[allow(unsafe_code)]
fn take_terminal(command: &mut Command) {
    // SAFETY: only async-signal-safe calls run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: cyber-sandbox-exec --forward <port> <socket> -- <program> [args...]\n       cyber-sandbox-exec --tty -- <program> [args...]"
    );
    ExitCode::from(2)
}

fn forward(listener: TcpListener, socket: &str) {
    for client in listener.incoming().flatten() {
        let Ok(upstream) = UnixStream::connect(socket) else {
            continue;
        };
        std::thread::spawn(move || pipe(client, upstream));
    }
}

fn pipe(client: TcpStream, upstream: UnixStream) {
    let (Ok(mut client_r), Ok(mut upstream_w)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    let to_upstream = std::thread::spawn(move || {
        copy(&mut client_r, &mut upstream_w);
        let _ = upstream_w.shutdown(Shutdown::Write);
    });
    let (mut upstream_r, mut client_w) = (upstream, client);
    copy(&mut upstream_r, &mut client_w);
    let _ = client_w.shutdown(Shutdown::Write);
    let _ = to_upstream.join();
}

fn copy(from: &mut impl Read, to: &mut impl Write) {
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 || to.write_all(&buf[..n]).is_err() {
            return;
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => code as u8,
        (None, Some(signal)) => 128u8.wrapping_add(signal as u8),
        _ => 1,
    }
}
