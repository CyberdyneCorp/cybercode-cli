//! Listener ownership under pending HTTP/2 service work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{Router, routing::get};
use cyber_server::http::serve_tcp;
use tokio::sync::{Mutex, Notify, oneshot};

struct RequestDrop(Option<oneshot::Sender<()>>);

impl Drop for RequestDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn shutdown_releases_pending_http2_handler_with_client_still_connected() {
    let entered = Arc::new(Notify::new());
    let (released, release) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(released)));
    let handler_entered = entered.clone();
    let route = Router::new().route(
        "/pending",
        get(move || {
            let entered = handler_entered.clone();
            let sender = sender.clone();
            async move {
                let _owner = RequestDrop(sender.lock().await.take());
                entered.notify_one();
                std::future::pending::<&'static str>().await
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_tcp(route, listener, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let request =
        tokio::spawn(async move { client.get(format!("http://{address}/pending")).send().await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let start = Instant::now();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Do not abort the client request to release server-side resources.
    tokio::time::timeout(Duration::from_secs(1), release)
        .await
        .unwrap()
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    let result = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
}
