//! Real HTTP body faults, without a live model or credentials.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use cyber_llm::adapters::{ApiKind, Endpoint, adapter};
use cyber_llm::{ErrorKind, LlmEvent, LlmRequest, RetryPolicy, collect, open_with_retry};
use futures::StreamExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct FaultServer {
    url: String,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How a response body ends after its events.
#[derive(Clone, Copy, PartialEq)]
enum Tail {
    /// The terminating chunk, then close.
    Complete,
    /// Close without the terminating chunk.
    Broken,
    /// Keep the connection open and send nothing more.
    Held,
}

impl FaultServer {
    /// Each request gets the next response; the last one repeats.
    async fn start(responses: Vec<(String, Tail)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                // Read the complete request before closing, so the injected fault is
                // in the response body rather than a reset during request upload.
                let mut request = Vec::new();
                let header_end = loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break request.len();
                    }
                };
                let headers = String::from_utf8_lossy(&request);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                request.resize(header_end + length, 0);
                socket.read_exact(&mut request[header_end..]).await.unwrap();
                let index = count.fetch_add(1, Ordering::SeqCst);
                let (body, tail) = &responses[index.min(responses.len() - 1)];
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                if *tail == Tail::Held {
                    // Hold this connection open while later requests are served.
                    tokio::spawn(async move {
                        let _socket = socket;
                        std::future::pending::<()>().await
                    });
                    continue;
                }
                if *tail == Tail::Complete {
                    socket.write_all(b"0\r\n\r\n").await.unwrap();
                }
                socket.shutdown().await.unwrap();
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

fn event(kind: &str, data: serde_json::Value) -> String {
    format!("event: {kind}\ndata: {data}\n\n")
}

fn text() -> String {
    event("response.output_text.delta", json!({"delta":"hello"}))
}
fn completed() -> String {
    event(
        "response.completed",
        json!({"response":{"usage":{"input_tokens":2,"output_tokens":1}}}),
    )
}

fn policy() -> RetryPolicy {
    RetryPolicy {
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        ..Default::default()
    }
}

fn model(server: &FaultServer) -> Box<dyn cyber_llm::Adapter> {
    let mut endpoint = Endpoint::new(server.url.clone(), None);
    endpoint.client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    adapter(ApiKind::OpenaiResponses, endpoint)
}

#[tokio::test]
async fn completed_response_survives_broken_http_tail() {
    let server = FaultServer::start(vec![(text() + &completed(), Tail::Broken)]).await;
    let output = collect(
        model(&server).stream(LlmRequest::default()).await.unwrap(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(output.text, "hello");
    assert_eq!(output.usage.input, 2);
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn completed_response_does_not_wait_for_http_eof() {
    let server = FaultServer::start(vec![(text() + &completed(), Tail::Held)]).await;
    let output = tokio::time::timeout(Duration::from_secs(10), async {
        collect(
            model(&server).stream(LlmRequest::default()).await.unwrap(),
            |_| {},
        )
        .await
    })
    .await
    .expect("protocol completion must not wait for HTTP EOF")
    .unwrap();
    assert_eq!(output.text, "hello");
    assert_eq!(output.usage.input, 2);
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn broken_body_before_output_retries_then_succeeds() {
    let server = FaultServer::start(vec![
        (
            event("response.created", json!({"response":{"id":"r1"}})),
            Tail::Broken,
        ),
        (text() + &completed(), Tail::Complete),
    ])
    .await;
    let adapter = model(&server);
    let mut retries = 0;
    let stream = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |retry| {
            retries += 1;
            assert!(!retry.discarded, "nothing was streamed");
            assert_eq!(retry.error.kind, ErrorKind::Transport);
            assert!(
                retry
                    .error
                    .message
                    .contains("response stream interrupted after"),
                "{}",
                retry.error
            );
        },
    )
    .await
    .unwrap();
    let mut seen = Vec::new();
    let output = collect(stream, |event| seen.push(event.clone()))
        .await
        .unwrap();
    assert_eq!(output.text, "hello");
    assert!(!seen.contains(&LlmEvent::Discarded));
    assert_eq!(retries, 1);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
}

/// What an interrupted attempt can have streamed before its body broke.
fn partial_prefixes() -> [String; 4] {
    let call = event(
        "response.output_item.added",
        json!({"item":{"type":"function_call","id":"i1","call_id":"c1","name":"write"}}),
    );
    [
        text(),
        event(
            "response.reasoning_summary_text.delta",
            json!({"delta":"thinking"}),
        ),
        call.clone()
            + &event(
                "response.function_call_arguments.delta",
                json!({"item_id":"i1","delta":"{\"path\":"}),
            ),
        // Complete but never dispatched: the stream had not ended.
        call + &event(
            "response.output_item.done",
            json!({"item":{"type":"function_call","id":"i1","arguments":"{}"}}),
        ),
    ]
}

/// `provider-catalog` → Retry policy: partial text, reasoning, streamed tool arguments or a
/// completed but undispatched call are discarded, and the same request is replayed.
#[tokio::test]
async fn broken_body_after_partial_output_is_discarded_and_replayed() {
    for prefix in partial_prefixes() {
        let server = FaultServer::start(vec![
            (prefix, Tail::Broken),
            (text() + &completed(), Tail::Complete),
        ])
        .await;
        let adapter = model(&server);
        let mut notices = Vec::new();
        let stream = open_with_retry(
            adapter.as_ref(),
            &LlmRequest::default(),
            &policy(),
            |retry| {
                notices.push((retry.attempt, retry.discarded, retry.error.kind));
            },
        )
        .await
        .unwrap();
        let mut seen = Vec::new();
        let output = collect(stream, |event| seen.push(event.clone()))
            .await
            .unwrap();
        let discarded = seen
            .iter()
            .position(|event| *event == LlmEvent::Discarded)
            .expect("the partial attempt is discarded");
        assert!(seen[..discarded].iter().any(LlmEvent::is_output));
        assert_eq!(notices, [(1, true, ErrorKind::Transport)]);
        assert_eq!(output.text, "hello");
        assert!(output.reasoning.is_empty() && output.tool_calls.is_empty());
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn clean_http_eof_before_protocol_completion_is_an_error() {
    let server = FaultServer::start(vec![(text(), Tail::Complete)]).await;
    let error = collect(
        model(&server).stream(LlmRequest::default()).await.unwrap(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Transport);
    assert!(
        error
            .message
            .contains("stream ended before response.completed")
    );
}

#[tokio::test]
async fn repeated_body_faults_stop_at_the_retry_limit() {
    let body = event("response.created", json!({"response":{"id":"r1"}}));
    let server = FaultServer::start(vec![(body.clone(), Tail::Broken)]).await;
    let adapter = model(&server);
    let mut retries = 0;
    let error = open_with_retry(adapter.as_ref(), &LlmRequest::default(), &policy(), |_| {
        retries += 1;
    })
    .await
    .err()
    .expect("all five attempts must fail");
    assert_eq!(retries, 4);
    assert_eq!(server.requests.load(Ordering::SeqCst), 5);
    assert_eq!(error.kind, ErrorKind::Transport);
    assert!(
        error
            .message
            .contains(&format!("after {} body bytes", body.len())),
        "{error}"
    );
}

/// Failures before and after output share one budget of four retries; the last attempt's
/// failure is returned with its partial output, so the Turn settles as interrupted.
#[tokio::test]
async fn retries_before_and_after_output_share_one_budget() {
    let created = event("response.created", json!({"response":{"id":"r1"}}));
    let server = FaultServer::start(vec![
        (created.clone(), Tail::Broken),
        (text(), Tail::Broken),
        (created, Tail::Broken),
        (text(), Tail::Broken),
    ])
    .await;
    let adapter = model(&server);
    let mut notices = Vec::new();
    let stream = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |retry| {
            notices.push(retry.discarded);
        },
    )
    .await
    .unwrap();
    let mut seen = Vec::new();
    let error = collect(stream, |event| seen.push(event.clone()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Transport);
    assert_eq!(notices, [false, true, false, true]);
    assert_eq!(server.requests.load(Ordering::SeqCst), 5);
    let last = seen
        .iter()
        .rposition(|e| *e == LlmEvent::Discarded)
        .unwrap();
    assert!(
        seen[last..].iter().any(LlmEvent::is_output),
        "the final attempt's partial output reaches the caller"
    );
}

/// `provider-catalog` → "Stalled stream": headers and one event, then silence for the
/// idle window, fails as a transport error; the partial output is discarded and replayed.
#[tokio::test]
async fn a_stalled_stream_fails_after_the_idle_window_and_is_replayed() {
    let server = FaultServer::start(vec![
        (text(), Tail::Held),
        (text() + &completed(), Tail::Complete),
    ])
    .await;
    // No client-wide timeout: under the paused clock only the idle timer may fire.
    let mut endpoint = Endpoint::new(server.url.clone(), None);
    endpoint.client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        endpoint.idle_timeout,
        Some(cyber_llm::adapters::DEFAULT_IDLE_TIMEOUT)
    );
    let adapter = adapter(ApiKind::OpenaiResponses, endpoint);
    let mut errors = Vec::new();
    let mut stream = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |retry| {
            errors.push(retry.error.message.clone());
        },
    )
    .await
    .unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert!(first.is_output(), "{first:?}");

    tokio::time::pause();
    let started = tokio::time::Instant::now();
    assert_eq!(stream.next().await.unwrap().unwrap(), LlmEvent::Discarded);
    assert!(started.elapsed() >= Duration::from_secs(300));
    // The replay runs on real time: a paused clock would fire its idle timer at once.
    tokio::time::resume();
    let output = collect(stream, |_| {}).await.unwrap();
    assert_eq!(output.text, "hello");
    assert_eq!(errors, ["stream idle for 300s"]);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
}
