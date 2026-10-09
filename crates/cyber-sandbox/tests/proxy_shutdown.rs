//! Accepted proxy connections remain owned through acknowledged shutdown.
use cyber_sandbox::proxy::{Endpoint, Proxy};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn shutdown_closes_active_tunnels_incomplete_heads_and_listener() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = Proxy::start(Arc::new(|_| Box::pin(async { true })))
        .await
        .unwrap();
    let Endpoint::Tcp(port) = proxy.endpoint else {
        panic!("TCP endpoint");
    };
    let mut tunnel = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tunnel
        .write_all(
            format!(
                "CONNECT {} HTTP/1.1\r\nHost: example\r\n\r\n",
                upstream.local_addr().unwrap()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let (mut remote, _) = upstream.accept().await.unwrap();
    let mut head = [0; 128];
    response_head(&mut tunnel).await;
    let mut incomplete = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    incomplete.write_all(b"GET ").await.unwrap();
    proxy.shutdown().await.unwrap();
    for socket in [&mut tunnel, &mut remote, &mut incomplete] {
        let result = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut head))
            .await
            .unwrap();
        assert!(matches!(result, Ok(0) | Err(_)));
    }
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
}

#[tokio::test]
async fn dropping_proxy_also_disposes_accepted_connections() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = Proxy::start(Arc::new(|_| Box::pin(async { true })))
        .await
        .unwrap();
    let Endpoint::Tcp(port) = proxy.endpoint else {
        panic!("TCP endpoint");
    };
    let mut tunnel = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tunnel
        .write_all(
            format!(
                "CONNECT {} HTTP/1.1\r\n\r\n",
                upstream.local_addr().unwrap()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let (mut remote, _) = upstream.accept().await.unwrap();
    let mut head = [0; 128];
    response_head(&mut tunnel).await;
    drop(proxy);
    let result = tokio::time::timeout(Duration::from_secs(1), remote.read(&mut head))
        .await
        .unwrap();
    assert!(matches!(result, Ok(0) | Err(_)));
}

async fn response_head(socket: &mut TcpStream) {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
            assert!(head.len() < 4096);
        }
    })
    .await
    .unwrap();
}
