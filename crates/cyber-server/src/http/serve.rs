//! Transports: TCP, the Unix socket and the in-process embedded client
//! (`server-api` → Listener defaults, Embedded transport).

use std::path::Path;

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use axum::{Extension, Router};
use tower::ServiceExt;

use super::Transport;

/// Serve the router on a bound TCP listener until `shutdown` resolves.
pub async fn serve_tcp(
    router: Router,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router.layer(Extension(Transport::Tcp)))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Serve on a Unix socket (mode 0600). Only peers running as this OS user are accepted.
#[cfg(unix)]
pub async fn serve_unix(
    router: Router,
    path: &Path,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let _ = std::fs::remove_file(path);
    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let uid = std::fs::metadata(path)?.uid();
    let checked = PeerChecked {
        inner: listener,
        uid,
    };
    axum::serve(checked, router.layer(Extension(Transport::Unix)))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(unix)]
struct PeerChecked {
    inner: tokio::net::UnixListener,
    uid: u32,
}

#[cfg(unix)]
impl axum::serve::Listener for PeerChecked {
    type Io = tokio::net::UnixStream;
    type Addr = tokio::net::unix::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) if stream.peer_cred().is_ok_and(|c| c.uid() == self.uid) => {
                    return (stream, addr);
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Requests through the router with no listener and no authentication, for `--embedded`
/// clients and tests. The base URL is `http://cyber.internal`.
#[derive(Clone)]
pub struct EmbeddedClient {
    router: Router,
}

impl EmbeddedClient {
    pub fn new(router: Router) -> Self {
        Self {
            router: router.layer(Extension(Transport::Embedded)),
        }
    }

    pub async fn request(&self, req: Request<Body>) -> Response {
        match self.router.clone().oneshot(req).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    }
}
