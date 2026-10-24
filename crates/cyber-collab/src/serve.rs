//! The standalone hub listener. A normal local owner API is never exposed here.
use crate::{auth::BackendCredentials, error::Error};
use axum::{
    Router,
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub struct TlsOptions {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

/// Operator-owned configuration. No cloned repository supplies these values.
pub struct ServeConfig {
    pub issuer: String,
    pub audience: String,
    pub bootstrap_sub: String,
    pub database: PathBuf,
    pub listen: SocketAddr,
    pub introspection: Option<BackendCredentials>,
    pub allow_guests: bool,
    pub require_mfa: bool,
    pub enabled: bool,
    pub tls: Option<TlsOptions>,
    /// The one TLS-terminating proxy permitted to contact a plain upstream.
    pub trusted_proxy: Option<IpAddr>,
    /// Development-only identity provider HTTP; discovery still restricts literal loopback.
    pub allow_loopback_http: bool,
}
impl ServeConfig {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        bootstrap_sub: impl Into<String>,
        database: impl Into<PathBuf>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            bootstrap_sub: bootstrap_sub.into(),
            database: database.into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 4749)),
            introspection: None,
            allow_guests: false,
            require_mfa: true,
            enabled: false,
            tls: None,
            trusted_proxy: None,
            allow_loopback_http: false,
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        if !self.enabled {
            return Err(Error::new(
                "FeatureDisabledError",
                "explicitly enable features.collaboration in user configuration",
            ));
        }
        if self.issuer.is_empty()
            || self.audience.trim().is_empty()
            || self.bootstrap_sub.trim().is_empty()
        {
            return Err(Error::invalid(
                "issuer, audience and verified bootstrap subject are required",
            ));
        }
        if self.introspection.is_none() {
            return Err(Error::invalid(
                "backend introspection credentials are required",
            ));
        }
        if self.tls.is_some() && self.trusted_proxy.is_some() {
            return Err(Error::invalid(
                "choose direct TLS or one explicit TLS-terminating proxy",
            ));
        }
        if !self.listen.ip().is_loopback() && self.tls.is_none() && self.trusted_proxy.is_none() {
            return Err(Error::invalid(
                "non-loopback hub listeners require TLS or an exact trusted TLS proxy",
            ));
        }
        if self
            .trusted_proxy
            .is_some_and(|ip| ip.is_unspecified() || ip.is_multicast())
        {
            return Err(Error::invalid(
                "trusted proxy must be one exact unicast IP address",
            ));
        }
        Ok(())
    }
}

/// Run until shutdown; all accepted connection tasks are owned and joined or aborted.
pub async fn serve(
    config: ServeConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), Error> {
    config.validate()?;
    let tls = config.tls.as_ref().map(tls_config).transpose()?;
    let state = crate::server::Hub::new(&config).await?;
    let router = crate::server::router(state);
    let listener = TcpListener::bind(config.listen).await.map_err(|_| {
        Error::new(
            "HubListenError",
            "could not bind the configured hub listener",
        )
    })?;
    serve_listener(router, listener, tls, config.trusted_proxy, shutdown).await
}

fn tls_config(options: &TlsOptions) -> Result<Arc<rustls::ServerConfig>, Error> {
    let certificates = CertificateDer::pem_file_iter(&options.certificate)
        .map_err(|_| Error::invalid("could not read TLS certificate"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::invalid("invalid PEM TLS certificate"))?;
    let key = PrivateKeyDer::from_pem_file(&options.private_key)
        .map_err(|_| Error::invalid("could not read PEM TLS private key"))?;
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| Error::invalid("could not configure hub TLS"))?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|_| Error::invalid("TLS certificate and private key do not match"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn proxy_allowed(peer: SocketAddr, headers: &HeaderMap, proxy: IpAddr) -> bool {
    peer.ip() == proxy
        && headers.get_all("x-forwarded-proto").iter().count() == 1
        && headers
            .get("x-forwarded-proto")
            .is_some_and(|v| v.as_bytes() == b"https")
}

async fn request_deadline(request: Request, next: Next) -> Response {
    // Also bounds a client that stalls while sending a small request body.
    match tokio::time::timeout(Duration::from_secs(30), next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

async fn serve_listener(
    router: Router,
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    proxy: Option<IpAddr>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), Error> {
    let router = router.layer(middleware::from_fn(request_deadline));
    let slots = Arc::new(Semaphore::new(128));
    let mut connections = JoinSet::new();
    let closing = CancellationToken::new();
    tokio::pin!(shutdown);
    let acceptor = tls.map(tokio_rustls::TlsAcceptor::from);
    loop {
        tokio::select! {
            biased;
            _=&mut shutdown=>break,
            Some(_)=connections.join_next(), if !connections.is_empty()=>{},
            accepted=listener.accept()=>{
                let (stream,peer)=accepted.map_err(|_|Error::new("HubListenError","hub listener failed"))?;
                let Ok(slot)=slots.clone().try_acquire_owned() else {drop(stream);continue;};
                if proxy.is_some_and(|ip|peer.ip()!=ip){drop(stream);continue;}
                let router=if let Some(proxy)=proxy {
                    router.clone().layer(middleware::from_fn(move|request:Request,next:Next|async move{
                        if !proxy_allowed(peer,request.headers(),proxy){return StatusCode::FORBIDDEN.into_response();}
                        next.run(request).await
                    }))
                }else{router.clone()};
                let router=router.layer(axum::Extension(ConnectInfo(peer)));
                let acceptor=acceptor.clone();let closing=closing.clone();
                connections.spawn(async move {
                    let _slot=slot;
                    if let Some(acceptor)=acceptor {
                        if let Ok(Ok(tls))=tokio::time::timeout(Duration::from_secs(10),acceptor.accept(stream)).await {connection(router,tls,closing).await;}
                    }else{connection(router,stream,closing).await;}
                });
            }
        }
    }
    closing.cancel();
    if tokio::time::timeout(Duration::from_secs(3), async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    Ok(())
}

async fn connection(
    router: Router,
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    closing: CancellationToken,
) {
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(10));
    let connection =
        builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(router));
    tokio::pin!(connection);
    tokio::select! {
        _=&mut connection=>{},
        _=closing.cancelled()=>{
            connection.as_mut().graceful_shutdown();
            let _=tokio::time::timeout(Duration::from_secs(2),connection).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_account::Secret;
    use tokio::net::TcpStream;
    fn config() -> ServeConfig {
        let mut config = ServeConfig::new("https://auth.example", "hub", "alice", "hub.db");
        config.enabled = true;
        config.introspection = Some(BackendCredentials::Token(Secret::new(
            "backend-only".into(),
        )));
        config
    }
    #[test]
    fn nonloopback_plain_upstream_needs_an_explicit_single_tls_proxy() {
        let mut config = config();
        assert!(config.validate().is_ok());
        config.listen = "0.0.0.0:4749".parse().unwrap();
        assert!(config.validate().is_err());
        config.trusted_proxy = Some("10.0.0.8".parse().unwrap());
        assert!(config.validate().is_ok());
        config.trusted_proxy = Some("0.0.0.0".parse().unwrap());
        assert!(config.validate().is_err());
        config.enabled = false;
        assert!(config.validate().is_err());
    }
    #[test]
    fn forwarded_proto_is_accepted_only_from_the_exact_proxy_and_single_https_value() {
        let peer = "10.0.0.8:44123".parse().unwrap();
        let proxy = "10.0.0.8".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(proxy_allowed(peer, &headers, proxy));
        assert!(!proxy_allowed(
            "10.0.0.9:44123".parse().unwrap(),
            &headers,
            proxy
        ));
        headers.insert("x-forwarded-proto", "https,http".parse().unwrap());
        assert!(!proxy_allowed(peer, &headers, proxy));
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        headers.append("x-forwarded-proto", "https".parse().unwrap());
        assert!(!proxy_allowed(peer, &headers, proxy));
    }
    #[tokio::test]
    async fn native_tls_serves_a_verified_certificate_and_rejects_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let certificate = rcgen::CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let options = TlsOptions {
            certificate: directory.path().join("certificate.pem"),
            private_key: directory.path().join("key.pem"),
        };
        std::fs::write(&options.certificate, certificate.pem()).unwrap();
        std::fs::write(&options.private_key, key.serialize_pem()).unwrap();
        let tls = tls_config(&options).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(serve_listener(
            Router::new().route("/ready", axum::routing::get(|| async { "ready" })),
            listener,
            Some(tls),
            None,
            async move { shutdown.cancelled().await },
        ));
        let client = reqwest::Client::builder()
            .add_root_certificate(
                reqwest::Certificate::from_pem(certificate.pem().as_bytes()).unwrap(),
            )
            .build()
            .unwrap();
        assert_eq!(
            client
                .get(format!("https://localhost:{}/ready", address.port()))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ready"
        );
        assert!(
            client
                .get(format!("http://{address}/ready"))
                .send()
                .await
                .is_err()
        );
        stop.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn actual_upstream_rejects_spoofed_proxy_headers_and_stops_with_idle_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let task = tokio::spawn(serve_listener(
            Router::new().route("/ready", axum::routing::get(|| async { "ready" })),
            listener,
            None,
            Some(address.ip()),
            async move { stop.cancelled().await },
        ));
        let client = reqwest::Client::new();
        let url = format!("http://{address}/ready");
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .get(&url)
                .header("x-forwarded-proto", "https")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ready"
        );
        assert_eq!(
            client
                .get(&url)
                .header("x-forwarded-proto", "https,http")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let _idle = TcpStream::connect(address).await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
