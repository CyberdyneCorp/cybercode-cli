use crate::discovery::bounded_json;
use crate::tokens::{Audience, Claims, now};
use crate::{Account, CodeGrant, DiscoveryClient, Error, Provider, Secret, TokenSet, Transport};
use jsonwebtoken::jwk::{Jwk, JwkSet, KeyOperations, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

const KEY_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_TOKEN: usize = 64 * 1024;

/// Client state is kept behind one account/service mutex. Credentials never live
/// in discovery metadata, key caches or error values.
#[derive(Clone)]
pub struct OidcClient {
    pub(crate) discovery: DiscoveryClient,
    provider: Provider,
    client_id: String,
    keys: JwkSet,
    fetched: Instant,
}

impl OidcClient {
    pub async fn discover(
        issuer: &str,
        client_id: &str,
        transport: Transport,
    ) -> Result<Self, Error> {
        Self::discover_with_cache(issuer, client_id, transport, None).await
    }

    pub async fn discover_with_cache(
        issuer: &str,
        client_id: &str,
        transport: Transport,
        cache: Option<&std::path::Path>,
    ) -> Result<Self, Error> {
        if client_id.trim().is_empty() {
            return Err(Error::InvalidClient);
        }
        let discovery = DiscoveryClient::new(transport)?;
        let provider = match cache {
            Some(cache) => discovery.discover_cached(issuer, cache).await?,
            None => discovery.discover(issuer).await?,
        };
        let response = discovery
            .client
            .get(provider.jwks_uri().clone())
            .send()
            .await
            .map_err(|_| Error::DiscoveryUnavailable)?;
        let keys = bounded_json(response).await?;
        Ok(Self {
            discovery,
            provider,
            client_id: client_id.to_owned(),
            keys,
            fetched: Instant::now(),
        })
    }

    /// Metadata/key cache freshness only; cloning this client copies no account or
    /// authorization lease. Keep the newest snapshot after concurrent verification.
    pub fn keys_fetched_at(&self) -> Instant {
        self.fetched
    }

    /// Obtain a backend-only service credential. This result is deliberately a
    /// different type from user identity and cannot construct an IdentityLease.
    pub async fn client_credentials(
        &self,
        client_id: &str,
        secret: &Secret,
        scope: Option<&str>,
    ) -> Result<ServiceCredential, Error> {
        if client_id.trim().is_empty() || secret.expose().is_empty() {
            return Err(Error::InvalidClient);
        }
        if !self.provider.supports_client_credentials() {
            return Err(Error::ServiceCredentialsUnsupported);
        }
        let started = Instant::now();
        let issued = now();
        let mut form = vec![("grant_type", "client_credentials")];
        if let Some(scope) = scope {
            form.push(("scope", scope));
        }
        let response = self
            .discovery
            .client
            .post(self.provider.token_endpoint().clone())
            .basic_auth(client_id, Some(secret.expose()))
            .form(&form)
            .send()
            .await
            .map_err(|_| Error::TokenExchange)?;
        let response = token_response(response).await?;
        let lifetime = response.expires_in.unwrap_or(60).min(3600);
        if lifetime == 0 {
            return Err(Error::InvalidToken);
        }
        Ok(ServiceCredential {
            token: response.access_token,
            expires_at: issued.saturating_add(lifetime),
            deadline: started + Duration::from_secs(lifetime),
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    async fn reload_keys(&mut self) -> Result<(), Error> {
        let provider = self.discovery.discover(self.provider.issuer()).await?;
        let response = self
            .discovery
            .client
            .get(provider.jwks_uri().clone())
            .send()
            .await
            .map_err(|_| Error::DiscoveryUnavailable)?;
        let keys = bounded_json(response).await?;
        self.provider = provider;
        self.keys = keys;
        self.fetched = Instant::now();
        Ok(())
    }

    async fn claims(
        &mut self,
        token: &str,
        audience: &str,
        strict_expiry: bool,
    ) -> Result<Claims, Error> {
        if audience.trim().is_empty() {
            return Err(Error::InvalidAudience);
        }
        if token.len() > MAX_TOKEN {
            return Err(Error::InvalidToken);
        }
        let header = decode_header(token).map_err(|_| Error::InvalidToken)?;
        if !asymmetric(header.alg) {
            return Err(Error::InvalidToken);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or(Error::UnknownSigningKey)?;
        if self.fetched.elapsed() >= KEY_TTL
            || !self
                .keys
                .keys
                .iter()
                .any(|k| k.common.key_id.as_deref() == Some(kid))
        {
            self.reload_keys().await?;
        }
        let mut matching = self
            .keys
            .keys
            .iter()
            .filter(|k| k.common.key_id.as_deref() == Some(kid));
        let key = matching.next().ok_or(Error::UnknownSigningKey)?;
        if matching.next().is_some()
            || !key_permits(key, header.alg, &self.provider.signing_algorithms)
        {
            return Err(Error::InvalidToken);
        }
        let key = DecodingKey::from_jwk(key).map_err(|_| Error::InvalidToken)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.provider.issuer()]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = if strict_expiry { 0 } else { 60 };
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| Error::InvalidToken)?
            .claims;
        let current = now();
        if claims.sub.trim().is_empty()
            || claims.iat > current.saturating_add(60)
            || claims.iat > claims.exp
            || (strict_expiry && claims.exp <= current)
            || claims
                .auth_time
                .is_some_and(|t| t > current.saturating_add(60))
        {
            return Err(Error::InvalidToken);
        }
        Ok(claims)
    }

    pub async fn verify_id_token(&mut self, token: &str, nonce: &str) -> Result<Account, Error> {
        if nonce.is_empty() {
            return Err(Error::InvalidToken);
        }
        let client_id = self.client_id.clone();
        let claims = self.claims(token, &client_id, false).await?;
        if claims
            .nonce
            .as_deref()
            .is_none_or(|n| !bool::from(n.as_bytes().ct_eq(nonce.as_bytes())))
            || claims.azp.as_ref().is_some_and(|azp| azp != &client_id)
            || (claims.aud.multiple() && claims.azp.as_deref() != Some(&client_id))
        {
            return Err(Error::InvalidToken);
        }
        Ok(claims.account(&client_id))
    }

    /// Consume the PKCE grant. Successful exchange alone is insufficient: both
    /// token signatures, identity binding, access audience and ID nonce are checked.
    pub async fn exchange(
        &mut self,
        grant: CodeGrant,
        access_audience: &str,
    ) -> Result<TokenSet, Error> {
        if grant.issuer() != self.provider.issuer()
            || grant.token_endpoint() != self.provider.token_endpoint()
        {
            return Err(Error::IssuerMismatch);
        }
        let form = grant.token_form()?;
        if form
            .iter()
            .find(|(k, _)| *k == "client_id")
            .map(|(_, v)| *v)
            != Some(self.client_id.as_str())
        {
            return Err(Error::InvalidClient);
        }
        let response = self
            .discovery
            .client
            .post(grant.token_endpoint().clone())
            .form(&form)
            .send()
            .await
            .map_err(|_| Error::TokenExchange)?;
        let response = token_response(response).await?;
        let id_token = response.id_token.ok_or(Error::InvalidToken)?;
        let id_account = self
            .verify_id_token(id_token.expose(), grant.expected_nonce())
            .await?;
        if grant.requires_recent_authentication()
            && !id_account
                .auth_time
                .is_some_and(|time| time <= now() && now().saturating_sub(time) <= 900)
        {
            return Err(Error::StepUpRequired);
        }
        let nonce = Some(Secret::new(grant.expected_nonce().to_owned()));
        self.bind_tokens(
            response.access_token,
            response.refresh_token,
            id_token,
            id_account,
            access_audience,
            nonce,
        )
        .await
    }

    async fn bind_tokens(
        &mut self,
        access: Secret,
        refresh: Option<Secret>,
        id: Secret,
        id_account: Account,
        audience: &str,
        nonce: Option<Secret>,
    ) -> Result<TokenSet, Error> {
        let claims = self.claims(access.expose(), audience, true).await?;
        if claims.sub != id_account.sub || claims.iss != id_account.issuer {
            return Err(Error::InvalidToken);
        }
        let mut account = claims.account(&self.client_id);
        account.email = id_account.email.or(account.email);
        account.name = id_account.name.or(account.name);
        // Refresh/access issuance cannot manufacture a newer authentication time.
        account.auth_time = id_account.auth_time;
        account.amr = id_account.amr;
        Ok(TokenSet {
            access_token: access,
            refresh_token: refresh,
            id_token: id,
            client_id: self.client_id.clone(),
            nonce,
            account,
        })
    }

    pub(crate) async fn accept_device_tokens(
        &mut self,
        response: reqwest::Response,
        audience: &str,
    ) -> Result<TokenSet, Error> {
        let response = token_response(response).await?;
        let id = response.id_token.ok_or(Error::InvalidToken)?;
        let client_id = self.client_id.clone();
        let claims = self.claims(id.expose(), &client_id, false).await?;
        if claims.azp.as_ref().is_some_and(|s| s != &client_id)
            || (claims.aud.multiple() && claims.azp.as_deref() != Some(&client_id))
        {
            return Err(Error::InvalidToken);
        }
        self.bind_tokens(
            response.access_token,
            response.refresh_token,
            id,
            claims.account(&client_id),
            audience,
            None,
        )
        .await
    }

    /// Local deletion is mandatory and happens in the caller before these best-effort
    /// network requests. Credentials stay in POST forms, not query strings/logs.
    pub async fn revoke_logout(&self, tokens: &TokenSet) {
        if tokens.account.issuer != self.provider.issuer() || tokens.client_id != self.client_id {
            return;
        }
        if let Some(endpoint) = self.provider.revocation_endpoint()
            && let Some(refresh) = &tokens.refresh_token
        {
            let _ = self
                .discovery
                .client
                .post(endpoint.clone())
                .form(&[
                    ("token", refresh.expose()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", self.client_id()),
                ])
                .send()
                .await;
        }
        if let Some(endpoint) = self.provider.end_session_endpoint() {
            let _ = self
                .discovery
                .client
                .post(endpoint.clone())
                .form(&[("id_token_hint", tokens.id_token.expose())])
                .send()
                .await;
        }
    }

    pub async fn refresh(&mut self, old: &TokenSet, audience: &str) -> Result<TokenSet, Error> {
        if old.account.issuer != self.provider.issuer() || old.client_id != self.client_id {
            return Err(Error::IssuerMismatch);
        }
        let refresh = old.refresh_token.as_ref().ok_or(Error::SessionExpired)?;
        let response = self
            .discovery
            .client
            .post(self.provider.token_endpoint().clone())
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("refresh_token", refresh.expose()),
            ])
            .send()
            .await
            .map_err(|_| Error::TokenExchange)?;
        let response = token_response(response).await?;
        let claims = self
            .claims(response.access_token.expose(), audience, true)
            .await?;
        if claims.sub != old.account.sub || claims.iss != old.account.issuer {
            return Err(Error::InvalidToken);
        }
        if let Some(id) = &response.id_token {
            let client_id = self.client_id.clone();
            let refreshed_id = self.claims(id.expose(), &client_id, false).await?;
            if refreshed_id.sub != old.account.sub
                || refreshed_id.iss != old.account.issuer
                || refreshed_id.azp.as_ref().is_some_and(|s| s != &client_id)
                || (refreshed_id.aud.multiple() && refreshed_id.azp.as_deref() != Some(&client_id))
                || refreshed_id.nonce.as_ref().is_some_and(|s| {
                    old.nonce
                        .as_ref()
                        .is_some_and(|n| !bool::from(s.as_bytes().ct_eq(n.expose().as_bytes())))
                })
            {
                return Err(Error::InvalidToken);
            }
        }
        let mut account = claims.account(&self.client_id);
        account.auth_time = old.account.auth_time;
        account.amr = old.account.amr.clone();
        account.email = old.account.email.clone();
        account.name = old.account.name.clone();
        Ok(TokenSet {
            access_token: response.access_token,
            refresh_token: response.refresh_token.or_else(|| old.refresh_token.clone()),
            id_token: response.id_token.unwrap_or_else(|| old.id_token.clone()),
            client_id: self.client_id.clone(),
            nonce: old.nonce.clone(),
            account,
        })
    }

    /// Always performs uncached live introspection. A caller can retain the returned
    /// lease for ordinary reads for <=60 seconds; enrollment/admin operations must
    /// call this again. Never use JWT organization snapshots to replace live `orgs`.
    pub async fn authorize_human(
        &mut self,
        access: &Secret,
        service: &Secret,
        audience: &str,
    ) -> Result<IdentityLease, Error> {
        let started = Instant::now();
        let signed = self.claims(access.expose(), audience, true).await?;
        if !has_scope(&signed.scope, "cyber:collaborate") {
            return Err(Error::MissingCollaborationScope);
        }
        let endpoint = self
            .provider
            .introspection_endpoint()
            .ok_or(Error::LiveValidationUnavailable)?;
        let response = self
            .discovery
            .client
            .post(endpoint.clone())
            .bearer_auth(service.expose())
            .form(&[
                ("token", access.expose()),
                ("token_type_hint", "access_token"),
            ])
            .send()
            .await
            .map_err(|_| Error::LiveValidationUnavailable)?;
        let live: Introspection = bounded_json(response)
            .await
            .map_err(|_| Error::LiveValidationUnavailable)?;
        let current = now();
        if !live.active
            || live.token_type.as_deref() != Some("user")
            || live.sub.as_deref() != Some(signed.sub.as_str())
            || !live.aud.as_ref().is_some_and(|a| a.contains(audience))
            || !has_scope(
                live.scope.as_deref().unwrap_or_default(),
                "cyber:collaborate",
            )
            || live.exp.is_none_or(|e| e <= current)
        {
            return Err(Error::InactiveIdentity);
        }
        let expires_at = signed.exp.min(live.exp.unwrap_or(0));
        let deadline = (started + Duration::from_secs(60))
            .min(Instant::now() + Duration::from_secs(expires_at.saturating_sub(current)));
        if Instant::now() >= deadline {
            return Err(Error::InactiveIdentity);
        }
        let mut account = signed.account(&self.client_id);
        account.orgs = live.orgs;
        account.org = live.org;
        account.auth_time = live.auth_time;
        account.amr = live.amr;
        account.expires_at = expires_at;
        Ok(IdentityLease { account, deadline })
    }
}

fn asymmetric(alg: Algorithm) -> bool {
    matches!(
        alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::EdDSA
    )
}

fn key_permits(key: &Jwk, algorithm: Algorithm, advertised: &[String]) -> bool {
    let name = format!("{algorithm:?}");
    advertised.contains(&name)
        && key
            .common
            .key_algorithm
            .is_none_or(|alg| alg.to_string() == name)
        && key
            .common
            .public_key_use
            .as_ref()
            .is_none_or(|u| *u == PublicKeyUse::Signature)
        && key
            .common
            .key_operations
            .as_ref()
            .is_none_or(|ops| ops.contains(&KeyOperations::Verify))
}

fn has_scope(scope: &str, expected: &str) -> bool {
    scope.split_whitespace().any(|s| s == expected)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Secret,
    refresh_token: Option<Secret>,
    id_token: Option<Secret>,
    token_type: String,
    expires_in: Option<u64>,
}

async fn token_response(response: reqwest::Response) -> Result<TokenResponse, Error> {
    if !response.status().is_success() {
        #[derive(Deserialize)]
        struct Denial {
            error: String,
        }
        let error: Denial = bounded_json_allow_error(response).await?;
        return Err(if error.error == "invalid_grant" {
            Error::SessionExpired
        } else {
            Error::TokenExchange
        });
    }
    let response: TokenResponse = bounded_json(response)
        .await
        .map_err(|_| Error::TokenExchange)?;
    if !response.token_type.eq_ignore_ascii_case("Bearer")
        || response.access_token.expose().is_empty()
    {
        return Err(Error::InvalidToken);
    }
    Ok(response)
}

async fn bounded_json_allow_error<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, Error> {
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::TokenExchange)? {
        if chunk.len() > crate::discovery::MAX_DOCUMENT.saturating_sub(bytes.len()) {
            return Err(Error::TokenExchange);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::TokenExchange)
}

#[derive(Deserialize)]
struct Introspection {
    active: bool,
    token_type: Option<String>,
    sub: Option<String>,
    aud: Option<Audience>,
    exp: Option<u64>,
    scope: Option<String>,
    #[serde(default)]
    orgs: Vec<crate::Organization>,
    org: Option<crate::Organization>,
    auth_time: Option<u64>,
    #[serde(default)]
    amr: Vec<String>,
}

/// Private backend credential, never a human account and never an API response.
#[derive(Clone, Debug)]
pub struct ServiceCredential {
    pub token: Secret,
    pub expires_at: u64,
    deadline: Instant,
}
impl ServiceCredential {
    pub fn needs_refresh(&self) -> bool {
        self.deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(30)
            || self.expires_at <= now().saturating_add(30)
    }
}

/// Non-serializable authorization evidence. Any derived lease must keep this exact
/// deadline (or an earlier one); creating another TTL would stack stale authority.
pub struct IdentityLease {
    account: Account,
    deadline: Instant,
}

impl IdentityLease {
    pub fn account(&self) -> &Account {
        &self.account
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn is_current(&self) -> bool {
        Instant::now() < self.deadline && now() < self.account.expires_at
    }
    pub fn require_organization(&self, organization: &str) -> Result<(), Error> {
        if self.is_current() && self.account.orgs.iter().any(|o| o.id == organization) {
            Ok(())
        } else {
            Err(Error::InactiveIdentity)
        }
    }
    pub fn require_step_up(&self, managed_mfa: bool) -> Result<(), Error> {
        let current = now();
        if !self.is_current()
            || self
                .account
                .auth_time
                .is_none_or(|t| t > current || current.saturating_sub(t) > 900)
            || (managed_mfa
                && !(self.account.amr.iter().any(|s| s == "mfa")
                    || (self.account.amr.iter().any(|s| s == "pwd")
                        && self.account.amr.iter().any(|s| s == "otp"))))
        {
            return Err(Error::StepUpRequired);
        }
        Ok(())
    }
}
