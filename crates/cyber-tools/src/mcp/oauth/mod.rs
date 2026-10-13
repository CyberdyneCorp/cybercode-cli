//! MCP OAuth 2.1 for remote servers (client side): discovery, registration, the PKCE
//! authorization-code flow over a loopback redirect, token requests and credential storage.
//!
//! [`authenticate`] is the `cyber mcp auth` acquisition. Server authorization (trust and
//! the individual digest approval of a project-defined server) is checked before any
//! request and rechecked before the token request. All traffic goes through the policy
//! client and its owned proxy, which is joined before returning. The authorize URL is
//! handed to the caller to show; nothing here opens a browser. Diagnostics never contain
//! URLs, codes or tokens.
mod challenge;
mod discovery;
mod flow;
mod provider;
mod register;
mod store;
mod token;

use std::path::Path;
use std::time::Duration;

use cyber_core::config::{McpOAuth, McpServer, Resolved};
use cyber_core::trust::TrustStore;
use cyber_sandbox::{NetworkMode, SandboxConfig};
use futures::StreamExt;
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap};
use serde_json::{Value, json};

pub use challenge::AuthChallenge;
pub use discovery::{AuthorizationServer, Discovery, discover};
pub use flow::{CALLBACK_PATH, CallbackListener, Pkce, authorize_url, random_state};
pub use provider::{REFRESH_WINDOW, TokenSource};
pub use register::{OAuthClient, client_for};
pub use store::{
    AuthMarker, CredentialKey, FileCredentialStore, McpCredentialStore, MemoryCredentialStore,
    RefreshLock, StoredCredential,
};
pub use token::{TokenRequest, TokenSet, exchange_code, refresh};

use super::authorize_server;

const STOP_ACK: Duration = Duration::from_secs(2);
/// Every OAuth request (metadata, registration, token) is abandoned after this long, so an
/// endpoint that accepts the connection and never answers cannot hold a caller forever.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Tokens, client ids and similar values longer than this are refused.
const VALUE_LIMIT: usize = 16 * 1024;

/// Why OAuth failed. Messages never carry URLs, codes, secrets or tokens.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// The server is not currently authorized (trust or digest approval).
    #[error("{0}")]
    Unauthorized(String),
    #[error("MCP server is not a remote server")]
    NotRemote,
    #[error("OAuth is disabled for this MCP server (oauth: false)")]
    Disabled,
    #[error(
        "needs_client_registration: the authorization server offers no dynamic client registration; configure oauth.client_id"
    )]
    NeedsClientRegistration,
    #[error("OAuth discovery failed: {0}")]
    Metadata(&'static str),
    #[error("protected resource metadata names a different resource")]
    ResourceMismatch,
    #[error("authorization server metadata names a different issuer")]
    IssuerMismatch,
    #[error("OAuth endpoints must use HTTPS (plain HTTP only on loopback)")]
    InsecureEndpoint,
    #[error("authorization server does not support PKCE S256")]
    NoPkce,
    #[error("OAuth {stage} failed with HTTP {status}")]
    Status { stage: &'static str, status: u16 },
    #[error("OAuth request failed: {0}")]
    Request(String),
    #[error("invalid OAuth response")]
    Body,
    #[error("token request rejected: {0}")]
    TokenRejected(String),
    #[error("authorization callback state does not match")]
    StateMismatch,
    #[error("authorization was denied: {0}")]
    Denied(String),
    #[error("timed out waiting for the authorization callback")]
    CallbackTimeout,
    #[error("authorization callback failed: {0}")]
    Callback(&'static str),
    #[error("secure random source unavailable")]
    Random,
    #[error("MCP credential storage failed: {0}")]
    Storage(String),
    /// The credential in use was removed (`cyber mcp logout`); nothing is refreshed or
    /// written back.
    #[error("the stored MCP OAuth credential was removed")]
    CredentialRemoved,
    #[error("{0}")]
    Network(String),
    #[error("MCP OAuth network proxy did not stop")]
    Unsettled,
}

/// HTTPS, or plain HTTP to a loopback host; never credentials or a fragment.
pub(crate) fn secure(url: &Url) -> Result<(), OAuthError> {
    let transport = match url.scheme() {
        "https" => true,
        "http" => loopback(url),
        _ => false,
    };
    if transport
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(OAuthError::InsecureEndpoint)
    }
}

/// The RFC 8707 resource indicator of a server URL: the URL without its query or fragment.
/// A query may carry a credential (`?key=...`), so it is never sent to an authorization
/// server or shown in an authorize URL; the credential key keeps the full URL.
pub(crate) fn resource_indicator(server: &Url) -> Url {
    let mut resource = server.clone();
    resource.set_query(None);
    resource.set_fragment(None);
    resource
}

/// [`secure`], and a local host only when `referrer` (the URL whose response or
/// configuration named `url`) is local too, so remote content never steers a request,
/// with its codes, verifiers, secrets or tokens, to a service on this machine.
pub(crate) fn secure_from(url: &Url, referrer: &Url) -> Result<(), OAuthError> {
    secure(url)?;
    if local(url) && !local(referrer) {
        return Err(OAuthError::InsecureEndpoint);
    }
    Ok(())
}

/// Loopback, unspecified (which reaches loopback) or a `localhost` name.
fn local(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback() || address.is_unspecified(),
        Some(url::Host::Ipv6(address)) => address.is_loopback() || address.is_unspecified(),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        None => false,
    }
}

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Visible ASCII only, bounded: safe in a header and in a form.
pub(crate) fn visible(value: &str) -> bool {
    !value.is_empty() && value.len() <= VALUE_LIMIT && value.bytes().all(|b| b.is_ascii_graphic())
}

/// An OAuth error code reduced to `[A-Za-z0-9_.-]`, at most 64 characters.
pub(crate) fn sanitized(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect()
}

pub(crate) fn request_failure(error: reqwest::Error) -> OAuthError {
    OAuthError::Request(error.without_url().to_string())
}

/// A bounded JSON object body.
pub(crate) async fn read_json(response: reqwest::Response) -> Result<Value, OAuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > super::LIMIT as u64)
    {
        return Err(OAuthError::Body);
    }
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(request_failure)?;
        if chunk.len() > super::LIMIT - bytes.len() {
            return Err(OAuthError::Body);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
        .ok_or(OAuthError::Body)
}

/// GET a metadata document: `None` when the server has none there (any non-success).
pub(crate) async fn get_json(
    client: &reqwest::Client,
    url: &Url,
) -> Result<Option<Value>, OAuthError> {
    let response = client
        .get(url.clone())
        .header(ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(request_failure)?;
    if !response.status().is_success() {
        return Ok(None);
    }
    read_json(response).await.map(Some)
}

/// One `cyber mcp auth` request.
pub struct AuthRequest<'a> {
    pub resolved: &'a Resolved,
    pub trust: &'a TrustStore,
    pub location: &'a Path,
    pub home: &'a Path,
    pub name: &'a str,
    /// Overrides `oauth.redirect_port`.
    pub port: Option<u16>,
    /// How long to wait for the browser callback.
    pub timeout: Duration,
}

/// What was stored, without any token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthOutcome {
    pub issuer: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<u64>,
    /// Whether the client was registered dynamically.
    pub registered: bool,
}

/// The remote definition's OAuth inputs.
struct Remote {
    url: Url,
    headers: HeaderMap,
    settings: McpOAuth,
    digest: String,
    restricted: bool,
}

/// Acquire and store a token for a remote server: authorize, probe for the challenge,
/// discover, register, show the authorize URL through `present`, wait for the loopback
/// callback, recheck authorization, exchange the code and store the credential.
pub async fn authenticate(
    request: &AuthRequest<'_>,
    store: &dyn McpCredentialStore,
    present: &(dyn Fn(&str) + Sync),
) -> Result<AuthOutcome, OAuthError> {
    let remote = remote(request)?;
    let config = SandboxConfig::resolve(
        &request.resolved.value,
        &request.resolved.sources,
        None,
        request.home,
    );
    let network = if remote.restricted {
        config.network
    } else {
        NetworkMode::On
    };
    let policy =
        crate::net_client::policy_client(&remote.url, network, config.allowed_domains, "MCP OAuth")
            .await
            .map_err(OAuthError::Network)?;
    let result = acquire(request, &remote, &policy.client, store, present).await;
    let stopped = matches!(
        tokio::time::timeout(STOP_ACK, policy.proxy.shutdown()).await,
        Ok(Ok(()))
    );
    match result {
        Ok(_) if !stopped => Err(OAuthError::Unsettled),
        result => result,
    }
}

fn remote(request: &AuthRequest<'_>) -> Result<Remote, OAuthError> {
    let selection = authorize_server(
        request.resolved,
        request.trust,
        request.location,
        request.name,
    )
    .map_err(OAuthError::Unauthorized)?;
    let McpServer::Remote { url, headers, .. } = &selection.definition else {
        return Err(OAuthError::NotRemote);
    };
    let settings = selection.definition.oauth().ok_or(OAuthError::Disabled)?;
    let url = Url::parse(url).map_err(|_| OAuthError::Metadata("invalid MCP remote URL"))?;
    // A bearer token is never obtained for a server it would cross the network to in
    // cleartext, and its metadata is never fetched in cleartext.
    secure(&url)?;
    let headers = crate::net_client::headers(
        headers,
        "MCP remote server",
        cyber_core::config::transport_owned_header,
    )
    .map_err(OAuthError::Network)?;
    Ok(Remote {
        url,
        headers,
        settings,
        digest: selection.digest,
        restricted: selection.requires_sandbox,
    })
}

async fn acquire(
    request: &AuthRequest<'_>,
    remote: &Remote,
    http: &reqwest::Client,
    store: &dyn McpCredentialStore,
    present: &(dyn Fn(&str) + Sync),
) -> Result<AuthOutcome, OAuthError> {
    let challenge = probe(http, remote).await?;
    let discovery = discover(http, &remote.url, challenge.as_ref()).await?;
    let listener = CallbackListener::bind(request.port.or(remote.settings.redirect_port)).await?;
    let redirect = listener.redirect_uri();
    let client = client(http, request.name, remote, &discovery, &redirect, store).await?;
    let marker = store.marker(request.name, remote.url.as_str())?;
    let scope = with_required(
        requested_scope(&remote.settings, challenge.as_ref(), &discovery),
        &marker.required_scopes,
    );
    let pkce = Pkce::generate()?;
    let state = random_state()?;
    let url = authorize_url(
        &discovery.server,
        &client.client_id,
        &redirect,
        &pkce,
        &state,
        scope.as_deref(),
        &discovery.resource,
    );
    present(url.as_str());
    let code = listener.wait(&state, request.timeout).await?;
    recheck(request, &remote.digest)?;
    let token_request = TokenRequest {
        client: &client,
        endpoint: &discovery.server.token_endpoint,
        resource: &discovery.resource,
    };
    let mut tokens = exchange_code(http, &token_request, &code, &pkce.verifier, &redirect).await?;
    if tokens.scopes.is_empty() {
        tokens.scopes = scope
            .iter()
            .flat_map(|s| s.split_whitespace())
            .map(str::to_owned)
            .collect();
    }
    let key = CredentialKey {
        server: request.name.into(),
        url: remote.url.to_string(),
        issuer: discovery.server.issuer.to_string(),
    };
    let credential = StoredCredential {
        tokens,
        client,
        token_endpoint: discovery.server.token_endpoint.to_string(),
    };
    store.put(&key, &credential)?;
    store.clear_marker(request.name, &key.url)?;
    Ok(AuthOutcome {
        issuer: key.issuer,
        scopes: credential.tokens.scopes,
        expires_at: credential.tokens.expires_at,
        registered: remote.settings.client_id.is_none(),
    })
}

/// The configured or registered client. Without either, the condition is recorded for the
/// server's exact URL so the runtime reports `needs_client_registration` without running
/// discovery; a failure to record it is logged and the typed condition still returned.
async fn client(
    http: &reqwest::Client,
    name: &str,
    remote: &Remote,
    discovery: &Discovery,
    redirect: &str,
    store: &dyn McpCredentialStore,
) -> Result<OAuthClient, OAuthError> {
    let result = client_for(http, &discovery.server, &remote.settings, redirect).await;
    if matches!(result, Err(OAuthError::NeedsClientRegistration)) {
        let marker = AuthMarker {
            needs_client_registration: true,
            ..AuthMarker::default()
        };
        if let Err(error) = store.mark(name, remote.url.as_str(), &marker) {
            cyber_core::log::error("mcp", &error.to_string(), json!({"server":name}));
        }
    }
    result
}

/// `scope` plus the scopes a refused request required (`insufficient_scope`), each once.
fn with_required(scope: Option<String>, required: &[String]) -> Option<String> {
    let mut scopes: Vec<&str> = scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    for extra in required {
        if !scopes.contains(&extra.as_str()) {
            scopes.push(extra);
        }
    }
    (!scopes.is_empty()).then(|| scopes.join(" "))
}

/// Configured scopes, else the server's challenge, else what the resource advertises.
fn requested_scope(
    settings: &McpOAuth,
    challenge: Option<&AuthChallenge>,
    discovery: &Discovery,
) -> Option<String> {
    let scope = match (&settings.scopes, challenge.and_then(|c| c.scope.as_deref())) {
        (Some(scopes), _) => scopes.join(" "),
        (None, Some(scope)) => scope.to_owned(),
        (None, None) => discovery.scopes_supported.join(" "),
    };
    (!scope.trim().is_empty()).then_some(scope)
}

/// The definition, trust and digest approval must be unchanged since the flow began.
fn recheck(request: &AuthRequest<'_>, digest: &str) -> Result<(), OAuthError> {
    let changed =
        || OAuthError::Unauthorized("MCP server authorization changed during OAuth".into());
    let selection = authorize_server(
        request.resolved,
        request.trust,
        request.location,
        request.name,
    )
    .map_err(|_| changed())?;
    if selection.digest != digest {
        return Err(changed());
    }
    Ok(())
}

/// An unauthenticated `initialize` that collects the server's `Bearer` challenge. A server
/// that answers without requiring authorization gets its new session closed again.
async fn probe(
    http: &reqwest::Client,
    remote: &Remote,
) -> Result<Option<AuthChallenge>, OAuthError> {
    let body = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":super::stdio::VERSION,"capabilities":{},
        "clientInfo":{"name":"cyber","version":env!("CARGO_PKG_VERSION")}}});
    let response = http
        .post(remote.url.clone())
        .headers(remote.headers.clone())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(body.to_string())
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(request_failure)?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Ok(AuthChallenge::from_headers(response.headers()));
    }
    if let Some(session) = response.headers().get("mcp-session-id").cloned() {
        let close = http
            .delete(remote.url.clone())
            .headers(remote.headers.clone())
            .header("mcp-session-id", session)
            .send();
        let _ = tokio::time::timeout(STOP_ACK, close).await;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_loopback_http_is_secure() {
        for url in [
            "https://as.example.test/token",
            "http://127.0.0.1:8080/token",
            "http://[::1]/token",
            "http://localhost/token",
        ] {
            assert!(secure(&Url::parse(url).unwrap()).is_ok(), "{url}");
        }
        for url in [
            "http://as.example.test/token",
            "http://127.0.0.1.example.test/token",
            "ftp://127.0.0.1/token",
            "https://user:pass@as.example.test/token",
            "https://as.example.test/token#fragment",
        ] {
            assert!(secure(&Url::parse(url).unwrap()).is_err(), "{url}");
        }
    }

    #[test]
    fn only_a_local_referrer_may_name_a_local_host() {
        let parse = |url: &str| Url::parse(url).unwrap();
        let remote = parse("https://mcp.example.test/mcp");
        let local = parse("http://127.0.0.1:7000/mcp");
        for url in [
            "http://127.0.0.1:8080/admin",
            "https://127.0.0.1/token",
            "http://localhost/token",
            "https://localhost./token",
            "https://api.localhost/token",
            "https://0.0.0.0/token",
            "http://[::1]/token",
        ] {
            assert!(secure_from(&parse(url), &remote).is_err(), "{url}");
        }
        assert!(secure_from(&parse("https://as.example.test/token"), &remote).is_ok());
        assert!(secure_from(&parse("http://127.0.0.1:8080/token"), &local).is_ok());
        assert!(secure_from(&parse("https://as.example.test/token"), &local).is_ok());
        assert!(secure_from(&parse("http://as.example.test/token"), &local).is_err());
    }

    #[test]
    fn scope_prefers_configuration_then_challenge_then_advertised() {
        let server = AuthorizationServer {
            issuer: Url::parse("https://as.test").unwrap(),
            authorization_endpoint: Url::parse("https://as.test/a").unwrap(),
            token_endpoint: Url::parse("https://as.test/t").unwrap(),
            registration_endpoint: None,
            scopes_supported: Vec::new(),
        };
        let discovery = Discovery {
            resource: Url::parse("https://mcp.test/mcp").unwrap(),
            scopes_supported: vec!["advertised".into()],
            server,
        };
        let challenge = AuthChallenge {
            scope: Some("challenged".into()),
            ..AuthChallenge::default()
        };
        let configured = McpOAuth {
            scopes: Some(vec!["a".into(), "b".into()]),
            ..McpOAuth::default()
        };
        let none = McpOAuth::default();
        assert_eq!(
            requested_scope(&configured, Some(&challenge), &discovery).as_deref(),
            Some("a b")
        );
        assert_eq!(
            requested_scope(&none, Some(&challenge), &discovery).as_deref(),
            Some("challenged")
        );
        assert_eq!(
            requested_scope(&none, None, &discovery).as_deref(),
            Some("advertised")
        );
        let mut silent = discovery.clone();
        silent.scopes_supported.clear();
        assert_eq!(requested_scope(&none, None, &silent), None);
    }

    #[test]
    fn required_scopes_are_added_once() {
        let required = ["b".to_string(), "c".to_string()];
        assert_eq!(
            with_required(Some("a b".into()), &required).as_deref(),
            Some("a b c")
        );
        assert_eq!(with_required(None, &required).as_deref(), Some("b c"));
        assert_eq!(with_required(Some("a".into()), &[]).as_deref(), Some("a"));
        assert_eq!(with_required(None, &[]), None);
    }
}
