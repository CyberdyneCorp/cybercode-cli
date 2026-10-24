use crate::{Error, OidcClient, Transport};
use serde::Deserialize;

/// Load this only from trusted user/managed global settings, never a checkout.
/// Reading settings performs no network requests or credential-vault operations.
#[derive(Clone, Debug)]
pub struct AccountSettings {
    pub issuer: String,
    pub client_id: String,
    pub audience: String,
    pub transport: Transport,
}
impl AccountSettings {
    pub fn from_global(
        config: &serde_json::Value,
        issuer_override: Option<String>,
    ) -> Result<Self, Error> {
        #[derive(Default, Deserialize)]
        struct Raw {
            issuer: Option<String>,
            client_id: Option<String>,
            audience: Option<String>,
            #[serde(default)]
            allow_loopback_http: bool,
        }
        let raw: Raw = match config.get("account") {
            None => Raw::default(),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|_| Error::InvalidClient)?
            }
        };
        let issuer = issuer_override.or(raw.issuer).unwrap_or_else(|| {
            option_env!("CYBER_ACCOUNT_DEFAULT_ISSUER")
                .unwrap_or("https://auth.backend.coolify.cyberdynecorp.ai")
                .to_owned()
        });
        let client_id = raw.client_id.unwrap_or_else(|| "cyber-cli".into());
        let audience = raw.audience.unwrap_or_else(|| client_id.clone());
        if client_id.trim().is_empty() {
            return Err(Error::InvalidClient);
        }
        if audience.trim().is_empty() {
            return Err(Error::InvalidAudience);
        }
        Ok(Self {
            issuer,
            client_id,
            audience,
            transport: if raw.allow_loopback_http {
                Transport::AllowLoopbackHttp
            } else {
                Transport::Https
            },
        })
    }
    pub async fn client(&self, cache: &std::path::Path) -> Result<OidcClient, Error> {
        OidcClient::discover_with_cache(&self.issuer, &self.client_id, self.transport, Some(cache))
            .await
    }
}
