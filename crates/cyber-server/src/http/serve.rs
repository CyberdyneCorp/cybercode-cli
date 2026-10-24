//! Transports: TCP, the Unix socket, the peer listener and the in-process embedded client
//! (`server-api` → Listener defaults, Embedded transport, Peer listener).

use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use axum::{Extension, Router};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::Transport;

/// The remote address of a TCP connection, attached to each of its requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// Serve the router on a bound TCP listener until `shutdown` resolves.
pub async fn serve_tcp(
    router: Router,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_owned(
        router.layer(Extension(Transport::Tcp)),
        listener,
        |addr| Some(*addr),
        plain,
        None,
        true,
        shutdown,
    )
    .await;
    Ok(())
}

/// Load an operator-provided certificate chain and matching private key. No generated
/// certificate or disabled certificate verification is used for the owner listener.
pub fn owner_tls(
    cert: &std::path::Path,
    key: &std::path::Path,
) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certificates = CertificateDer::pem_file_iter(cert)
        .map_err(|e| format!("cannot read server.tls.cert: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid server.tls.cert: {e}"))?;
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("cannot read server.tls.key: {e}"))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|e| format!("invalid server.tls certificate/key: {e}"))?;
    Ok(Arc::new(config))
}

/// Serve the owner API and dashboard using the same authenticated router over TLS.
pub async fn serve_tcp_tls(
    router: Router,
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_tls_with(
        router.layer(Extension(Transport::Tcp)),
        listener,
        tls,
        PeerLimits::default(),
        true,
        shutdown,
    )
    .await
}

/// How long a peer has to finish the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the peer listener allows each connection, which any host that can reach it may
/// open before authenticating: so many at once, and so long to send a request's headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerLimits {
    /// Connections served at once; one more is closed as soon as it is accepted.
    pub connections: usize,
    /// How long a connection may take to send a request's headers, which is also how
    /// long it may stay idle before or between requests.
    pub header_timeout: Duration,
}

impl Default for PeerLimits {
    fn default() -> Self {
        Self {
            connections: 64,
            header_timeout: Duration::from_secs(10),
        }
    }
}

/// [`serve_peer_with`] with the default [`PeerLimits`].
pub async fn serve_peer(
    router: Router,
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_peer_with(router, listener, tls, PeerLimits::default(), shutdown).await
}

/// Serve the peer router ([`super::peer::router`]) on a bound TCP listener until
/// `shutdown` resolves: TLS with `tls`, plain HTTP only when the listener is on loopback
/// (an SSH-forwarded port). A plain listener on any other address is refused. Only
/// HTTP/1.1 is served, within `limits`, so connections that never send a request (or
/// stall) are closed and cannot exhaust the process's file descriptors.
pub async fn serve_peer_with(
    router: Router,
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: PeerLimits,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_tls_with(router, listener, tls, limits, false, shutdown).await
}
async fn serve_tls_with(
    router: Router,
    listener: tokio::net::TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: PeerLimits,
    upgrades: bool,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let Some(config) = tls else {
        let address = listener.local_addr()?;
        if !super::guard::loopback_peer(address.ip()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("a plain peer listener is allowed only on loopback, not {address}"),
            ));
        }
        let peer = |addr: &SocketAddr| Some(*addr);
        serve_owned(
            router,
            listener,
            peer,
            plain,
            Some(limits),
            upgrades,
            shutdown,
        )
        .await;
        return Ok(());
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let handshake = move |io: tokio::net::TcpStream| {
        let accept = acceptor.accept(io);
        async move {
            tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
                .await
                .ok()?
                .ok()
        }
    };
    let peer = |addr: &SocketAddr| Some(*addr);
    serve_owned(
        router,
        listener,
        peer,
        handshake,
        Some(limits),
        upgrades,
        shutdown,
    )
    .await;
    Ok(())
}

/// The connection as accepted, for listeners without a handshake.
fn plain<Io>(io: Io) -> std::future::Ready<Option<Io>> {
    std::future::ready(Some(io))
}

/// Own connection tasks so a blocked socket cannot prevent listener shutdown indefinitely.
/// `upgrade` runs in the connection's task (a TLS handshake), so a slow client does not
/// hold up `accept`; a connection it rejects is closed. With `limits`, a connection past
/// the cap is closed on accept, and each one is held to the header timeout.
async fn serve_owned<L, U, F, Io>(
    router: Router,
    mut listener: L,
    peer: fn(&L::Addr) -> Option<SocketAddr>,
    upgrade: U,
    limits: Option<PeerLimits>,
    upgrades: bool,
    shutdown: impl Future<Output = ()> + Send + 'static,
) where
    L: axum::serve::Listener,
    U: Fn(L::Io) -> F,
    F: Future<Output = Option<Io>> + Send + 'static,
    Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let closing = CancellationToken::new();
    let _on_drop = closing.clone().drop_guard();
    let mut connections = tokio::task::JoinSet::new();
    let slots = limits.map(|limits| Arc::new(Semaphore::new(limits.connections)));
    let header_timeout = limits.map(|limits| limits.header_timeout);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            (io, addr) = listener.accept() => {
                let Ok(slot) = take_slot(slots.as_ref()) else {
                    // Over the cap: closed before any byte is read.
                    drop(io);
                    continue;
                };
                let connection = Connection {
                    router: router.clone(),
                    peer: peer(&addr),
                    closing: closing.clone(),
                    header_timeout,
                    upgrades,
                    _slot: slot,
                };
                connections.spawn(serve_upgraded(connection, upgrade(io)));
            }
        }
    }
    drop(listener);
    closing.cancel();
    while connections.join_next().await.is_some() {}
}

/// A free connection slot, when connections are capped; `Err` when none is free.
fn take_slot(slots: Option<&Arc<Semaphore>>) -> Result<Option<OwnedSemaphorePermit>, ()> {
    match slots {
        Some(slots) => Arc::clone(slots)
            .try_acquire_owned()
            .map(Some)
            .map_err(|_| ()),
        None => Ok(None),
    }
}

/// One accepted connection and what serves it.
struct Connection {
    router: Router,
    peer: Option<SocketAddr>,
    closing: CancellationToken,
    /// HTTP/1.1 only, with this header (and idle) timeout; `None` serves HTTP/1.1 and
    /// HTTP/2 without one (the main API on loopback and the Unix socket).
    header_timeout: Option<Duration>,
    upgrades: bool,
    /// The connection's place under the listener's cap, freed when it closes.
    _slot: Option<OwnedSemaphorePermit>,
}

async fn serve_upgraded<Io>(connection: Connection, handshake: impl Future<Output = Option<Io>>)
where
    Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let io = tokio::select! {
        biased;
        _ = connection.closing.cancelled() => return,
        io = handshake => io,
    };
    if let Some(io) = io {
        serve_connection(connection, io).await;
    }
}

/// The HTTP connection builder: HTTP/1.1 and HTTP/2, or with a header timeout, HTTP/1.1
/// only (hyper has no idle timeout for HTTP/2), whose header timer also closes a
/// connection idle before or between requests.
fn builder(header_timeout: Option<Duration>) -> Builder<TokioExecutor> {
    let Some(timeout) = header_timeout else {
        let mut builder = Builder::new(TokioExecutor::new());
        builder.http2().enable_connect_protocol();
        return builder;
    };
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(timeout);
    builder
}

async fn serve_connection(
    connection: Connection,
    io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
) {
    let Connection {
        router,
        peer,
        closing,
        header_timeout,
        upgrades,
        _slot,
    } = connection;
    let builder = builder(header_timeout);
    let service = router.map_request(move |mut req: Request<_>| {
        if let Some(peer) = peer {
            req.extensions_mut().insert(PeerAddr(peer));
        }
        req
    });
    let (io, service) = (
        TokioIo::new(ConnectionIo { inner: io, _slot }),
        TowerToHyperService::new(service),
    );
    if let Some(timeout) = header_timeout.filter(|_| upgrades) {
        // Owner HTTP/1 supports upgrades without the auto builder's protocol
        // sniffing gap before the first header deadline starts.
        let mut http1 = hyper::server::conn::http1::Builder::new();
        http1.timer(TokioTimer::new()).header_read_timeout(timeout);
        let connection = http1.serve_connection(io, service).with_upgrades();
        drive(connection, closing, |c| c.graceful_shutdown()).await;
    } else if header_timeout.is_some() {
        // HTTP/1.1 from the first byte, so the header timer also runs before any byte
        // arrives (the version sniffing of upgradeable connections waits without one);
        // the peer routes need no upgrades.
        let connection = builder.serve_connection(io, service);
        drive(connection, closing, |c| c.graceful_shutdown()).await;
    } else {
        let connection = builder.serve_connection_with_upgrades(io, service);
        drive(connection, closing, |c| c.graceful_shutdown()).await;
    }
}

/// A slot follows upgraded IO too, so an authenticated WebSocket cannot silently
/// leave the listener's connection limit when its HTTP handshake completes.
struct ConnectionIo<Io> {
    inner: Io,
    _slot: Option<OwnedSemaphorePermit>,
}
impl<Io: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ConnectionIo<Io> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl<Io: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for ConnectionIo<Io> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

/// Run `connection` until it ends or `closing` fires, then shut it down gracefully for
/// up to 2 seconds.
async fn drive<C: Future>(connection: C, closing: CancellationToken, shutdown: fn(Pin<&mut C>)) {
    tokio::pin!(connection);
    tokio::select! {
        biased;
        _ = closing.cancelled() => {},
        _ = &mut connection => return,
    }
    shutdown(connection.as_mut());
    // On timeout the owned connection future and its IO are dropped before this task ends.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), connection).await;
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
    serve_owned(
        router.layer(Extension(Transport::Unix)),
        checked,
        |_| None,
        plain,
        None,
        true,
        shutdown,
    )
    .await;
    Ok(())
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
