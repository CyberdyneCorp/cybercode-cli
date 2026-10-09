//! Actual byte framing and RPC ownership without claiming subprocess/HTTP settlement.
use cyber_core::hooks::{HookAction, HookEvent, HookLocation};
use cyber_tools::mcp::{McpError, StdioClient, decision, render_arguments};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

type Client = StdioClient<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;
type Peer = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);
const TIMEOUT: Duration = Duration::from_secs(1);
fn pair() -> (Client, Peer) {
    let (client, peer) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client);
    let (peer_read, peer_write) = tokio::io::split(peer);
    (
        StdioClient::new(read, write),
        (BufReader::new(peer_read), peer_write),
    )
}
async fn read(peer: &mut Peer) -> Value {
    let mut line = String::new();
    peer.0.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}
async fn send(peer: &mut Peer, value: Value) {
    peer.1
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}
async fn initialize_peer(peer: &mut Peer) {
    let message = read(peer).await;
    assert_eq!(message["method"], "initialize");
    assert_eq!(message["params"]["capabilities"], json!({}));
    send(peer, json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}})).await;
    let message = read(peer).await;
    assert_eq!(message["method"], "notifications/initialized");
    assert!(message.get("id").is_none());
}

#[tokio::test]
async fn initialization_and_tool_call_have_distinct_ids_and_exact_arguments() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        let message = read(&mut peer).await;
        assert_eq!(message["method"], "tools/call");
        assert_eq!(message["params"]["arguments"], json!({"nested":{"x":2}}));
        send(&mut peer, json!({"jsonrpc":"2.0","id":message["id"],"result":{"content":[{"type":"text","text":"{\"decision\":\"deny\"}"}]}})).await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    let result = client
        .call_tool("judge", json!({"nested":{"x":2}}), TIMEOUT)
        .await
        .unwrap();
    assert_eq!(result["content"][0]["type"], "text");
    assert!(!client.unresolved());
    server.await.unwrap();
}

#[tokio::test]
async fn server_ping_is_answered_and_unadvertised_callbacks_are_refused() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        let pending = read(&mut peer).await;
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":"ping","method":"ping"}),
        )
        .await;
        assert_eq!(read(&mut peer).await["result"], json!({}));
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":"sampling","method":"sampling/createMessage","params":{}}),
        )
        .await;
        assert_eq!(read(&mut peer).await["error"]["code"], -32601);
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":pending["id"],"result":{"content":[]}}),
        )
        .await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    client.call_tool("audit", json!({}), TIMEOUT).await.unwrap();
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn matching_progress_resets_request_inactivity() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        let pending = read(&mut peer).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        send(&mut peer, json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":pending["id"],"progress":1}})).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":pending["id"],"result":{}}),
        )
        .await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    client.call_tool("audit", json!({}), TIMEOUT).await.unwrap();
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn foreign_progress_cannot_keep_a_request_alive() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        read(&mut peer).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        send(&mut peer, json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"foreign","progress":1}})).await;
        std::future::pending::<()>().await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(matches!(
        client.call_tool("audit", json!({}), TIMEOUT).await,
        Err(McpError::Timeout)
    ));
    assert!(client.unresolved());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn explicit_cancellation_uses_pending_identity_without_claiming_settlement() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        let pending = read(&mut peer).await;
        let cancelled = read(&mut peer).await;
        assert_eq!(cancelled["method"], "notifications/cancelled");
        assert_eq!(cancelled["params"]["requestId"], pending["id"]);
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(matches!(
        client
            .call_tool("audit", json!({}), Duration::from_millis(10))
            .await,
        Err(McpError::Timeout)
    ));
    client.cancel_pending(TIMEOUT).await.unwrap();
    assert!(client.unresolved());
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_initialization_refuses_reuse_and_cancellation_notification() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        read(&mut peer).await;
        std::future::pending::<()>().await;
    });
    assert!(matches!(
        client.initialize(Duration::from_millis(10)).await,
        Err(McpError::Timeout)
    ));
    assert!(client.unresolved());
    assert!(matches!(
        client.cancel_pending(TIMEOUT).await,
        Err(McpError::Protocol(_))
    ));
    assert!(matches!(
        client.initialize(TIMEOUT).await,
        Err(McpError::Protocol(_))
    ));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn malformed_unterminated_and_oversized_frames_poison_initialization() {
    for bytes in [
        b"not JSON\n".to_vec(),
        b"{}".to_vec(),
        vec![b'x'; 1024 * 1024 + 2],
    ] {
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            read(&mut peer).await;
            let _ = peer.1.write_all(&bytes).await;
            peer.1.shutdown().await.unwrap();
        });
        assert!(matches!(
            client.initialize(TIMEOUT).await,
            Err(McpError::Protocol(_))
        ));
        assert!(client.unresolved());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn oversized_outgoing_calls_are_refused_before_pending_or_peer_effects() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize_peer(&mut peer).await;
        let request = read(&mut peer).await;
        assert_eq!(request["params"]["name"], "second");
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":request["id"],"result":{}}),
        )
        .await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(matches!(
        client
            .call_tool("oversized", json!({"value":"x".repeat(1024*1024)}), TIMEOUT)
            .await,
        Err(McpError::Protocol(_))
    ));
    assert!(!client.unresolved());
    client
        .call_tool("second", json!({}), TIMEOUT)
        .await
        .unwrap();
    server.await.unwrap();
}

struct LimitedWriter {
    writer: WriteHalf<DuplexStream>,
    remaining: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl tokio::io::AsyncWrite for LimitedWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::sync::atomic::Ordering::SeqCst;
        let remaining = self.remaining.load(SeqCst);
        if remaining == 0 {
            return std::task::Poll::Pending;
        }
        let length = bytes.len().min(remaining);
        let result = std::pin::Pin::new(&mut self.writer).poll_write(cx, &bytes[..length]);
        if let std::task::Poll::Ready(Ok(n)) = result {
            self.remaining.fetch_sub(n, SeqCst);
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn partial_write_timeout_requires_transport_close_before_any_other_message() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    };
    let (client, peer) = tokio::io::duplex(4096);
    let (read_client, write_client) = tokio::io::split(client);
    let (read_peer, write_peer) = tokio::io::split(peer);
    let remaining = Arc::new(AtomicUsize::new(usize::MAX));
    let mut client = StdioClient::new(
        read_client,
        LimitedWriter {
            writer: write_client,
            remaining: remaining.clone(),
        },
    );
    let server = tokio::spawn(async move {
        let mut peer = (BufReader::new(read_peer), write_peer);
        initialize_peer(&mut peer).await;
        std::future::pending::<()>().await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    remaining.store(32, SeqCst);
    assert!(matches!(
        client
            .call_tool("audit", json!({}), Duration::from_millis(10))
            .await,
        Err(McpError::Timeout)
    ));
    assert!(client.unresolved());
    assert!(matches!(
        client.cancel_pending(TIMEOUT).await,
        Err(McpError::Protocol(_))
    ));
    assert!(matches!(
        client.call_tool("another", json!({}), TIMEOUT).await,
        Err(McpError::Protocol(_))
    ));
    assert_eq!(remaining.load(SeqCst), 0);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn unsupported_version_or_invalid_initialization_cannot_enter_ready_state() {
    for result in [
        json!({"protocolVersion":"unsupported","capabilities":{},"serverInfo":{"name":"test","version":"1"}}),
        json!({"protocolVersion":"2025-11-25","capabilities":{"tools":true},"serverInfo":{"name":"test","version":"1"}}),
        json!({"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{}}),
    ] {
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            let request = read(&mut peer).await;
            send(
                &mut peer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
            )
            .await;
        });
        assert!(matches!(
            client.initialize(TIMEOUT).await,
            Err(McpError::Protocol(_))
        ));
        assert!(client.unresolved() && client.protocol_version().is_none());
        assert!(matches!(
            client.call_tool("audit", json!({}), TIMEOUT).await,
            Err(McpError::Protocol(_))
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn timed_out_or_disposed_request_stays_unresolved_and_cannot_launch_another() {
    for dispose in [false, true] {
        let (mut client, mut peer) = pair();
        let (send_started, started) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            initialize_peer(&mut peer).await;
            read(&mut peer).await;
            send_started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        client.initialize(TIMEOUT).await.unwrap();
        if dispose {
            let mut call = Box::pin(client.call_tool("audit", json!({}), TIMEOUT));
            tokio::select! { result = &mut call => panic!("early completion {result:?}"), result = started => result.unwrap() }
            drop(call);
        } else {
            assert!(matches!(
                client
                    .call_tool("audit", json!({}), Duration::from_millis(10))
                    .await,
                Err(McpError::Timeout)
            ));
        }
        assert!(client.unresolved());
        assert!(matches!(
            client.call_tool("again", json!({}), TIMEOUT).await,
            Err(McpError::Protocol(_))
        ));
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn malformed_identity_or_response_remains_unresolved_but_remote_error_settles_rpc() {
    for response in [
        json!({"id":"foreign","result":{}}),
        json!({"result":{},"error":{"code":-1,"message":"bad"}}),
        json!({"error":{"code":-1,"message":"private remote diagnostic"}}),
    ] {
        let remote_error = response.get("result").is_none() && response["id"].is_null();
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            initialize_peer(&mut peer).await;
            let pending = read(&mut peer).await;
            let mut response = response;
            response["jsonrpc"] = json!("2.0");
            if response.get("id").is_none() {
                response["id"] = pending["id"].clone();
            }
            send(&mut peer, response).await;
        });
        client.initialize(TIMEOUT).await.unwrap();
        let error = client
            .call_tool("audit", json!({}), TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(client.unresolved(), !remote_error);
        assert!(!error.to_string().contains("private remote diagnostic"));
        server.await.unwrap();
    }
}

fn event() -> HookEvent {
    HookEvent::synthetic(
        "PreToolUse",
        HookLocation {
            directory: std::env::current_dir().unwrap(),
            workspace: None,
        },
        "global".into(),
        "build".into(),
        "default".into(),
        1,
        json!({"tool_name":"write","tool_input":{"count":2,"nested":{"x":true}}}),
    )
    .unwrap()
}

#[test]
fn templates_preserve_types_and_unknown_fields_or_nonobjects_fail() {
    let event = event();
    let input = render_arguments(Some(&json!({"input":"${tool_input}","count":"${tool_input.count}","label":"call:${tool_name}"})), &event).unwrap();
    assert_eq!(input["input"]["nested"]["x"], true);
    assert_eq!(input["count"], 2);
    assert_eq!(input["label"], "call:write");
    for template in [
        json!("${tool_name}"),
        json!({"x":"${missing}"}),
        json!({"x":"prefix ${tool_name"}),
    ] {
        assert!(render_arguments(Some(&template), &event).is_err());
    }
    assert_eq!(render_arguments(None, &event).unwrap(), *event.as_json());
}

#[test]
fn textual_decisions_refuse_failed_ambiguous_missing_or_oversized_content() {
    let event = event();
    let result = json!({"content":[{"type":"text","text":"audit info"},{"type":"text","text":"{\"decision\":\"deny\",\"reason\":\"policy\"}"}]});
    assert_eq!(
        decision(&event, &result).unwrap().decision.decision,
        Some(HookAction::Deny)
    );
    let mut failed = result.clone();
    failed["isError"] = json!(true);
    let mut ambiguous = result.clone();
    ambiguous["content"][0]["text"] = json!("{}");
    for result in [
        failed,
        ambiguous,
        json!({"content":[]}),
        json!({"structuredContent":{"decision":"allow"}}),
        json!({"content":[{"type":"text","text":"x".repeat(1024*1024+1)}]}),
    ] {
        assert!(decision(&event, &result).is_err());
    }
}
