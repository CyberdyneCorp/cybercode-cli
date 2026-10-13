//! The OAuth client identity: a configured `oauth.client_id`, or RFC 7591 dynamic client
//! registration as a public client bound to the loopback redirect URI.
use reqwest::Url;
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{AuthorizationServer, OAuthError, read_json, request_failure, visible};
use cyber_core::config::McpOAuth;

/// A client identity. `Debug` never shows the secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthClient {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "***"))
            .finish()
    }
}

/// The configured client, else a dynamically registered one. Without a configured
/// `client_id` and without a registration endpoint the server needs manual registration.
pub async fn client_for(
    http: &reqwest::Client,
    server: &AuthorizationServer,
    settings: &McpOAuth,
    redirect_uri: &str,
) -> Result<OAuthClient, OAuthError> {
    if let Some(client_id) = &settings.client_id {
        return Ok(OAuthClient {
            client_id: client_id.clone(),
            client_secret: settings.client_secret.clone(),
        });
    }
    let endpoint = server
        .registration_endpoint
        .as_ref()
        .ok_or(OAuthError::NeedsClientRegistration)?;
    register(http, endpoint, redirect_uri, settings.scopes.as_deref()).await
}

async fn register(
    http: &reqwest::Client,
    endpoint: &Url,
    redirect_uri: &str,
    scopes: Option<&[String]>,
) -> Result<OAuthClient, OAuthError> {
    let mut body = json!({
        "client_name": "Cyber Code",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none"
    });
    if let Some(scopes) = scopes {
        body["scope"] = json!(scopes.join(" "));
    }
    let response = http
        .post(endpoint.clone())
        .header(ACCEPT, "application/json")
        .json(&body)
        .timeout(super::REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(request_failure)?;
    let status = response.status().as_u16();
    if !matches!(status, 200 | 201) {
        return Err(OAuthError::Status {
            stage: "client registration",
            status,
        });
    }
    let document = read_json(response).await?;
    let client_id = document["client_id"]
        .as_str()
        .filter(|id| visible(id))
        .ok_or(OAuthError::Body)?;
    Ok(OAuthClient {
        client_id: client_id.into(),
        client_secret: document["client_secret"].as_str().map(str::to_owned),
    })
}
