use crate::Error;
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use std::{net::IpAddr, time::Duration};
use url::{Host, Url};

const MAX_DOCUMENT: usize = 256 * 1024;

/// Plain HTTP is only allowed for literal loopback development fixtures, by opt-in.
#[derive(Clone, Copy, Debug, Default)]
pub enum Transport {
    #[default]
    Https,
    AllowLoopbackHttp,
}

impl Transport {
    fn permits(self, url: &Url) -> bool {
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return false;
        }
        match (url.scheme(), url.host()) {
            ("https", Some(_)) => true,
            ("http", Some(Host::Ipv4(ip))) => {
                matches!(self, Self::AllowLoopbackHttp) && IpAddr::V4(ip).is_loopback()
            }
            ("http", Some(Host::Ipv6(ip))) => {
                matches!(self, Self::AllowLoopbackHttp) && IpAddr::V6(ip).is_loopback()
            }
            _ => false,
        }
    }
}

#[derive(Deserialize)]
struct Document {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: Option<String>,
    end_session_endpoint: Option<String>,
    introspection_endpoint: Option<String>,
    device_authorization_endpoint: Option<String>,
    scopes_supported: Option<Vec<String>>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    grant_types_supported: Option<Vec<String>>,
}

/// Validated metadata. Only discovery can construct it; no credentials are stored here.
#[derive(Debug)]
pub struct Provider {
    pub(crate) issuer: String,
    pub(crate) authorization: Url,
    token: Url,
    jwks: Url,
    userinfo: Option<Url>,
    end_session: Option<Url>,
    introspection: Option<Url>,
    device: Option<Url>,
    pub(crate) scopes: Option<Vec<String>>,
    grants: Vec<String>,
}

impl Provider {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn token_endpoint(&self) -> &Url {
        &self.token
    }

    pub fn jwks_uri(&self) -> &Url {
        &self.jwks
    }

    pub fn userinfo_endpoint(&self) -> Option<&Url> {
        self.userinfo.as_ref()
    }

    pub fn end_session_endpoint(&self) -> Option<&Url> {
        self.end_session.as_ref()
    }

    pub fn introspection_endpoint(&self) -> Option<&Url> {
        self.introspection.as_ref()
    }

    /// A schema path alone does not enable device login; both advertisements are required.
    pub fn device_authorization_endpoint(&self) -> Option<&Url> {
        self.grants
            .iter()
            .any(|g| g == "urn:ietf:params:oauth:grant-type:device_code")
            .then_some(self.device.as_ref())
            .flatten()
    }
}

/// Explicitly invoked transport; constructing it does not contact the issuer.
/// Redirects are refused, responses bounded and requests limited to ten seconds.
pub struct DiscoveryClient {
    client: Client,
    transport: Transport,
}

impl DiscoveryClient {
    pub fn new(transport: Transport) -> Result<Self, Error> {
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::DiscoveryUnavailable)?;
        Ok(Self { client, transport })
    }

    pub async fn discover(&self, issuer: &str) -> Result<Provider, Error> {
        let issuer = self.issuer(issuer)?;
        let endpoint = format!("{issuer}/.well-known/openid-configuration");
        let mut response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| Error::DiscoveryUnavailable)?;
        if !response.status().is_success() {
            return Err(Error::DiscoveryUnavailable);
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_DOCUMENT as u64)
        {
            return Err(Error::DiscoveryTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::DiscoveryUnavailable)?
        {
            if chunk.len() > MAX_DOCUMENT.saturating_sub(bytes.len()) {
                return Err(Error::DiscoveryTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let document = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidDiscovery)?;
        self.validate(issuer, document)
    }

    fn issuer(&self, issuer: &str) -> Result<String, Error> {
        let url = Url::parse(issuer).map_err(|_| Error::InvalidIssuer)?;
        if !self.transport.permits(&url) || url.query().is_some() {
            return Err(Error::InvalidIssuer);
        }
        Ok(url.as_str().trim_end_matches('/').to_owned())
    }

    fn endpoint(&self, endpoint: String) -> Result<Url, Error> {
        let url = Url::parse(&endpoint).map_err(|_| Error::InvalidDiscovery)?;
        self.transport
            .permits(&url)
            .then_some(url)
            .ok_or(Error::InvalidDiscovery)
    }

    fn validate(&self, issuer: String, doc: Document) -> Result<Provider, Error> {
        if self
            .issuer(&doc.issuer)
            .map_err(|_| Error::IssuerMismatch)?
            != issuer
        {
            return Err(Error::IssuerMismatch);
        }
        if !doc
            .code_challenge_methods_supported
            .iter()
            .any(|s| s == "S256")
            || doc
                .grant_types_supported
                .as_ref()
                .is_some_and(|grants| !grants.iter().any(|s| s == "authorization_code"))
        {
            return Err(Error::PkceUnsupported);
        }
        let optional = |s: Option<String>| s.map(|s| self.endpoint(s)).transpose();
        Ok(Provider {
            issuer,
            authorization: self.endpoint(doc.authorization_endpoint)?,
            token: self.endpoint(doc.token_endpoint)?,
            jwks: self.endpoint(doc.jwks_uri)?,
            userinfo: optional(doc.userinfo_endpoint)?,
            end_session: optional(doc.end_session_endpoint)?,
            introspection: optional(doc.introspection_endpoint)?,
            device: optional(doc.device_authorization_endpoint)?,
            scopes: doc.scopes_supported,
            grants: doc
                .grant_types_supported
                .unwrap_or_else(|| vec!["authorization_code".into()]),
        })
    }
}
