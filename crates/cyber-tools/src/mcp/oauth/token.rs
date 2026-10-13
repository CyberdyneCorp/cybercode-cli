//! Token endpoint requests: the authorization-code exchange and refresh, both carrying the
//! RFC 8707 `resource`. A confidential client authenticates with HTTP Basic; a public
//! client sends only its `client_id`. Tokens never appear in `Debug` or diagnostics.
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{OAuthClient, OAuthError, read_json, request_failure, sanitized, visible};

/// Issued tokens. `issued_at` and `expires_at` are Unix seconds.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// When the access token was issued, so its refresh window can follow its lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"***")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_at", &self.expires_at)
            .field("issued_at", &self.issued_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Who asks, where, and for which resource.
pub struct TokenRequest<'a> {
    pub client: &'a OAuthClient,
    pub endpoint: &'a Url,
    pub resource: &'a Url,
}

/// Redeem an authorization code with its PKCE verifier.
pub async fn exchange_code(
    http: &reqwest::Client,
    request: &TokenRequest<'_>,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    post(http, request, &form).await
}

/// Use a refresh token. A server that rotates returns a new refresh token; one that does
/// not keeps the presented token valid, so it is carried forward.
pub async fn refresh(
    http: &reqwest::Client,
    request: &TokenRequest<'_>,
    refresh_token: &str,
    scope: Option<&str>,
) -> Result<TokenSet, OAuthError> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if let Some(scope) = scope {
        form.push(("scope", scope));
    }
    let mut tokens = post(http, request, &form).await?;
    tokens
        .refresh_token
        .get_or_insert_with(|| refresh_token.to_owned());
    Ok(tokens)
}

async fn post(
    http: &reqwest::Client,
    request: &TokenRequest<'_>,
    form: &[(&str, &str)],
) -> Result<TokenSet, OAuthError> {
    let mut builder = http
        .post(request.endpoint.clone())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ACCEPT, "application/json")
        .body(encode(request, form))
        .timeout(super::REQUEST_TIMEOUT);
    if let Some(secret) = &request.client.client_secret {
        builder = builder.basic_auth(&request.client.client_id, Some(secret));
    }
    let response = builder.send().await.map_err(request_failure)?;
    let status = response.status().as_u16();
    let document = read_json(response).await;
    match status {
        200 => parse(&document?),
        400 | 401 => Err(OAuthError::TokenRejected(error_code(document.ok()))),
        status => Err(OAuthError::Status {
            stage: "token request",
            status,
        }),
    }
}

/// The form body with the client and RFC 8707 `resource`. The serializer is not `Send`,
/// so it never lives across an await.
fn encode(request: &TokenRequest<'_>, form: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .append_pair("client_id", &request.client.client_id)
        .append_pair("resource", request.resource.as_str())
        .finish()
}

fn error_code(document: Option<Value>) -> String {
    document
        .as_ref()
        .and_then(|document| document["error"].as_str())
        .map(sanitized)
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| "invalid_response".into())
}

/// Only `Bearer` tokens made of visible ASCII are accepted, so the token can never inject
/// into a request header.
fn parse(document: &Value) -> Result<TokenSet, OAuthError> {
    let bearer = document["token_type"]
        .as_str()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("bearer"));
    let access_token = document["access_token"]
        .as_str()
        .filter(|token| bearer && visible(token))
        .ok_or(OAuthError::Body)?;
    let refresh_token = match &document["refresh_token"] {
        Value::Null => None,
        Value::String(token) if visible(token) => Some(token.clone()),
        _ => return Err(OAuthError::Body),
    };
    let issued_at = now();
    Ok(TokenSet {
        access_token: access_token.into(),
        refresh_token,
        expires_at: document["expires_in"]
            .as_u64()
            .map(|seconds| issued_at.saturating_add(seconds)),
        issued_at: Some(issued_at),
        scopes: document["scope"]
            .as_str()
            .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
    })
}

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
