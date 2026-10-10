use cyber_tools::lsp::{Framed, TransportError};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex, empty, sink};

fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).unwrap();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend(body);
    bytes
}

async fn rejected(bytes: &[u8]) -> String {
    let mut transport = Framed::new(bytes, sink());
    let error = transport.read().await.unwrap_err().to_string();
    assert!(matches!(
        transport.read().await,
        Err(TransportError::Unavailable)
    ));
    assert!(matches!(
        transport
            .write(&json!({"jsonrpc":"2.0","method":"exit"}))
            .await,
        Err(TransportError::Unavailable)
    ));
    error
}

#[tokio::test]
async fn utf8_byte_lengths_and_adjacent_messages_roundtrip() {
    let first = json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"text":"λ🦀\nContent-Length: 0"}});
    let second = json!({"jsonrpc":"2.0","id":1,"result":null});
    let (writer, mut wire) = duplex(4096);
    let mut outgoing = Framed::new(empty(), writer);
    outgoing.write(&first).await.unwrap();
    outgoing.write(&second).await.unwrap();
    drop(outgoing);
    let mut bytes = Vec::new();
    wire.read_to_end(&mut bytes).await.unwrap();
    let expected = [frame(&first), frame(&second)].concat();
    assert_eq!(bytes, expected);
    let mut incoming = Framed::new(bytes.as_slice(), sink());
    assert_eq!(incoming.read().await.unwrap(), Some(first));
    assert_eq!(incoming.read().await.unwrap(), Some(second));
    assert!(incoming.read().await.unwrap().is_none());
    assert!(matches!(
        incoming.read().await,
        Err(TransportError::Unavailable)
    ));
}

#[tokio::test]
async fn fragmented_headers_and_bodies_are_reassembled() {
    let message = json!({"jsonrpc":"2.0","id":"fixture","result":{"name":"日本語"}});
    let bytes = frame(&message);
    let (reader, mut writer) = duplex(1);
    let sending = tokio::spawn(async move {
        for byte in bytes {
            writer.write_all(&[byte]).await.unwrap();
        }
    });
    let mut incoming = Framed::new(reader, sink());
    assert_eq!(incoming.read().await.unwrap(), Some(message));
    sending.await.unwrap();
}

#[tokio::test]
async fn content_type_defaults_utf8_alias_and_case_are_supported() {
    let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
    for content_type in [
        "application/vscode-jsonrpc",
        "application/vscode-jsonrpc; charset=utf8",
        "Application/Vscode-Jsonrpc; Charset=\"UTF-8\"",
    ] {
        let bytes = format!(
            "content-length: {}\r\nContent-Type: {content_type}\r\nX-Test: ignored\r\n\r\n",
            body.len()
        )
        .into_bytes();
        let bytes = [bytes, body.to_vec()].concat();
        let mut incoming = Framed::new(bytes.as_slice(), sink());
        assert_eq!(incoming.read().await.unwrap().unwrap()["id"], 1);
    }
}

#[tokio::test]
async fn ambiguous_invalid_and_unbounded_headers_are_refused() {
    for header in [
        "X-Test: missing\r\n\r\n",
        "Content-Length: 2\r\ncontent-length: 2\r\n\r\n",
        "Content-Length: -1\r\n\r\n",
        "Content-Length: +2\r\n\r\n",
        "Content-Length: 0\r\n\r\n",
        "Content-Length: 4194305\r\n\r\n",
        "Content-Length: 999999999999999999999999999999999\r\n\r\n",
        "Content-Length: 2\n\n",
        "Bad Name: a\r\nContent-Length: 2\r\n\r\n",
        "Content-Length: 2\r\nX-Test: é\r\n\r\n",
        "Content-Length: 2\r\nX-Test: a\nb\r\n\r\n",
        "Content-Length: 2\r\nX-Test: \0\r\n\r\n",
    ] {
        rejected(header.as_bytes()).await;
    }
    let header = vec![b'x'; 8192];
    assert!(rejected(&header).await.contains("header limit exceeded"));
}

#[tokio::test]
async fn unsupported_and_ambiguous_content_types_are_refused() {
    for content_type in [
        "text/plain",
        "application/vscode-jsonrpc; charset=latin1",
        "application/vscode-jsonrpc; charset=utf8; charset=utf8",
        "application/vscode-jsonrpc; unknown=value",
    ] {
        let bytes = format!("Content-Length: 2\r\nContent-Type: {content_type}\r\n\r\n{{}}");
        rejected(bytes.as_bytes()).await;
    }
    rejected(b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}").await;
}

#[tokio::test]
async fn truncated_invalid_and_non_object_messages_are_refused_without_echoing_content() {
    rejected(b"Content-Length:").await;
    rejected(b"Content-Length: 20\r\n\r\n{}").await;
    let secret = "fixture-password-not-for-diagnostics";
    let raw = format!("Content-Length: {}\r\n\r\n{secret}", secret.len());
    assert!(!rejected(raw.as_bytes()).await.contains(secret));
    for value in [json!(null), json!([]), json!({"jsonrpc":"1.0"})] {
        rejected(&frame(&value)).await;
    }
    rejected(b"Content-Length: 1\r\n\r\n\xff").await;
}

#[tokio::test]
async fn cancelled_partial_read_fences_both_directions() {
    let (reader, mut writer) = duplex(64);
    writer
        .write_all(b"Content-Length: 30\r\n\r\n{")
        .await
        .unwrap();
    let mut incoming = Framed::new(reader, sink());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), incoming.read())
            .await
            .is_err()
    );
    assert!(matches!(
        incoming.read().await,
        Err(TransportError::Unavailable)
    ));
    assert!(matches!(
        incoming
            .write(&json!({"jsonrpc":"2.0","method":"exit"}))
            .await,
        Err(TransportError::Unavailable)
    ));
}

#[tokio::test]
async fn cancelled_partial_write_cannot_replay_or_resume() {
    let (writer, mut wire) = duplex(16);
    let mut outgoing = Framed::new(empty(), writer);
    let message = json!({"jsonrpc":"2.0","method":"fixture","params":{"text":"x".repeat(1024)}});
    assert!(
        tokio::time::timeout(Duration::from_millis(10), outgoing.write(&message))
            .await
            .is_err()
    );
    let mut prefix = [0; 16];
    assert_eq!(wire.read_exact(&mut prefix).await.unwrap(), 16);
    assert_eq!(&prefix, b"Content-Length: ");
    assert!(matches!(
        outgoing.write(&message).await,
        Err(TransportError::Unavailable)
    ));
    assert!(matches!(
        outgoing.read().await,
        Err(TransportError::Unavailable)
    ));
}

#[tokio::test]
async fn rejected_local_message_sends_nothing_and_keeps_transport_usable() {
    let (writer, mut wire) = duplex(1024);
    let mut outgoing = Framed::new(empty(), writer);
    assert!(outgoing.write(&json!([])).await.is_err());
    let oversized =
        json!({"jsonrpc":"2.0","method":"fixture","params":{"text":"x".repeat(4*1024*1024)}});
    assert!(outgoing.write(&oversized).await.is_err());
    let message = json!({"jsonrpc":"2.0","method":"exit"});
    outgoing.write(&message).await.unwrap();
    drop(outgoing);
    let mut bytes = Vec::new();
    wire.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, frame(&message));
}
