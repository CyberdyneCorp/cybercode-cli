//! The client side of the direct peer transport (`cross-session-messaging` → Direct peer
//! transport): requests to another `cyber` server's peer listener.
//!
//! TLS accepts exactly the pinned key: a custom verifier compares the SHA-256 of the
//! presented certificate's SubjectPublicKeyInfo with the remote's `fingerprint` and ignores
//! system roots, host names and validity dates, so IP changes and DHCP renames keep a
//! pairing and a LAN attacker cannot present any other certificate. The handshake still
//! proves possession of that key (the TLS 1.3 signature is checked against it), and a
//! mismatch fails the handshake before any request byte, the token included, is sent.
//! Plain `http` is allowed only to loopback (an SSH-forwarded port), as config validation
//! requires. Redirects are never followed and proxies never used, so the token goes only
//! to the pinned peer. Connect times out after 5 s and a whole request after 10 s.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::Deserialize;
use serde_json::Value;

use super::{PeerAuthReason, PeerError};
use crate::http::peer::tls::certificate_fingerprint;

/// Connect timeout of every peer request.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Total timeout of every peer request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A configured remote this machine sends to.
#[derive(Clone)]
pub struct PeerTarget {
    pub name: String,
    pub url: String,
    pub fingerprint: String,
    pub token: String,
}

impl std::fmt::Debug for PeerTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerTarget")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("fingerprint", &self.fingerprint)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// `GET /api/v1/peer/hello` as the peer answered it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteHello {
    pub machine: String,
    pub version: String,
    pub fingerprint: String,
    /// Scopes this version may not know are kept as text.
    pub scopes: Vec<String>,
    /// This machine's name on the peer.
    pub grant: String,
}

pub struct PeerClient {
    target: PeerTarget,
    http: reqwest::Client,
    /// Set by the verifier when the peer presented another key.
    mismatch: Arc<AtomicBool>,
}

impl std::fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerClient")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl PeerClient {
    pub fn new(target: PeerTarget) -> Result<Self, PeerError> {
        Self::build(target, Some(REQUEST_TIMEOUT))
    }

    /// A client without the total request timeout, for an attach: its approval waits up to
    /// 2 minutes and its event stream stays open. Connect still times out after 5 s.
    pub fn long_lived(target: PeerTarget) -> Result<Self, PeerError> {
        Self::build(target, None)
    }

    fn build(target: PeerTarget, timeout: Option<Duration>) -> Result<Self, PeerError> {
        let mismatch = Arc::new(AtomicBool::new(false));
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT);
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        let builder = if target.url.starts_with("https://") {
            let verifier = PinnedKey::new(&target.fingerprint, Arc::clone(&mismatch));
            builder.tls_backend_preconfigured(client_config(verifier).map_err(|cause| {
                PeerError::Unreachable {
                    peer: target.name.clone(),
                    cause,
                }
            })?)
        } else {
            builder
        };
        let http = builder.build().map_err(|error| PeerError::Unreachable {
            peer: target.name.clone(),
            cause: format!("cannot build the peer client: {error}"),
        })?;
        Ok(Self {
            target,
            http,
            mismatch,
        })
    }

    pub fn target(&self) -> &PeerTarget {
        &self.target
    }

    /// `GET /api/v1/peer/hello` and its round-trip time.
    pub async fn hello(&self) -> Result<(RemoteHello, Duration), PeerError> {
        let started = Instant::now();
        let (status, body) = self.request(reqwest::Method::GET, "/hello", None).await?;
        let elapsed = started.elapsed();
        if status != 200 {
            return Err(self.unreachable(format!("hello answered HTTP {status}")));
        }
        let hello = serde_json::from_value(body)
            .map_err(|_| self.unreachable("hello answered an unexpected body".into()))?;
        Ok((hello, elapsed))
    }

    /// One request to `/api/v1/peer<path>`: the status and JSON body of an answer the
    /// caller handles, or the [`PeerError`] of a failure (network, TLS, the pin, `401`,
    /// `403 PeerScopeError`, `429` and `5xx`).
    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), PeerError> {
        let response = self.send(method, path, body, &[]).await?;
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        self.classify(status, &body)?;
        Ok((status, body))
    }

    /// One request to `/api/v1/peer<path>` with extra `headers`, its response unread and
    /// unclassified (an attach's event stream); only a failure to get an answer (network,
    /// TLS, the pin) is an error. [`PeerClient::classify`] judges the answer.
    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response, PeerError> {
        let url = format!(
            "{}/api/v1/peer{path}",
            self.target.url.trim_end_matches('/')
        );
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&self.target.token);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|error| self.failure(&error))
    }

    /// The [`PeerError`] of an answer that is the transport's failure (`401`, a `403` other
    /// than a refused permission, `429`, `5xx` other than disabled messaging); `Ok` for an
    /// answer the caller handles.
    pub fn classify(&self, status: u16, body: &Value) -> Result<(), PeerError> {
        match status {
            401 => Err(self.auth(PeerAuthReason::Unauthorized)),
            403 if body["_tag"] == "PeerScopeError" => Err(self.auth(PeerAuthReason::Scope)),
            // The user on the peer refused (`remote.attach`): the caller reports it.
            403 if body["_tag"] == "PermissionDeniedError" => Ok(()),
            403 => Err(self.auth(PeerAuthReason::Unauthorized)),
            429 => Err(self.unreachable("the peer is rate limiting this machine".into())),
            // The peer answered: messaging is disabled there. The caller reports it.
            503 if body["_tag"] == "ServiceUnavailableError" => Ok(()),
            500.. => Err(self.unreachable(format!("the peer answered HTTP {status}"))),
            _ => Ok(()),
        }
    }

    fn failure(&self, error: &reqwest::Error) -> PeerError {
        if self.mismatch.load(Ordering::SeqCst) {
            return self.auth(PeerAuthReason::FingerprintMismatch);
        }
        let cause = if error.is_timeout() {
            "timed out".to_string()
        } else if error.is_connect() {
            "connection failed".to_string()
        } else {
            "the request failed".to_string()
        };
        self.unreachable(cause)
    }

    fn auth(&self, reason: PeerAuthReason) -> PeerError {
        PeerError::Auth {
            peer: self.target.name.clone(),
            reason,
        }
    }

    fn unreachable(&self, cause: String) -> PeerError {
        PeerError::Unreachable {
            peer: self.target.name.clone(),
            cause,
        }
    }
}

fn client_config(verifier: PinnedKey) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| format!("cannot configure peer TLS: {error}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Accepts exactly the certificate whose SPKI hash is the pinned fingerprint.
#[derive(Debug)]
struct PinnedKey {
    fingerprint: String,
    mismatch: Arc<AtomicBool>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedKey {
    fn new(fingerprint: &str, mismatch: Arc<AtomicBool>) -> Self {
        Self {
            fingerprint: fingerprint.to_string(),
            mismatch,
            algorithms: rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for PinnedKey {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = certificate_fingerprint(end_entity).unwrap_or_default();
        if constant_eq(presented.as_bytes(), self.fingerprint.as_bytes()) {
            return Ok(ServerCertVerified::assertion());
        }
        self.mismatch.store(true, Ordering::SeqCst);
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::ApplicationVerificationFailure,
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn client() -> PeerClient {
        PeerClient::new(PeerTarget {
            name: "desk".into(),
            url: "http://127.0.0.1:9".into(),
            fingerprint: format!("sha256:{}", "0".repeat(64)),
            token: "cyp_pgrx_s".into(),
        })
        .unwrap()
    }

    /// A refused `remote.attach` is the peer user's answer, not a broken pairing; every
    /// other `403` still is one.
    #[test]
    fn a_refused_attach_is_not_an_auth_failure() {
        let client = client();
        let denied = json!({"_tag": "PermissionDeniedError", "message": "refused"});
        assert!(client.classify(403, &denied).is_ok());
        let scope = json!({"_tag": "PeerScopeError", "scope": "attach"});
        assert!(matches!(
            client.classify(403, &scope),
            Err(PeerError::Auth {
                reason: PeerAuthReason::Scope,
                ..
            })
        ));
        assert!(matches!(
            client.classify(403, &json!({"_tag": "ForbiddenError"})),
            Err(PeerError::Auth {
                reason: PeerAuthReason::Unauthorized,
                ..
            })
        ));
        assert!(client.classify(404, &json!({})).is_ok());
        // Regression: messaging disabled on the peer was reported as an unreachable peer.
        let disabled = json!({"_tag": "ServiceUnavailableError", "message": "disabled"});
        assert!(client.classify(503, &disabled).is_ok());
        assert!(matches!(
            client.classify(503, &json!({"_tag": "UnknownError"})),
            Err(PeerError::Unreachable { .. })
        ));
        assert!(PeerClient::long_lived(client.target().clone()).is_ok());
    }
}
