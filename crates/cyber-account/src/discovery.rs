use crate::Error;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{net::IpAddr, time::Duration};
use url::{Host, Url};

pub(crate) const MAX_DOCUMENT: usize = 256 * 1024;

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

#[derive(Clone, Deserialize, Serialize)]
struct Document {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: Option<String>,
    end_session_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    introspection_endpoint: Option<String>,
    device_authorization_endpoint: Option<String>,
    scopes_supported: Option<Vec<String>>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    grant_types_supported: Option<Vec<String>>,
    #[serde(default)]
    id_token_signing_alg_values_supported: Vec<String>,
}

/// Validated metadata. Only discovery can construct it; no credentials are stored here.
#[derive(Clone, Debug)]
pub struct Provider {
    pub(crate) issuer: String,
    pub(crate) authorization: Url,
    token: Url,
    jwks: Url,
    userinfo: Option<Url>,
    end_session: Option<Url>,
    revocation: Option<Url>,
    introspection: Option<Url>,
    device: Option<Url>,
    pub(crate) scopes: Option<Vec<String>>,
    grants: Vec<String>,
    pub(crate) signing_algorithms: Vec<String>,
}

impl Provider {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn supports_client_credentials(&self) -> bool {
        self.grants.iter().any(|g| g == "client_credentials")
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

    pub fn revocation_endpoint(&self) -> Option<&Url> {
        self.revocation.as_ref()
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
#[derive(Clone)]
pub struct DiscoveryClient {
    pub(crate) client: Client,
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
        let document = self.fetch(&issuer).await?;
        self.validate(issuer, document)
    }

    async fn fetch(&self, issuer: &str) -> Result<Document, Error> {
        let endpoint = format!("{issuer}/.well-known/openid-configuration");
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| Error::DiscoveryUnavailable)?;
        let document = bounded_json(response).await?;
        Ok(document)
    }

    /// Cache public discovery metadata for 24 hours in an owner-only atomic file.
    /// Invalid/expired cache entries are refetched; refresh after an unknown key ID
    /// always uses `discover` directly, bypassing this cache.
    pub async fn discover_cached(
        &self,
        issuer: &str,
        cache: &std::path::Path,
    ) -> Result<Provider, Error> {
        let issuer = self.issuer(issuer)?;
        let path = cache.join(format!(
            "oidc-{:x}.json",
            sha2::Sha256::digest(issuer.as_bytes())
        ));
        if let Some(cached) = read_cache(&path)
            && cached.version == 1
            && cached.checked_at <= crate::tokens::now()
            && crate::tokens::now().saturating_sub(cached.checked_at) < 24 * 60 * 60
            && let Ok(provider) = self.validate(issuer.clone(), cached.document)
        {
            return Ok(provider);
        }
        let document = self.fetch(&issuer).await?;
        let provider = self.validate(issuer, document.clone())?;
        let bytes = serde_json::to_vec(&Cached {
            version: 1,
            checked_at: crate::tokens::now(),
            document,
        })
        .map_err(|_| Error::InvalidDiscovery)?;
        let _ = write_cache(cache, &path, &bytes);
        Ok(provider)
    }

    fn issuer(&self, issuer: &str) -> Result<String, Error> {
        let url = Url::parse(issuer).map_err(|_| Error::InvalidIssuer)?;
        if !self.transport.permits(&url) || url.query().is_some() {
            return Err(Error::InvalidIssuer);
        }
        Ok(url.as_str().trim_end_matches('/').to_owned())
    }

    pub(crate) fn endpoint(&self, endpoint: String) -> Result<Url, Error> {
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
            // Token issuer checks use the exact discovery issuer. Only the
            // configured/discovered URL comparison normalizes a trailing slash.
            issuer: doc.issuer,
            authorization: self.endpoint(doc.authorization_endpoint)?,
            token: self.endpoint(doc.token_endpoint)?,
            jwks: self.endpoint(doc.jwks_uri)?,
            userinfo: optional(doc.userinfo_endpoint)?,
            end_session: optional(doc.end_session_endpoint)?,
            revocation: optional(doc.revocation_endpoint)?,
            introspection: optional(doc.introspection_endpoint)?,
            device: optional(doc.device_authorization_endpoint)?,
            scopes: doc.scopes_supported,
            signing_algorithms: doc.id_token_signing_alg_values_supported,
            grants: doc
                .grant_types_supported
                .unwrap_or_else(|| vec!["authorization_code".into()]),
        })
    }
}

/// Bounded JSON without reflecting bodies or credential-bearing URLs in errors.
pub(crate) async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, Error> {
    if !response.status().is_success() {
        return Err(Error::DiscoveryUnavailable);
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOCUMENT as u64)
    {
        return Err(Error::DiscoveryTooLarge);
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
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
    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidDiscovery)
}

#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    checked_at: u64,
    document: Document,
}
fn read_cache(path: &std::path::Path) -> Option<Cached> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_DOCUMENT as u64 * 2 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT as u64 * 2 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_DOCUMENT * 2 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn write_cache(
    cache: &std::path::Path,
    path: &std::path::Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(cache)?;
    let mut file = tempfile::NamedTempFile::new_in(cache)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
