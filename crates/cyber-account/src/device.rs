//! Optional RFC 8628 flow, selected only through advertised discovery capabilities.
use crate::discovery::bounded_json;
use crate::{Error, OidcClient, Secret, TokenSet};
use serde::Deserialize;
use std::time::{Duration, Instant};
use url::Url;

pub struct DeviceAuthorization {
    issuer: String,
    client_id: String,
    device_code: Secret,
    verification_uri: Url,
    user_code: String,
    deadline: Instant,
    interval: Duration,
}
impl DeviceAuthorization {
    pub fn verification_uri(&self) -> &Url {
        &self.verification_uri
    }
    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

impl OidcClient {
    pub async fn start_device_authorization(&self) -> Result<DeviceAuthorization, Error> {
        #[derive(Deserialize)]
        struct Response {
            device_code: Secret,
            user_code: String,
            verification_uri: String,
            expires_in: u64,
            interval: Option<u64>,
        }
        let endpoint = self
            .provider()
            .device_authorization_endpoint()
            .ok_or(Error::PkceUnsupported)?;
        let scopes = self.provider().login_scopes()?;
        let started = Instant::now();
        let response = self
            .discovery
            .client
            .post(endpoint.clone())
            .form(&[
                ("client_id", self.client_id()),
                ("scope", &scopes.requested.join(" ")),
            ])
            .send()
            .await
            .map_err(|_| Error::TokenExchange)?;
        let response: Response = bounded_json(response)
            .await
            .map_err(|_| Error::TokenExchange)?;
        if response.device_code.expose().is_empty()
            || response.user_code.is_empty()
            || response.user_code.len() > 64
            || response.user_code.chars().any(char::is_control)
            || response.expires_in == 0
            || response.interval == Some(0)
        {
            return Err(Error::InvalidToken);
        }
        let verification_uri = self.discovery.endpoint(response.verification_uri)?;
        Ok(DeviceAuthorization {
            issuer: self.provider().issuer().to_owned(),
            client_id: self.client_id().to_owned(),
            device_code: response.device_code,
            verification_uri,
            user_code: response.user_code,
            deadline: started + Duration::from_secs(response.expires_in.min(900)),
            interval: Duration::from_secs(response.interval.unwrap_or(5)),
        })
    }
    pub async fn poll_device_authorization(
        &mut self,
        mut device: DeviceAuthorization,
        audience: &str,
    ) -> Result<TokenSet, Error> {
        if device.issuer != self.provider().issuer() || device.client_id != self.client_id() {
            return Err(Error::IssuerMismatch);
        }
        loop {
            let remaining = device.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::LoginExpired);
            }
            tokio::time::sleep(device.interval.min(remaining)).await;
            if Instant::now() >= device.deadline {
                return Err(Error::LoginExpired);
            }
            let response = self
                .discovery
                .client
                .post(self.provider().token_endpoint().clone())
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.client_id()),
                    ("device_code", device.device_code.expose()),
                ])
                .send()
                .await
                .map_err(|_| Error::TokenExchange)?;
            if response.status().is_success() {
                return self.accept_device_tokens(response, audience).await;
            }
            #[derive(Deserialize)]
            struct Denial {
                error: String,
            }
            // OAuth pending/slow_down use HTTP 400. Parse a bounded body without
            // reflecting provider text, URLs or the secret device code in errors.
            let mut response = response;
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::TokenExchange)? {
                if chunk.len() > crate::discovery::MAX_DOCUMENT.saturating_sub(bytes.len()) {
                    return Err(Error::TokenExchange);
                }
                bytes.extend_from_slice(&chunk);
            }
            let denial: Denial =
                serde_json::from_slice(&bytes).map_err(|_| Error::TokenExchange)?;
            match denial.error.as_str() {
                "authorization_pending" => {}
                "slow_down" => {
                    device.interval = device.interval.saturating_add(Duration::from_secs(5))
                }
                "access_denied" => return Err(Error::AuthorizationDenied),
                "expired_token" => return Err(Error::LoginExpired),
                _ => return Err(Error::TokenExchange),
            }
        }
    }
}
