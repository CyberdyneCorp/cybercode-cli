//! Stored OAuth tokens on the remote transport: the bearer for one server at exactly its
//! canonical URL (never another URL, never looked up by name alone), refreshed through the
//! stored token endpoint with the RFC 8707 `resource`. A rotated refresh token is persisted
//! before the new access token is used, and only while the stored credential is still the
//! one that was refreshed. Refreshes of one credential are serialized across connections and
//! processes. A credential adopted from the store is validated like a loaded one, so its
//! tokens, client, issuer and token endpoint always travel together. Authorization
//! rechecks belong to the caller.
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use reqwest::header::HeaderValue;

use super::token::now;
use super::{
    AuthMarker, CredentialKey, McpCredentialStore, OAuthError, REQUEST_TIMEOUT, RefreshLock,
    StoredCredential, TokenRequest, TokenSet, refresh, resource_indicator, secure, secure_from,
    visible,
};

/// The longest refresh window: an access token expiring within it is refreshed before it
/// is used. A token issued for less than twice this long uses half its lifetime instead,
/// so a freshly issued token is never already due.
pub const REFRESH_WINDOW: Duration = Duration::from_secs(5 * 60);
/// A token younger than this is never due, whatever lifetime the server gave it.
const MIN_AGE: u64 = 10;
const LOCK_RETRY: Duration = Duration::from_millis(50);
/// How long a refresh waits for another holder's refresh of the same credential.
const LOCK_WAIT: Duration = Duration::from_secs(2 * REQUEST_TIMEOUT.as_secs());

/// One server's stored credential. `Debug` and diagnostics never show a token.
pub struct TokenSource {
    store: Arc<dyn McpCredentialStore>,
    key: CredentialKey,
    credential: StoredCredential,
    endpoint: Url,
    resource: Url,
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSource")
            .field("server", &self.key.server)
            .field("tokens", &self.credential.tokens)
            .finish_non_exhaustive()
    }
}

impl TokenSource {
    /// The credential stored for `server` at exactly `url`, or `None`. A server URL that is
    /// not HTTPS (plain HTTP only on loopback), a stored issuer or token endpoint that is no
    /// longer HTTPS (or loopback HTTP from a loopback referrer) or a token that cannot be
    /// sent as a header fails closed, so no bearer crosses the network in cleartext. The
    /// store is read on the blocking pool (see [`Self::blocking`]), so the caller's
    /// deadline still fires while an OS keyring read waits on the user.
    pub async fn load(
        store: Arc<dyn McpCredentialStore>,
        server: &str,
        url: &Url,
    ) -> Result<Option<Self>, OAuthError> {
        let (reader, name, address) = (Arc::clone(&store), server.to_owned(), url.to_string());
        let found = run_blocking(move || reader.find(&name, &address)).await?;
        let Some((key, credential)) = found else {
            return Ok(None);
        };
        let resource = resource_indicator(url);
        let endpoint = validated(&key, &credential, &resource)?;
        Ok(Some(Self {
            store,
            key,
            credential,
            endpoint,
            resource,
        }))
    }

    /// Whether a refresh token is held, so a refused access token may be renewed.
    pub fn refreshable(&self) -> bool {
        self.credential.tokens.refresh_token.is_some()
    }

    /// Whether the access token is inside its refresh window.
    pub fn due(&self) -> bool {
        expiring(&self.credential.tokens, now())
    }

    /// The `Authorization` value, marked sensitive.
    pub fn authorization(&self) -> HeaderValue {
        let mut value =
            HeaderValue::from_str(&format!("Bearer {}", self.credential.tokens.access_token))
                .unwrap_or_else(|_| HeaderValue::from_static("Bearer"));
        value.set_sensitive(true);
        value
    }

    /// Record what a 403 `insufficient_scope` asked for: the union of the scopes this token
    /// was granted and `required`, for the next `cyber mcp auth` to request. Nothing here
    /// re-authorizes.
    pub fn require_scopes(&self, required: &[String]) -> Result<(), OAuthError> {
        let marker = AuthMarker {
            required_scopes: self
                .credential
                .tokens
                .scopes
                .iter()
                .chain(required)
                .cloned()
                .collect(),
            ..AuthMarker::default()
        };
        self.store.mark(&self.key.server, &self.key.url, &marker)
    }

    /// Whether the credential in use was removed from the store (`cyber mcp logout`). A
    /// store that cannot be read answers `false`, so nothing reconnects on its account. Only
    /// the store's index is consulted ([`McpCredentialStore::contains`]), never the OS
    /// keyring, so this never waits on the user.
    pub fn removed(&self) -> bool {
        matches!(
            self.store.contains(&self.key.server, &self.key.url),
            Ok(false)
        )
    }

    /// Adopt what the store now holds for this server and URL: tokens another connection
    /// rotated, or a credential `cyber mcp auth` replaced, validated like [`Self::load`].
    /// A removed credential (`cyber mcp logout`) fails with
    /// [`OAuthError::CredentialRemoved`], so it is never sent again, whether or not a
    /// reconnect follows. Whether the credential in use changed. The store is read on the
    /// blocking pool, so a caller's timeout still fires.
    pub async fn sync(&mut self) -> Result<bool, OAuthError> {
        let (key, stored) = self.stored_blocking().await?;
        self.adopt(key, stored)
    }

    /// Renew the access token while holding the store's refresh lock. A credential another
    /// holder already rotated and that is not due is adopted without a request; a removed
    /// credential fails closed. The response is persisted only if the stored credential is
    /// still the refreshed one; otherwise the user's removal or replacement wins. Store
    /// reads and writes run on the blocking pool (see [`Self::blocking`]), so a caller's
    /// timeout around this future still fires.
    pub async fn refresh(&mut self, http: &reqwest::Client) -> Result<(), OAuthError> {
        let lock = self.lock_refresh().await?;
        let (key, stored) = self.stored_blocking().await?;
        if self.adopt(key, stored)? && !self.due() {
            return Ok(());
        }
        let presented = self
            .credential
            .tokens
            .refresh_token
            .clone()
            .ok_or_else(|| OAuthError::TokenRejected("no_refresh_token".into()))?;
        let request = TokenRequest {
            client: &self.credential.client,
            endpoint: &self.endpoint,
            resource: &self.resource,
        };
        let tokens = refresh(http, &request, &presented, None).await?;
        self.persist(tokens, lock).await
    }

    /// Store the response. The refresh `lock` is released only once the write has ended,
    /// even when this future is dropped by a timeout meanwhile, so no other holder reads
    /// the rotated-away refresh token while the write is still in flight.
    async fn persist(&mut self, mut tokens: TokenSet, lock: RefreshLock) -> Result<(), OAuthError> {
        if tokens.scopes.is_empty() {
            tokens.scopes.clone_from(&self.credential.tokens.scopes);
        }
        let credential = StoredCredential {
            tokens,
            ..self.credential.clone()
        };
        let (key, previous, next) = (
            self.key.clone(),
            self.credential.tokens.clone(),
            credential.clone(),
        );
        let replaced = self
            .blocking(move |store| {
                let _lock = lock;
                store.replace(&key, &previous, &next)
            })
            .await?;
        if replaced {
            self.credential = credential;
            return Ok(());
        }
        let (key, stored) = self.stored_blocking().await?;
        self.adopt(key, stored)?;
        if self.due() {
            return Err(OAuthError::TokenRejected("credential_replaced".into()));
        }
        Ok(())
    }

    /// Replace the credential in use when the store holds another; whether it did.
    fn adopt(&mut self, key: CredentialKey, stored: StoredCredential) -> Result<bool, OAuthError> {
        if key == self.key && stored == self.credential {
            return Ok(false);
        }
        self.endpoint = validated(&key, &stored, &self.resource)?;
        self.key = key;
        self.credential = stored;
        Ok(true)
    }

    /// Run a store operation on the blocking pool: an OS keyring call may wait on the user
    /// (a Keychain prompt), which must neither stall a runtime worker nor outlast the
    /// caller's timeout.
    async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&dyn McpCredentialStore) -> Result<T, OAuthError> + Send + 'static,
    ) -> Result<T, OAuthError> {
        let store = Arc::clone(&self.store);
        run_blocking(move || operation(store.as_ref())).await
    }

    async fn stored_blocking(&self) -> Result<(CredentialKey, StoredCredential), OAuthError> {
        let (server, url) = (self.key.server.clone(), self.key.url.clone());
        self.blocking(move |store| store.find(&server, &url))
            .await?
            .ok_or(OAuthError::CredentialRemoved)
    }

    async fn lock_refresh(&self) -> Result<RefreshLock, OAuthError> {
        let deadline = tokio::time::Instant::now() + LOCK_WAIT;
        loop {
            if let Some(lock) = self
                .store
                .try_lock_refresh(&self.key.server, &self.key.url)?
            {
                return Ok(lock);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(OAuthError::Storage(
                    "credential refresh is held by another process".into(),
                ));
            }
            tokio::time::sleep(LOCK_RETRY).await;
        }
    }
}

/// Run `operation` on the blocking pool; dropping the future leaves it to finish there.
async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, OAuthError> + Send + 'static,
) -> Result<T, OAuthError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| OAuthError::Storage("credential store task failed".into()))?
}

/// The token endpoint of a stored credential: `resource` must be HTTPS (plain HTTP only
/// on loopback), its issuer usable from `resource` and the endpoint from the issuer, and
/// its access token a valid header value.
fn validated(
    key: &CredentialKey,
    credential: &StoredCredential,
    resource: &Url,
) -> Result<Url, OAuthError> {
    secure(resource)?;
    let invalid = |reason| OAuthError::Metadata(reason);
    let issuer = Url::parse(&key.issuer).map_err(|_| invalid("stored issuer is invalid"))?;
    secure_from(&issuer, resource)?;
    let endpoint = Url::parse(&credential.token_endpoint)
        .map_err(|_| invalid("stored token endpoint is invalid"))?;
    secure_from(&endpoint, &issuer)?;
    if !visible(&credential.tokens.access_token) {
        return Err(OAuthError::Storage("stored access token is invalid".into()));
    }
    Ok(endpoint)
}

/// Inside the window: [`REFRESH_WINDOW`], or half the token's lifetime when that is
/// shorter, and never within [`MIN_AGE`] of issue.
fn expiring(tokens: &TokenSet, now: u64) -> bool {
    let Some(expires_at) = tokens.expires_at else {
        return false;
    };
    let longest = REFRESH_WINDOW.as_secs();
    let Some(issued_at) = tokens.issued_at else {
        return expires_at <= now.saturating_add(longest);
    };
    let window = (expires_at.saturating_sub(issued_at) / 2).min(longest);
    now >= issued_at.saturating_add(MIN_AGE) && expires_at <= now.saturating_add(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(issued_at: Option<u64>, expires_at: Option<u64>) -> TokenSet {
        TokenSet {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            expires_at,
            issued_at,
            scopes: Vec::new(),
        }
    }

    #[test]
    fn a_short_lived_token_is_due_after_half_its_lifetime_not_on_issue() {
        let issued = 1_000_000;
        // A 300 s token (a common default lifetime) is not due when issued.
        let short = tokens(Some(issued), Some(issued + 300));
        assert!(!expiring(&short, issued));
        assert!(!expiring(&short, issued + 149));
        assert!(expiring(&short, issued + 150));
        // An hour-long token uses the full five-minute window.
        let long = tokens(Some(issued), Some(issued + 3600));
        assert!(!expiring(&long, issued + 3299));
        assert!(expiring(&long, issued + 3300));
        // A token the server issued already expiring is not renewed in a loop.
        let instant = tokens(Some(issued), Some(issued));
        assert!(!expiring(&instant, issued + MIN_AGE - 1));
        assert!(expiring(&instant, issued + MIN_AGE));
    }

    #[test]
    fn a_token_without_issue_time_uses_the_full_window() {
        let now = 1_000_000;
        assert!(!expiring(&tokens(None, Some(now + 301)), now));
        assert!(expiring(&tokens(None, Some(now + 300)), now));
        assert!(!expiring(&tokens(None, None), now));
    }
}
