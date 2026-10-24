//! Hub identity proofs. Cached reads retain the original authorization deadline;
//! enrollment and other sensitive changes always perform live introspection.
use crate::error::Error;
use cyber_account::{IdentityLease, OidcClient, Secret, ServiceCredential, Transport};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const MAX_IDENTITIES: usize = 1024;

/// Backend-only credentials, never accepted as a human project identity.
#[derive(Clone)]
pub enum BackendCredentials {
    Token(Secret),
    Client {
        client_id: String,
        client_secret: Secret,
        scope: Option<String>,
    },
}
struct Record {
    token: Secret,
    proof: Arc<IdentityLease>,
}
#[derive(Default)]
struct Cache {
    tokens: BTreeMap<String, Record>,
    subjects: BTreeMap<(String, String), String>,
}
struct Inner {
    verifier: Mutex<OidcClient>,
    issuer: String,
    audience: String,
    credentials: BackendCredentials,
    service: Mutex<Option<ServiceCredential>>,
    cache: Mutex<Cache>,
}
#[derive(Clone)]
pub struct Auth {
    inner: Arc<Inner>,
}
impl Auth {
    pub async fn new(
        issuer: &str,
        audience: &str,
        credentials: BackendCredentials,
        allow_loopback: bool,
    ) -> Result<Self, Error> {
        if audience.trim().is_empty() {
            return Err(cyber_account::Error::InvalidAudience.into());
        }
        match &credentials {
            BackendCredentials::Token(secret) if secret.expose().is_empty() => {
                return Err(Error::invalid(
                    "backend introspection credential is required",
                ));
            }
            BackendCredentials::Client {
                client_id,
                client_secret,
                ..
            } if client_id.trim().is_empty() || client_secret.expose().is_empty() => {
                return Err(Error::invalid("backend client credentials are required"));
            }
            _ => {}
        }
        // This client verifies access tokens only. Its client ID selects claims for
        // the configured resource audience; human login uses its own public client.
        let verifier = OidcClient::discover(
            issuer,
            audience,
            if allow_loopback {
                Transport::AllowLoopbackHttp
            } else {
                Transport::Https
            },
        )
        .await?;
        let issuer = verifier.provider().issuer().to_owned();
        Ok(Self {
            inner: Arc::new(Inner {
                verifier: Mutex::new(verifier),
                issuer,
                audience: audience.to_owned(),
                credentials,
                service: Mutex::new(None),
                cache: Mutex::new(Cache::default()),
            }),
        })
    }
    pub fn issuer(&self) -> &str {
        &self.inner.issuer
    }
    pub fn audience(&self) -> &str {
        &self.inner.audience
    }

    /// `fresh` is mandatory for sensitive mutations. No result extends a previous
    /// lease, and a failed live check removes the corresponding cached authority.
    pub async fn authenticate(
        &self,
        token: Secret,
        fresh: bool,
    ) -> Result<Arc<IdentityLease>, Error> {
        let key = fingerprint(&token);
        {
            let mut cache = self.inner.cache.lock().await;
            cache.prune();
            if !fresh
                && let Some(record) = cache.tokens.get(&key)
                && record.proof.is_current()
            {
                return Ok(record.proof.clone());
            }
        }
        let mut verifier = self.inner.verifier.lock().await.clone();
        let result = async {
            let service = self.backend_token(&verifier).await?;
            verifier
                .authorize_human(&token, &service, &self.inner.audience)
                .await
                .map_err(Error::from)
        }
        .await;
        // Different humans do not serialize network introspection behind the JWKS
        // cache lock. Preserve a key refresh performed by any successful request.
        {
            let mut cached = self.inner.verifier.lock().await;
            if verifier.keys_fetched_at() > cached.keys_fetched_at() {
                *cached = verifier;
            }
        }
        let mut cache = self.inner.cache.lock().await;
        match result {
            Ok(proof) => {
                if !proof.is_current() {
                    cache.remove(&key);
                    return Err(cyber_account::Error::InactiveIdentity.into());
                }
                cache.prune();
                if cache.tokens.len() >= MAX_IDENTITIES
                    && !cache.tokens.contains_key(&key)
                    && let Some(old) = cache.tokens.keys().next().cloned()
                {
                    cache.remove(&old);
                }
                let proof = Arc::new(proof);
                cache.subjects.insert(
                    (proof.account().issuer.clone(), proof.account().sub.clone()),
                    key.clone(),
                );
                cache.tokens.insert(
                    key,
                    Record {
                        token,
                        proof: proof.clone(),
                    },
                );
                Ok(proof)
            }
            Err(error) => {
                cache.remove(&key);
                Err(error)
            }
        }
    }
    /// Revalidate a source/recipient using a previously authenticated credential.
    /// Missing, expired or revoked credentials never fall back to a JWT snapshot.
    pub async fn subject(
        &self,
        issuer: &str,
        sub: &str,
        fresh: bool,
    ) -> Result<Arc<IdentityLease>, Error> {
        if issuer != self.inner.issuer {
            return Err(cyber_account::Error::InactiveIdentity.into());
        }
        let token = {
            let mut cache = self.inner.cache.lock().await;
            cache.prune();
            cache
                .subjects
                .get(&(issuer.to_owned(), sub.to_owned()))
                .and_then(|key| cache.tokens.get(key))
                .map(|record| record.token.clone())
        }
        .ok_or_else(|| Error::from(cyber_account::Error::InactiveIdentity))?;
        self.authenticate(token, fresh).await
    }
    async fn backend_token(&self, verifier: &OidcClient) -> Result<Secret, Error> {
        match &self.inner.credentials {
            BackendCredentials::Token(token) => Ok(token.clone()),
            BackendCredentials::Client {
                client_id,
                client_secret,
                scope,
            } => {
                let mut service = self.inner.service.lock().await;
                if service
                    .as_ref()
                    .is_none_or(ServiceCredential::needs_refresh)
                {
                    *service = Some(
                        verifier
                            .client_credentials(client_id, client_secret, scope.as_deref())
                            .await?,
                    );
                }
                service
                    .as_ref()
                    .map(|credential| credential.token.clone())
                    .ok_or_else(|| Error::from(cyber_account::Error::LiveValidationUnavailable))
            }
        }
    }
}
impl Cache {
    fn remove(&mut self, key: &str) {
        self.tokens.remove(key);
        self.subjects.retain(|_, value| value != key);
    }
    fn prune(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        self.tokens
            .retain(|_, record| record.proof.account().expires_at > now);
        self.subjects.retain(|_, key| self.tokens.contains_key(key));
    }
}
fn fingerprint(token: &Secret) -> String {
    format!("{:x}", Sha256::digest(token.expose().as_bytes()))
}

#[cfg(test)]
use crate::fixture;
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    #[tokio::test]
    async fn cached_proofs_keep_the_original_deadline_and_fresh_checks_observe_revocation() {
        let f = fixture::Fixture::start().await;
        let auth = Auth::new(
            &f.state.issuer,
            "hub",
            BackendCredentials::Token(f.service()),
            true,
        )
        .await
        .unwrap();
        let alice = f.user("alice", json!([]));
        let bob = f.user("bob", json!([]));
        let original = auth.authenticate(alice.clone(), false).await.unwrap();
        let cached = auth.authenticate(alice.clone(), false).await.unwrap();
        assert!(Arc::ptr_eq(&original, &cached));
        assert_eq!(original.deadline(), cached.deadline());
        assert_eq!(f.state.checks.load(Ordering::SeqCst), 1);
        auth.authenticate(bob, false).await.unwrap();
        f.state.users.lock().unwrap().get_mut("alice").unwrap()["active"] = json!(false);
        assert!(auth.subject(&f.state.issuer, "alice", true).await.is_err());
        assert!(auth.authenticate(alice, false).await.is_err());
        assert!(auth.subject(&f.state.issuer, "bob", true).await.is_ok());
        assert!(
            auth.subject("https://other.example", "bob", true)
                .await
                .is_err()
        );
        assert!(
            auth.subject(&f.state.issuer, "mallory", true)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn backend_client_credentials_are_private_shared_and_not_human_authority() {
        let f = fixture::Fixture::start().await;
        let auth = Auth::new(
            &f.state.issuer,
            "hub",
            BackendCredentials::Client {
                client_id: "backend".into(),
                client_secret: Secret::new("service-secret".into()),
                scope: None,
            },
            true,
        )
        .await
        .unwrap();
        let alice = f.user("alice", json!([]));
        let bob = f.user("bob", json!([]));
        let (a, b) = tokio::join!(auth.authenticate(alice, true), auth.authenticate(bob, true));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(f.state.service_tokens.load(Ordering::SeqCst), 1);
        assert!(auth.authenticate(f.service(), true).await.is_err());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    #[tokio::test]
    async fn source_revalidation_observes_live_org_withdrawal_and_auth_outage() {
        let f = fixture::Fixture::start().await;
        let auth = Auth::new(
            &f.state.issuer,
            "hub",
            BackendCredentials::Token(f.service()),
            true,
        )
        .await
        .unwrap();
        let token = f.user("alice", json!([{"id":"org-uuid","short_name":"team"}]));
        auth.authenticate(token, false)
            .await
            .unwrap()
            .require_organization("org-uuid")
            .unwrap();
        f.state.users.lock().unwrap().get_mut("alice").unwrap()["orgs"] = json!([]);
        let fresh = auth.subject(&f.state.issuer, "alice", true).await.unwrap();
        assert!(fresh.require_organization("org-uuid").is_err());
        f.task.abort();
        assert!(auth.subject(&f.state.issuer, "alice", true).await.is_err());
        assert!(auth.subject(&f.state.issuer, "alice", false).await.is_err());
    }
    #[tokio::test]
    async fn expired_source_credential_is_not_resurrected_by_subject_lookup() {
        let f = fixture::Fixture::start().await;
        let auth = Auth::new(
            &f.state.issuer,
            "hub",
            BackendCredentials::Token(f.service()),
            true,
        )
        .await
        .unwrap();
        let mut claims = f.claims();
        claims["exp"] = json!(fixture::now() + 1);
        let token = Secret::new(f.signing.sign(&claims));
        auth.authenticate(token, false).await.unwrap();
        let before = f.state.checks.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(auth.subject(&f.state.issuer, "alice", true).await.is_err());
        assert_eq!(f.state.checks.load(Ordering::SeqCst), before);
    }
}
