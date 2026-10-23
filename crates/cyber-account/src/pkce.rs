use crate::{Error, Provider};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use url::{Host, Url};
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(300);
const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "cyber:relay",
    "cyber:runner",
    "cyber:share",
    "cyber:messaging",
    "cyber:collaborate",
];
const PARAMETERS: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "state",
    "nonce",
    "code_challenge",
    "code_challenge_method",
    "request",
    "request_uri",
];

#[derive(Debug, PartialEq, Eq)]
pub struct ScopeSelection {
    pub requested: Vec<String>,
    /// Callers must warn about omitted scopes; features still check the issued token.
    pub omitted: Vec<String>,
}

impl Provider {
    pub fn login_scopes(&self) -> Result<ScopeSelection, Error> {
        let supported = |scope: &&str| {
            self.scopes
                .as_ref()
                .is_none_or(|scopes| scopes.iter().any(|s| s == *scope))
        };
        let (requested, omitted): (Vec<_>, Vec<_>) = SCOPES.iter().copied().partition(supported);
        if !requested.contains(&"openid") {
            return Err(Error::OpenIdUnsupported);
        }
        Ok(ScopeSelection {
            requested: requested.into_iter().map(|s| s.to_string()).collect(),
            omitted: omitted.into_iter().map(|s| s.to_string()).collect(),
        })
    }
}

/// In-memory, single-use login state. Intentionally neither Debug nor Clone/Serialize.
/// Creating an attempt does not bind a listener or open a browser. The caller must
/// own the loopback listener at `redirect` before constructing it.
pub struct AuthorizationAttempt {
    authorization: Url,
    token_endpoint: Url,
    issuer: String,
    client_id: String,
    redirect: Url,
    verifier: Zeroizing<String>,
    state: Zeroizing<String>,
    nonce: Zeroizing<String>,
    started: Instant,
}

impl AuthorizationAttempt {
    pub fn new(provider: &Provider, client_id: &str, redirect: Url) -> Result<Self, Error> {
        if client_id.trim().is_empty() {
            return Err(Error::InvalidClient);
        }
        validate_redirect(&redirect)?;
        if provider
            .authorization
            .query_pairs()
            .any(|(k, _)| PARAMETERS.contains(&k.as_ref()))
        {
            return Err(Error::InvalidDiscovery);
        }
        let scopes = provider.login_scopes()?;
        let verifier = random::<64>()?;
        let state = random::<32>()?;
        let nonce = random::<32>()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization = provider.authorization.clone();
        authorization.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect.as_str()),
            ("scope", &scopes.requested.join(" ")),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        Ok(Self {
            authorization,
            token_endpoint: provider.token_endpoint().clone(),
            issuer: provider.issuer().to_owned(),
            client_id: client_id.to_owned(),
            redirect,
            verifier,
            state,
            nonce,
            started: Instant::now(),
        })
    }

    /// Intended only for the interactive login UI; never send this URL to models/logs.
    pub fn authorization_url(&self) -> &Url {
        &self.authorization
    }

    /// Accept a full callback URL from the owned listener or headless paste.
    /// Consumes the attempt, including on denial/error; no grant can be replayed here.
    /// Bare codes cannot prove state and are not accepted by this low-level API.
    pub fn finish(self, callback: &str) -> Result<CodeGrant, Error> {
        if self.started.elapsed() >= LIFETIME {
            return Err(Error::LoginExpired);
        }
        let mut callback = Url::parse(callback).map_err(|_| Error::InvalidCallback)?;
        let pairs: Vec<_> = callback
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        callback.set_query(None);
        if callback != self.redirect {
            return Err(Error::InvalidCallback);
        }
        let state = parameter(&pairs, "state")?.ok_or(Error::StateMismatch)?;
        if !bool::from(state.as_bytes().ct_eq(self.state.as_bytes())) {
            return Err(Error::StateMismatch);
        }
        let error = parameter(&pairs, "error")?;
        let code = parameter(&pairs, "code")?;
        match (code, error) {
            (None, Some(_)) => Err(Error::AuthorizationDenied),
            (Some(code), None) if !code.is_empty() => Ok(CodeGrant {
                code: Zeroizing::new(code.to_owned()),
                client_id: self.client_id,
                redirect: self.redirect,
                verifier: self.verifier,
                nonce: self.nonce,
                issuer: self.issuer,
                token_endpoint: self.token_endpoint,
                started: self.started,
            }),
            _ => Err(Error::InvalidCallback),
        }
    }
}

/// An authorization code plus its verifier, NOT an authenticated account.
/// The caller must exchange it, verify the ID-token signature/issuer/audience/
/// time/nonce and securely store the resulting tokens before completing login.
/// This type deliberately has no Debug/Clone/Serialize implementation.
pub struct CodeGrant {
    code: Zeroizing<String>,
    client_id: String,
    redirect: Url,
    verifier: Zeroizing<String>,
    nonce: Zeroizing<String>,
    issuer: String,
    token_endpoint: Url,
    started: Instant,
}

impl CodeGrant {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn token_endpoint(&self) -> &Url {
        &self.token_endpoint
    }

    /// Compare against the nonce in the verified ID token, never an unverified payload.
    pub fn expected_nonce(&self) -> &str {
        &self.nonce
    }

    /// Sensitive form data for a single token exchange; do not log or export.
    /// This contains no client secret because the CLI is a public client.
    pub fn token_form(&self) -> Result<[(&str, &str); 5], Error> {
        if self.started.elapsed() >= LIFETIME {
            return Err(Error::LoginExpired);
        }
        Ok([
            ("grant_type", "authorization_code"),
            ("client_id", &self.client_id),
            ("redirect_uri", self.redirect.as_str()),
            ("code", &self.code),
            ("code_verifier", &self.verifier),
        ])
    }
}

fn random<const N: usize>() -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new([0; N]);
    getrandom::fill(bytes.as_mut()).map_err(|_| Error::RandomUnavailable)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

fn validate_redirect(redirect: &Url) -> Result<(), Error> {
    let loopback = matches!(redirect.host(), Some(Host::Ipv4(ip)) if ip == std::net::Ipv4Addr::LOCALHOST)
        || matches!(redirect.host(), Some(Host::Ipv6(ip)) if ip == std::net::Ipv6Addr::LOCALHOST);
    if redirect.scheme() != "http"
        || !loopback
        || redirect.path() != "/callback"
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(Error::InvalidCallback);
    }
    Ok(())
}

fn parameter<'a>(pairs: &'a [(String, String)], name: &str) -> Result<Option<&'a str>, Error> {
    let mut values = pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.as_str());
    let first = values.next();
    if values.next().is_some() {
        return Err(Error::InvalidCallback);
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_attempt_cannot_release_a_code() {
        let attempt = AuthorizationAttempt {
            authorization: Url::parse("https://auth.test/authorize").unwrap(),
            token_endpoint: Url::parse("https://auth.test/token").unwrap(),
            issuer: "https://auth.test".into(),
            client_id: "cyber-cli".into(),
            redirect: Url::parse("http://127.0.0.1:12345/callback").unwrap(),
            verifier: Zeroizing::new("verifier".into()),
            state: Zeroizing::new("state".into()),
            nonce: Zeroizing::new("nonce".into()),
            started: Instant::now() - LIFETIME,
        };
        assert!(matches!(
            attempt.finish("http://127.0.0.1:12345/callback?state=state&code=code"),
            Err(Error::LoginExpired)
        ));
    }

    #[test]
    fn expired_grant_cannot_generate_exchange_parameters() {
        let grant = CodeGrant {
            code: Zeroizing::new("code".into()),
            client_id: "cyber-cli".into(),
            redirect: Url::parse("http://127.0.0.1:12345/callback").unwrap(),
            verifier: Zeroizing::new("verifier".into()),
            nonce: Zeroizing::new("nonce".into()),
            issuer: "https://auth.test".into(),
            token_endpoint: Url::parse("https://auth.test/token").unwrap(),
            started: Instant::now() - LIFETIME,
        };
        assert_eq!(grant.token_form().unwrap_err(), Error::LoginExpired);
    }
}
