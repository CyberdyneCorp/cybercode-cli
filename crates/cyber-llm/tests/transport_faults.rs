//! Real HTTP body faults, without a live model or credentials.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use cyber_llm::adapters::{ApiKind, Endpoint, adapter};
use cyber_llm::{ErrorKind, LlmEvent, LlmRequest, RetryPolicy, collect, open_with_retry};
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

impl FaultServer {
    async fn start(responses: Vec<(String, bool)>) -> Self {
        Self::start_with_held_tail(responses, false).await
    }

    async fn start_with_held_tail(responses: Vec<(String, bool)>, held: bool) -> Self {
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
                let (body, broken) = &responses[index.min(responses.len() - 1)];
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                if held {
                    std::future::pending::<()>().await;
                }
                if !broken {
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
    let server = FaultServer::start(vec![(text() + &completed(), true)]).await;
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
    let server =
        FaultServer::start_with_held_tail(vec![(text() + &completed(), false)], true).await;
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
            true,
        ),
        (text() + &completed(), false),
    ])
    .await;
    let adapter = model(&server);
    let mut retries = 0;
    let stream = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |_, _, error| {
            retries += 1;
            assert_eq!(error.kind, ErrorKind::Transport);
            assert!(
                error.message.contains("response stream interrupted after"),
                "{error}"
            );
        },
    )
    .await
    .unwrap();
    assert_eq!(collect(stream, |_| {}).await.unwrap().text, "hello");
    assert_eq!(retries, 1);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn broken_body_after_partial_output_is_never_retried() {
    for prefix in [
        text(),
        event(
            "response.reasoning_summary_text.delta",
            json!({"delta":"thinking"}),
        ),
        event(
            "response.output_item.added",
            json!({"item":{"type":"function_call","id":"i1","call_id":"c1","name":"write"}}),
        ) + &event(
            "response.function_call_arguments.delta",
            json!({"item_id":"i1","delta":"{\"path\":"}),
        ),
        event(
            "response.output_item.added",
            json!({"item":{"type":"function_call","id":"i1","call_id":"c1","name":"write"}}),
        ) + &event(
            "response.output_item.done",
            json!({"item":{"type":"function_call","id":"i1","arguments":"{}"}}),
        ),
    ] {
        let server = FaultServer::start(vec![(prefix, true)]).await;
        let adapter = model(&server);
        let stream = open_with_retry(
            adapter.as_ref(),
            &LlmRequest::default(),
            &policy(),
            |_, _, _| panic!("partial output must not be replayed"),
        )
        .await
        .unwrap();
        let mut seen = Vec::new();
        let error = collect(stream, |event| seen.push(event.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Transport);
        assert!(
            error.message.contains("response stream interrupted after"),
            "{error}"
        );
        assert!(seen.iter().any(LlmEvent::is_output));
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn clean_http_eof_before_protocol_completion_is_an_error() {
    let server = FaultServer::start(vec![(text(), false)]).await;
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
    let server = FaultServer::start(vec![(body.clone(), true)]).await;
    let adapter = model(&server);
    let mut retries = 0;
    let error = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |_, _, _| {
            retries += 1;
        },
    )
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

/// `provider-catalog` → "Stalled stream": headers and one event, then silence for the
/// idle window, fails as a transport error, and the partial output is not replayed.
#[tokio::test]
async fn a_stalled_stream_fails_after_the_idle_window() {
    let server = FaultServer::start_with_held_tail(vec![(text(), false)], true).await;
    // No client-wide timeout: under the paused clock only the idle timer may fire.
    let mut endpoint = Endpoint::new(server.url.clone(), None);
    endpoint.client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        endpoint.idle_timeout,
        Some(cyber_llm::adapters::DEFAULT_IDLE_TIMEOUT)
    );
    let adapter = adapter(ApiKind::OpenaiResponses, endpoint);
    let mut stream = open_with_retry(
        adapter.as_ref(),
        &LlmRequest::default(),
        &policy(),
        |_, _, _| panic!("partial output must not be replayed"),
    )
    .await
    .unwrap();
    use futures::StreamExt;
    let first = stream.next().await.unwrap().unwrap();
    assert!(first.is_output(), "{first:?}");

    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind, ErrorKind::Transport);
    assert_eq!(error.message, "stream idle for 300s");
    assert!(started.elapsed() >= Duration::from_secs(300));
    assert!(
        stream.next().await.is_none(),
        "the stream ends with the failure"
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}
