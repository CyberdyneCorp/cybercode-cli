//! Owner TLS wrapping must preserve existing TUI/SDK WebSocket transports.
use axum::{Router, extract::ws::WebSocketUpgrade, response::Response, routing::get};
use cyber_server::http::{owner_tls, serve_tcp_tls};
use futures::{SinkExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio_tungstenite::{client_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

async fn echo(socket: WebSocketUpgrade) -> Response {
    socket.on_upgrade(|mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            if socket.send(message).await.is_err() {
                break;
            }
        }
    })
}
async fn listener(
    tls: Option<Arc<rustls::ServerConfig>>,
) -> (
    std::net::SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let closing = stop.clone();
    let task = tokio::spawn(serve_tcp_tls(
        Router::new().route("/ws", get(echo)),
        listener,
        tls,
        async move { closing.cancelled().await },
    ));
    (address, stop, task)
}
async fn roundtrip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: S, url: &str) {
    let (mut socket, response) = client_async(url, socket).await.unwrap();
    assert_eq!(response.status(), 101);
    socket
        .send(Message::Text("owner transport".into()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Text("owner transport".into())
    );
    socket.close(None).await.unwrap();
}
#[tokio::test]
async fn owner_plain_wrapper_preserves_websocket_upgrades() {
    let (address, stop, task) = listener(None).await;
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    roundtrip(stream, &format!("ws://{address}/ws")).await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(4), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn owner_tls_wrapper_preserves_verified_websocket_upgrades() {
    let fixture = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cert = directory.path().join("cert.pem");
    let key = directory.path().join("key.pem");
    std::fs::write(&cert, fixture.cert.pem()).unwrap();
    std::fs::write(&key, fixture.signing_key.serialize_pem()).unwrap();
    let (address, stop, task) = listener(Some(owner_tls(&cert, &key).unwrap())).await;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(fixture.cert.der().clone()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect("localhost".try_into().unwrap(), stream)
        .await
        .unwrap();
    roundtrip(stream, &format!("wss://localhost:{}/ws", address.port())).await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(4), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn upgraded_owner_connections_keep_their_bounded_listener_slot() {
    let (address, stop, task) = listener(None).await;
    let mut sockets = Vec::new();
    for _ in 0..64 {
        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let (socket, _) = client_async(format!("ws://{address}/ws"), stream)
            .await
            .unwrap();
        sockets.push(socket);
    }
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            client_async(format!("ws://{address}/ws"), stream)
        )
        .await
        .unwrap()
        .is_err()
    );
    drop(sockets);
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(4), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
