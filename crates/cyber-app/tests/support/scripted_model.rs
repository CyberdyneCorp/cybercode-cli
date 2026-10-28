//! A local OpenAI-compatible model for production-path tests: an HTTP endpoint that
//! answers every streamed chat request with the `delta` its reply function chooses and
//! records each request body. A reply of `{"hold": true}` keeps the request open until
//! [`ScriptedModel::release`], so a test can stop a server mid-Turn, and `{"fail": 400}`
//! answers with that HTTP error status (and the error `"message"`, when the reply has one, or
//! the whole `"error"` object). A `"headers"` object in a reply is sent as
//! response headers (provider usage windows) instead of being part of the `delta`.
//! A reply of `{"gate": "<label>"}` logs `(label, start time)` and waits for one
//! [`ScriptedModel::open_gate`]; gated requests pass one per opening, oldest first, and
//! answer their `"then"` delta, else `<label> done`. It never reaches the internet.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The streamed `delta` for a request body.
pub type Reply = fn(&Value) -> Value;

/// Gated requests: the labels waiting, oldest first, openings not yet used, and every
/// gated request's start.
#[derive(Default)]
struct Gates {
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    waiting: VecDeque<u64>,
    held: Vec<(u64, String)>,
    open: usize,
    next: u64,
    starts: Vec<(String, Instant)>,
}

impl Gates {
    /// Wait until this request is the oldest gated one and an opening is available.
    fn pass(&self, label: &str, done: &AtomicBool) {
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next += 1;
            let id = state.next;
            state.waiting.push_back(id);
            state.held.push((id, label.to_owned()));
            state.starts.push((label.to_owned(), Instant::now()));
            id
        };
        while !done.load(Ordering::SeqCst) {
            {
                let mut state = self.state.lock().unwrap();
                if state.open > 0 && state.waiting.front() == Some(&id) {
                    state.open -= 1;
                    state.waiting.pop_front();
                    state.held.retain(|(held, _)| *held != id);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

pub struct ScriptedModel {
    pub url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    gates: Arc<Gates>,
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
        let gates = Arc::new(Gates::default());
        let (stop, seen, gate, gated) = (
            Arc::clone(&done),
            Arc::clone(&requests),
            Arc::clone(&released),
            Arc::clone(&gates),
        );
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (seen, gate) = (Arc::clone(&seen), Arc::clone(&gate));
                        let (gated, stop) = (Arc::clone(&gated), Arc::clone(&stop));
                        std::thread::spawn(move || {
                            answer(stream, reply, &seen, &gate, (&gated, &stop));
                        });
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
            gates,
            released,
            done,
            thread: Some(thread),
        }
    }

    /// Answer held requests (and every later one) normally.
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    /// Let the oldest gated request answer.
    pub fn open_gate(&self) {
        self.gates.state.lock().unwrap().open += 1;
    }

    /// Labels of gated requests still waiting, oldest first.
    pub fn held(&self) -> Vec<String> {
        let state = self.gates.state.lock().unwrap();
        state.held.iter().map(|(_, label)| label.clone()).collect()
    }

    /// Every gated request's label and arrival, in arrival order.
    pub fn starts(&self) -> Vec<(String, Instant)> {
        self.gates.state.lock().unwrap().starts.clone()
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
    (gates, done): (&Gates, &AtomicBool),
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
    if let Some(label) = delta.get("gate").and_then(Value::as_str).map(str::to_owned) {
        gates.pass(&label, done);
        delta = delta
            .get("then")
            .cloned()
            .unwrap_or_else(|| json!({"content": format!("{label} done")}));
    }
    if delta.get("hold").is_some() {
        while !released.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        delta = json!({"content": "released answer"});
    }
    let headers: String = delta
        .as_object_mut()
        .and_then(|d| d.remove("headers"))
        .and_then(|h| h.as_object().cloned())
        .unwrap_or_default()
        .iter()
        .map(|(name, value)| format!("{name}: {}\r\n", value.as_str().unwrap_or_default()))
        .collect();
    if let Some(status) = delta.get("fail").and_then(Value::as_u64) {
        let message = delta["message"].as_str().unwrap_or("scripted failure");
        let error = delta
            .get("error")
            .cloned()
            .unwrap_or_else(|| json!({"message": message, "type": "invalid_request_error"}));
        let error = json!({ "error": error }).to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 {status} Error\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{error}",
            error.len()
        );
        return;
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
        "HTTP/1.1 200 OK\r\n{headers}Content-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
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
