//! The authorization-code leg: PKCE S256 and `state` from the OS random source, the
//! authorize URL with the RFC 8707 `resource`, and a one-shot loopback callback listener.
//!
//! The listener binds 127.0.0.1 only (an ephemeral or configured port) and accepts nothing
//! but `GET` of its callback path (`/mcp/oauth/callback` for MCP); other paths get 404 and
//! the wait continues. The first
//! callback ends the wait: its `state` is compared in constant time, and a mismatch, an
//! `error=` response or a missing code fails the flow. `wait` consumes the listener, so
//! once it returns (including on timeout) the socket is closed and no later callback
//! connects; nothing runs in the background.
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use reqwest::Url;
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{AuthorizationServer, OAuthError, sanitized};

pub const CALLBACK_PATH: &str = "/mcp/oauth/callback";
/// Callback requests are tiny; longer request heads are refused.
const REQUEST_LIMIT: usize = 8 * 1024;
/// One callback connection may take this long to send its request head.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A PKCE S256 pair (RFC 7636). `Debug` never shows the verifier.
#[derive(Clone)]
pub struct Pkce {
    /// 43 unreserved characters from 32 random bytes.
    pub verifier: String,
    pub challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl Pkce {
    pub fn generate() -> Result<Self, OAuthError> {
        let verifier = random_token()?;
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
        })
    }
}

/// An unguessable `state` value.
pub fn random_state() -> Result<String, OAuthError> {
    random_token()
}

fn random_token() -> Result<String, OAuthError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OAuthError::Random)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// The authorization request URL; the endpoint's own query parameters are kept.
pub fn authorize_url(
    server: &AuthorizationServer,
    client_id: &str,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
    scope: Option<&str>,
    resource: &Url,
) -> Url {
    let mut url = server.authorization_endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", resource.as_str());
        if let Some(scope) = scope {
            query.append_pair("scope", scope);
        }
    }
    url
}

/// A bound one-shot loopback redirect listener.
pub struct CallbackListener {
    listener: TcpListener,
    address: SocketAddr,
    path: &'static str,
}

impl CallbackListener {
    /// Bind 127.0.0.1 on `port`, or an ephemeral port, for the MCP callback path.
    pub async fn bind(port: Option<u16>) -> Result<Self, OAuthError> {
        Self::bind_path(port, CALLBACK_PATH).await
    }

    /// Bind 127.0.0.1 on `port`, or an ephemeral port, accepting callbacks at `path`.
    pub async fn bind_path(port: Option<u16>, path: &'static str) -> Result<Self, OAuthError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port.unwrap_or(0)))
            .await
            .map_err(|_| OAuthError::Callback("could not bind the loopback callback port"))?;
        let address = listener
            .local_addr()
            .map_err(|_| OAuthError::Callback("loopback callback address unavailable"))?;
        Ok(Self {
            listener,
            address,
            path,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.address.port(), self.path)
    }

    /// Wait for the callback carrying `state` and return its authorization code. The
    /// listener closes when this returns.
    pub async fn wait(self, state: &str, timeout: Duration) -> Result<String, OAuthError> {
        tokio::time::timeout(timeout, self.accept(state))
            .await
            .map_err(|_| OAuthError::CallbackTimeout)?
    }

    async fn accept(&self, state: &str) -> Result<String, OAuthError> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|_| OAuthError::Callback("loopback callback listener failed"))?;
            let target = tokio::time::timeout(READ_TIMEOUT, request_target(&mut stream))
                .await
                .ok()
                .flatten();
            let Some(result) = target
                .as_deref()
                .and_then(|target| verdict(target, self.path, state))
            else {
                respond(&mut stream, "404 Not Found", "Not found.").await;
                continue;
            };
            let page = match &result {
                Ok(_) => {
                    "Authorization complete. You can close this window and return to Cyber Code."
                }
                Err(_) => "Authorization failed. Return to Cyber Code for details.",
            };
            let status = if result.is_ok() {
                "200 OK"
            } else {
                "400 Bad Request"
            };
            respond(&mut stream, status, page).await;
            return result;
        }
    }
}

/// The request target of a bounded `GET` request head; `None` for anything else.
async fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 || head.len() + count > REQUEST_LIMIT {
            return None;
        }
        head.extend_from_slice(&chunk[..count]);
    }
    let line = std::str::from_utf8(&head).ok()?.lines().next()?;
    let mut parts = line.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    (method == "GET" && target.starts_with('/') && !target.starts_with("//"))
        .then(|| target.to_owned())
}

/// `None` when the request is not for the callback `path`; otherwise the flow's outcome.
fn verdict(target: &str, path: &str, state: &str) -> Option<Result<String, OAuthError>> {
    let url = Url::parse("http://127.0.0.1").ok()?.join(target).ok()?;
    if url.path() != path {
        return None;
    }
    Some(Callback::from_url(&url).and_then(|callback| callback.result(state)))
}

/// The authorization code of a redirect URL a user pasted from the browser's address bar,
/// checked like a loopback callback: its `state` must match.
pub fn callback_code(redirect: &str, state: &str) -> Result<String, OAuthError> {
    let url = Url::parse(redirect.trim())
        .map_err(|_| OAuthError::Callback("pasted text is not the redirect URL"))?;
    Callback::from_url(&url).and_then(|callback| callback.result(state))
}

#[derive(Default)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

impl Callback {
    /// Repeated parameters make the response ambiguous and are refused.
    fn from_url(url: &Url) -> Result<Self, OAuthError> {
        let mut callback = Self::default();
        for (name, value) in url.query_pairs() {
            let slot = match name.as_ref() {
                "code" => &mut callback.code,
                "state" => &mut callback.state,
                "error" => &mut callback.error,
                _ => continue,
            };
            if slot.replace(value.into_owned()).is_some() {
                return Err(OAuthError::Callback("repeated callback parameter"));
            }
        }
        Ok(callback)
    }

    fn result(self, state: &str) -> Result<String, OAuthError> {
        let presented = self.state.unwrap_or_default();
        if !constant_time_eq(presented.as_bytes(), state.as_bytes()) {
            return Err(OAuthError::StateMismatch);
        }
        if let Some(error) = self.error {
            return Err(OAuthError::Denied(sanitized(&error)));
        }
        self.code
            .filter(|code| !code.is_empty())
            .ok_or(OAuthError::Callback("authorization response has no code"))
    }
}

/// Compare without an early exit on the first differing byte.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

async fn respond(stream: &mut TcpStream, status: &str, page: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict_mcp(target: &str, state: &str) -> Option<Result<String, OAuthError>> {
        verdict(target, CALLBACK_PATH, state)
    }

    #[test]
    fn verdict_checks_path_state_error_and_repetition() {
        assert!(verdict_mcp("/favicon.ico", "s").is_none());
        assert!(verdict_mcp("/mcp/oauth/callbackx?code=c&state=s", "s").is_none());
        assert_eq!(
            verdict_mcp("/mcp/oauth/callback?code=c&state=s", "s")
                .unwrap()
                .unwrap(),
            "c"
        );
        assert!(matches!(
            verdict_mcp("/mcp/oauth/callback?code=c&state=t", "s").unwrap(),
            Err(OAuthError::StateMismatch)
        ));
        assert!(matches!(
            verdict_mcp("/mcp/oauth/callback?code=c", "s").unwrap(),
            Err(OAuthError::StateMismatch)
        ));
        assert!(matches!(
            verdict_mcp("/mcp/oauth/callback?code=c&code=d&state=s", "s").unwrap(),
            Err(OAuthError::Callback(_))
        ));
        assert!(matches!(
            verdict_mcp("/mcp/oauth/callback?state=s", "s").unwrap(),
            Err(OAuthError::Callback(_))
        ));
        match verdict_mcp("/mcp/oauth/callback?error=bad%0Aline%3Cx%3E&state=s", "s").unwrap() {
            Err(OAuthError::Denied(code)) => assert_eq!(code, "badlinex"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_pasted_redirect_must_carry_the_state() {
        let pasted = "http://localhost:1455/auth/callback?code=c&state=s\n";
        assert_eq!(callback_code(pasted, "s").unwrap(), "c");
        assert!(matches!(
            callback_code("http://localhost:1455/auth/callback?code=c&state=x", "s"),
            Err(OAuthError::StateMismatch)
        ));
        assert!(matches!(
            callback_code("c", "s"),
            Err(OAuthError::Callback(_))
        ));
    }

    #[test]
    fn constant_time_comparison_matches_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
