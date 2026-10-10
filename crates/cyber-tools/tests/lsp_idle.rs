use cyber_tools::lsp::{Framed, LspError, StdioClient};
use serde_json::{Value, json};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, DuplexStream, ReadBuf, ReadHalf, WriteHalf, duplex, split},
    sync::oneshot,
};

type Client = StdioClient<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;
type Reader = Framed<ReadHalf<DuplexStream>, tokio::io::Sink>;

fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).unwrap();
    [
        format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes(),
        body,
    ]
    .concat()
}

fn pair(root: &std::path::Path) -> (Client, Reader, WriteHalf<DuplexStream>) {
    let (client, server) = duplex(4096);
    let (reader, writer) = split(client);
    let (server_reader, server_writer) = split(server);
    (
        StdioClient::new(reader, writer, root).unwrap(),
        Framed::new(server_reader, tokio::io::sink()),
        server_writer,
    )
}

async fn initialize(reader: &mut Reader, writer: &mut WriteHalf<DuplexStream>) {
    let request = reader.read().await.unwrap().unwrap();
    writer
        .write_all(&frame(
            &json!({"jsonrpc":"2.0","id":request["id"],"result":{"capabilities":{}}}),
        ))
        .await
        .unwrap();
    assert_eq!(
        reader.read().await.unwrap().unwrap()["method"],
        "initialized"
    );
}

#[tokio::test]
async fn cancelling_idle_wait_does_not_interrupt_a_partial_header_or_body() {
    for split_at in [10, 27] {
        let root = tempfile::tempdir().unwrap();
        let (mut client, mut reader, mut writer) = pair(root.path());
        let message = json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///untrusted","version":7,"diagnostics":[]}});
        let bytes = frame(&message);
        let (sent, prefix_sent) = oneshot::channel();
        let (resume, resumed) = oneshot::channel();
        let fixture = tokio::spawn(async move {
            initialize(&mut reader, &mut writer).await;
            writer.write_all(&bytes[..split_at]).await.unwrap();
            sent.send(()).unwrap();
            resumed.await.unwrap();
            writer.write_all(&bytes[split_at..]).await.unwrap();
            let request = reader.read().await.unwrap().unwrap();
            writer
                .write_all(&frame(
                    &json!({"jsonrpc":"2.0","id":request["id"],"result":true}),
                ))
                .await
                .unwrap();
        });
        client
            .initialize(Value::Null, Duration::from_secs(1))
            .await
            .unwrap();
        prefix_sent.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), client.next_idle())
                .await
                .is_err()
        );
        resume.send(()).unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), client.next_idle())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, message);
        client.handle_idle(received).await.unwrap();
        assert_eq!(client.take_notifications().len(), 1);
        assert_eq!(
            client
                .request("fixture", Value::Null, Duration::from_secs(1))
                .await
                .unwrap(),
            true
        );
        fixture.await.unwrap();
    }
}

#[tokio::test]
async fn idle_server_requests_are_answered_without_a_client_rpc() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut reader, mut writer) = pair(root.path());
    let fixture = tokio::spawn(async move {
        initialize(&mut reader, &mut writer).await;
        writer.write_all(&frame(&json!({"jsonrpc":"2.0","id":"server","method":"workspace/applyEdit","params":{"edit":{}}}))).await.unwrap();
        let reply = reader.read().await.unwrap().unwrap();
        assert_eq!(reply["id"], "server");
        assert_eq!(reply["error"]["code"], -32601);
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    let message = client.next_idle().await.unwrap();
    client.handle_idle(message).await.unwrap();
    fixture.await.unwrap();
}

#[tokio::test]
async fn unsolicited_responses_and_malformed_idle_envelopes_fence_protocol_reuse() {
    for message in [
        json!({"jsonrpc":"2.0","id":99,"result":true}),
        json!({"jsonrpc":"2.0","method":"fixture","result":true}),
        json!({"jsonrpc":"2.0","method":7}),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut client, mut reader, mut writer) = pair(root.path());
        let fixture = tokio::spawn(async move {
            initialize(&mut reader, &mut writer).await;
            writer.write_all(&frame(&message)).await.unwrap();
        });
        client
            .initialize(Value::Null, Duration::from_secs(1))
            .await
            .unwrap();
        let message = client.next_idle().await.unwrap();
        assert!(client.handle_idle(message).await.is_err());
        assert!(
            client
                .request("fixture", Value::Null, Duration::from_secs(1))
                .await
                .is_err()
        );
        fixture.await.unwrap();
    }
}

#[tokio::test]
async fn truncated_idle_frame_is_reported_without_a_foreground_request() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut reader, mut writer) = pair(root.path());
    let fixture = tokio::spawn(async move {
        initialize(&mut reader, &mut writer).await;
        writer
            .write_all(b"Content-Length: 100\r\n\r\n{")
            .await
            .unwrap();
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(matches!(
        client.next_idle().await,
        Err(LspError::Transport(_))
    ));
    assert!(client.notify("fixture", Value::Null).await.is_err());
    fixture.await.unwrap();
}

struct Tracked<R> {
    reader: R,
    released: Arc<AtomicBool>,
}
impl<R: AsyncRead + Unpin> AsyncRead for Tracked<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}
impl<R> Drop for Tracked<R> {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_client_aborts_the_retained_reader_and_releases_its_stream() {
    let root = tempfile::tempdir().unwrap();
    let (client, server) = duplex(4096);
    let (reader, writer) = split(client);
    let released = Arc::new(AtomicBool::new(false));
    let mut client = StdioClient::new(
        Tracked {
            reader,
            released: released.clone(),
        },
        writer,
        root.path(),
    )
    .unwrap();
    let (reader, mut writer) = split(server);
    let mut reader = Framed::new(reader, tokio::io::sink());
    let fixture = tokio::spawn(async move {
        initialize(&mut reader, &mut writer).await;
        assert!(reader.read().await.unwrap().is_none());
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!released.load(Ordering::SeqCst));
    drop(client);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !released.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.await.unwrap();
}

#[tokio::test]
async fn bounded_inbox_backpressures_the_peer_and_drop_releases_a_blocked_reader() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut reader, mut writer) = pair(root.path());
    let sent = Arc::new(AtomicUsize::new(0));
    let fixture = {
        let sent = sent.clone();
        tokio::spawn(async move {
            initialize(&mut reader, &mut writer).await;
            let bytes = frame(
                &json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"x".repeat(1024)}}),
            );
            for _ in 0..100 {
                if writer.write_all(&bytes).await.is_err() {
                    return;
                }
                sent.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while sent.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Four queued frames plus bounded reader/wire buffers cannot drain the 100-frame stream.
    assert!(sent.load(Ordering::SeqCst) < 32);
    assert!(!fixture.is_finished());
    drop(client);
    tokio::time::timeout(Duration::from_secs(1), fixture)
        .await
        .unwrap()
        .unwrap();
}
