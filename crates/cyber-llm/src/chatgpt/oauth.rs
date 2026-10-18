//! The OAuth requests: the authorize URL, the code exchange (form) and the refresh (JSON),
//! and the claims Cyber Code reads from the issued tokens.
//!
//! Token claims are read without checking signatures: the tokens come straight from the
//! token endpoint over TLS, and only identify the account the backend already bills.
use base64::Engine;
use futures::StreamExt;
use reqwest::Url;
use serde_json::{Value, json};

use super::{CLIENT_ID, ChatGptError, Issuer, ORIGINATOR, REDIRECT_URI};

/// Token responses are small; longer ones are refused.
const RESPONSE_LIMIT: usize = 64 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const SCOPE: &str = "openid profile email offline_access";
const REFRESH_SCOPE: &str = "openid profile email";

/// The authorization request URL for one PKCE `challenge` and `state`.
pub fn authorize_url(issuer: &Issuer, challenge: &str, state: &str) -> Url {
    let mut url = issuer.authorize_endpoint();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", ORIGINATOR);
    url
}

/// What the token endpoint returned; a refresh may omit any token it does not rotate.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in: Option<u64>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mask = |token: &Option<String>| token.as_ref().map(|_| "***");
        f.debug_struct("TokenResponse")
            .field("access_token", &mask(&self.access_token))
            .field("refresh_token", &mask(&self.refresh_token))
            .field("id_token", &mask(&self.id_token))
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Redeem an authorization code with its PKCE verifier.
pub async fn exchange_code(
    http: &reqwest::Client,
    issuer: &Issuer,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse, ChatGptError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", CLIENT_ID),
        ("code_verifier", verifier),
    ];
    let request = http.post(issuer.token_endpoint()).form(&form);
    send(request, "code exchange").await
}

/// Use a refresh token.
pub async fn refresh(
    http: &reqwest::Client,
    issuer: &Issuer,
    refresh_token: &str,
) -> Result<TokenResponse, ChatGptError> {
    let body = json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "scope": REFRESH_SCOPE,
    });
    let request = http.post(issuer.token_endpoint()).json(&body);
    send(request, "token refresh").await
}

async fn send(
    request: reqwest::RequestBuilder,
    stage: &'static str,
) -> Result<TokenResponse, ChatGptError> {
    let response = request
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| ChatGptError::Transport(stage))?;
    let status = response.status().as_u16();
    let document = read_json(response, stage).await;
    match status {
        200 => parse(&document?),
        400 | 401 => Err(ChatGptError::Rejected(error_code(document.ok()))),
        status => Err(ChatGptError::Status { stage, status }),
    }
}

async fn read_json(
    response: reqwest::Response,
    stage: &'static str,
) -> Result<Value, ChatGptError> {
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| ChatGptError::Transport(stage))?;
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            return Err(ChatGptError::Body("response too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ChatGptError::Body("not JSON"))
}

/// The OAuth `error` code, reduced to a safe token for diagnostics.
fn error_code(document: Option<Value>) -> String {
    let code: String = document
        .as_ref()
        .and_then(|document| document["error"].as_str())
        .unwrap_or("invalid_response")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect();
    code
}

fn parse(document: &Value) -> Result<TokenResponse, ChatGptError> {
    let token = |name: &str| match &document[name] {
        Value::Null => Ok(None),
        Value::String(token) if visible(token) => Ok(Some(token.clone())),
        _ => Err(ChatGptError::Body("token is not a visible ASCII string")),
    };
    Ok(TokenResponse {
        access_token: token("access_token")?,
        refresh_token: token("refresh_token")?,
        id_token: token("id_token")?,
        expires_in: document["expires_in"].as_u64(),
    })
}

/// Only visible ASCII is accepted, so a token can never inject into a request header.
pub(super) fn visible(token: &str) -> bool {
    !token.is_empty() && token.len() <= 16 * 1024 && token.bytes().all(|b| b.is_ascii_graphic())
}

/// What Cyber Code reads from an ID or access token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub expires_at: Option<u64>,
}

impl Claims {
    /// The claims of a JWT, or empty claims when it is not one.
    pub fn of(token: &str) -> Self {
        let Some(payload) = jwt_payload(token) else {
            return Self::default();
        };
        let auth = &payload["https://api.openai.com/auth"];
        let text = |value: &Value| value.as_str().filter(|v| !v.is_empty()).map(str::to_owned);
        Self {
            account_id: text(&auth["chatgpt_account_id"]),
            email: text(&payload["email"])
                .or_else(|| text(&payload["https://api.openai.com/profile"]["email"])),
            plan: text(&auth["chatgpt_plan_type"]),
            expires_at: payload["exp"].as_u64(),
        }
    }
}

fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
pub(crate) fn test_jwt(payload: &Value) -> String {
    let encode =
        |value: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.sig",
        encode(&json!({"alg": "none"})),
        encode(payload)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_the_codex_flow_parameters() {
        let url = authorize_url(&Issuer::default(), "chal", "st");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/oauth/authorize");
        assert_eq!(pairs["client_id"], CLIENT_ID);
        assert_eq!(pairs["redirect_uri"], "http://localhost:1455/auth/callback");
        assert_eq!(pairs["scope"], "openid profile email offline_access");
        assert_eq!(pairs["code_challenge"], "chal");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["state"], "st");
        assert_eq!(pairs["originator"], "cyber");
        assert_eq!(pairs["id_token_add_organizations"], "true");
    }

    #[test]
    fn claims_read_account_plan_email_and_expiry() {
        let token = test_jwt(&json!({
            "email": "a@b.c",
            "exp": 42,
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct", "chatgpt_plan_type": "plus"}
        }));
        assert_eq!(
            Claims::of(&token),
            Claims {
                account_id: Some("acct".into()),
                email: Some("a@b.c".into()),
                plan: Some("plus".into()),
                expires_at: Some(42),
            }
        );
    }

    #[test]
    fn claims_of_a_malformed_token_are_empty() {
        assert_eq!(Claims::of("opaque"), Claims::default());
        assert_eq!(Claims::of("a.!!!.c"), Claims::default());
        let no_account =
            test_jwt(&json!({"https://api.openai.com/auth": {"chatgpt_account_id": ""}}));
        assert_eq!(Claims::of(&no_account).account_id, None);
    }

    #[test]
    fn token_parsing_refuses_header_unsafe_values() {
        assert!(parse(&json!({"access_token": "a b"})).is_err());
        assert!(parse(&json!({"access_token": 1})).is_err());
        let parsed = parse(&json!({"access_token": "a", "expires_in": 10})).unwrap();
        assert_eq!(parsed.access_token.as_deref(), Some("a"));
        assert_eq!(parsed.refresh_token, None);
        assert_eq!(parsed.expires_in, Some(10));
    }

    #[test]
    fn error_codes_are_sanitized() {
        assert_eq!(
            error_code(Some(json!({"error": "invalid_grant\n<x>"}))),
            "invalid_grantx"
        );
        assert_eq!(error_code(None), "invalid_response");
    }
}
