//! Unix socket bridge used by the Linux sandbox helper.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::process::{Command, ExitCode};

pub(super) fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(split) = args.iter().position(|a| a == "--") else {
        return usage();
    };
    let (options, command) = (&args[..split], &args[split + 1..]);
    let ["--forward", port, socket] = options.iter().map(String::as_str).collect::<Vec<_>>()[..]
    else {
        return usage();
    };
    let Some(program) = command.first() else {
        return usage();
    };
    match TcpListener::bind(("127.0.0.1", port.parse().unwrap_or(3128))) {
        Ok(listener) => {
            let socket = socket.to_string();
            std::thread::spawn(move || forward(listener, &socket));
        }
        Err(e) => eprintln!("cyber-sandbox-exec: cannot listen for the proxy bridge: {e}"),
    }
    match Command::new(program).args(&command[1..]).status() {
        Ok(status) => ExitCode::from(exit_code(status)),
        Err(e) => {
            eprintln!("cyber-sandbox-exec: {program}: {e}");
            ExitCode::from(127)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: cyber-sandbox-exec --forward <port> <socket> -- <program> [args...]");
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
