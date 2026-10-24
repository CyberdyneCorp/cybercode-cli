//! A loopback HTTP server standing in for the release source: it serves scripted replies
//! per path and records every request, so tests can count what reached the network.
//! It uses only the standard library, so the CLI tests share it with `#[path]`.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One scripted reply.
#[derive(Clone)]
pub enum Reply {
    Ok(Vec<u8>),
    Status(u16),
    /// Wait before answering, to trip a client timeout.
    Delay(Duration, Vec<u8>),
    /// Declare `declared` bytes but send only the body, then close: an interrupted
    /// download.
    Truncated {
        declared: usize,
        body: Vec<u8>,
    },
}

/// A request the stub received.
#[derive(Debug, Clone)]
pub struct Request {
    pub path: String,
    pub user_agent: Option<String>,
}

#[derive(Default)]
struct State {
    /// Replies per path, in order; the last one repeats.
    routes: HashMap<String, VecDeque<Reply>>,
    requests: Vec<Request>,
}

pub struct StubRelease {
    pub base_url: String,
    state: Arc<Mutex<State>>,
}

impl StubRelease {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || serve(stream, &state));
            }
        });
        Self { base_url, state }
    }

    /// Serve `replies` at `path`, in order, repeating the last one.
    pub fn route(&self, path: &str, replies: Vec<Reply>) {
        let mut state = self.state.lock().unwrap();
        state.routes.insert(path.to_string(), replies.into());
    }

    /// Serve a channel manifest naming `version`.
    pub fn manifest(&self, channel: &str, version: &str) {
        let body = format!(
            r#"{{"version": "{version}", "published_at": "2026-10-01T00:00:00Z", "notes_url": "https://example.test/notes/{version}", "min_supported": "0.0.1"}}"#
        );
        self.route(
            &format!("/channel-{channel}/{channel}.json"),
            vec![Reply::Ok(body.into_bytes())],
        );
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut user_agent = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("user-agent")
        {
            user_agent = Some(value.trim().to_string());
        }
    }
    let reply = {
        let mut state = state.lock().unwrap();
        state.requests.push(Request {
            path: path.clone(),
            user_agent,
        });
        state.routes.get_mut(&path).and_then(|replies| {
            if replies.len() > 1 {
                replies.pop_front()
            } else {
                replies.front().cloned()
            }
        })
    };
    let (status, body, length) = match reply {
        Some(Reply::Ok(body)) => {
            let length = body.len();
            (200, body, length)
        }
        Some(Reply::Truncated { declared, body }) => (200, body, declared),
        Some(Reply::Status(code)) => (code, Vec::new(), 0),
        Some(Reply::Delay(wait, body)) => {
            std::thread::sleep(wait);
            let length = body.len();
            (200, body, length)
        }
        None => (404, Vec::new(), 0),
    };
    let mut stream = reader.into_inner();
    let head = format!(
        "HTTP/1.1 {status} Stub\r\nContent-Length: {length}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}
