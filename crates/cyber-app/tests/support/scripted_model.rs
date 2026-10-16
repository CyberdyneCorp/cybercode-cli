//! A local OpenAI-compatible model for production-path tests: an HTTP endpoint that
//! answers every streamed chat request with the `delta` its reply function chooses and
//! records each request body. A reply of `{"hold": true}` keeps the request open until
//! [`ScriptedModel::release`], so a test can stop a server mid-Turn. It never reaches the
//! internet.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// The streamed `delta` for a request body.
pub type Reply = fn(&Value) -> Value;

pub struct ScriptedModel {
    pub url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    released: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScriptedModel {
    pub fn start(reply: Reply) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (stop, seen, gate) = (
            Arc::clone(&done),
            Arc::clone(&requests),
            Arc::clone(&released),
        );
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (seen, gate) = (Arc::clone(&seen), Arc::clone(&gate));
                        std::thread::spawn(move || answer(stream, reply, &seen, &gate));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            requests,
            released,
            done,
            thread: Some(thread),
        }
    }

    /// Answer held requests (and every later one) normally.
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    /// Request bodies received so far.
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// The global configuration naming this model as provider `test`, model `main`.
    pub fn config(&self) -> Value {
        json!({
            "providers":{"test":{"api":{"type":"openai-compatible","url":format!("{}/v1", self.url),
                "settings":{"auth":"none"}},"models":{"main":{}}}},
            "model":"test/main",
        })
    }
}

fn answer(
    mut stream: std::net::TcpStream,
    reply: Reply,
    seen: &Mutex<Vec<Value>>,
    released: &AtomicBool,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Some(body) = read_body(&mut stream) else {
        return;
    };
    let mut delta = reply(&body);
    seen.lock().unwrap().push(body.clone());
    if delta.get("hold").is_some() {
        while !released.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        delta = json!({"content": "released answer"});
    }
    let finish = if delta.get("tool_calls").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    let chunk = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],
        "usage":{"prompt_tokens":100,"completion_tokens":10}});
    let reply = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

fn read_body(stream: &mut std::net::TcpStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            if bytes.len() >= end + 4 + length {
                return Some(
                    serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap_or_default(),
                );
            }
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
}

impl Drop for ScriptedModel {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
