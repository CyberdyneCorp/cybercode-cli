//! The short-lived Copilot API token, minted from the GitHub token at
//! `GET <api>/copilot_internal/v2/token`, and the session that keeps it in memory.
//!
//! Each process mints its own token and reuses it until [`REUSE_WINDOW`] before it expires;
//! it is never written to disk. A mint rejected with 401 means the GitHub token was revoked:
//! the request fails with the login hint and the stored login is left as it is.
use std::sync::Arc;

use reqwest::Url;
use serde_json::Value;

use super::http::{self, editor};
use super::{AuthFile, CopilotError, Login, ModelsFile, Overrides, Urls, join, secure};
use crate::subscription::now;

/// A token expiring within this window is minted again before it is used.
pub const REUSE_WINDOW: u64 = 5 * 60;
const RESPONSE_LIMIT: usize = 64 * 1024;

/// One minted Copilot API token. `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct CopilotToken {
    pub token: String,
    /// Unix seconds.
    pub expires_at: u64,
    /// The account's Copilot API base URL (`endpoints.api`).
    pub api: Url,
}

impl std::fmt::Debug for CopilotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotToken")
            .field("expires_at", &self.expires_at)
            .field("api", &self.api.as_str())
            .finish_non_exhaustive()
    }
}

impl CopilotToken {
    /// Whether the token is still outside [`REUSE_WINDOW`] of its expiry at `now`.
    pub fn usable(&self, now: u64) -> bool {
        self.expires_at > now.saturating_add(REUSE_WINDOW)
    }
}

/// Mint a Copilot API token from `github_token`. 401 is [`CopilotError::Revoked`]; 403 and
/// 404, or a response naming no API endpoint, are [`CopilotError::NoSubscription`].
pub async fn mint(
    client: &reqwest::Client,
    urls: &Urls,
    github_token: &str,
) -> Result<CopilotToken, CopilotError> {
    let request = editor(client.get(join(&urls.api, "copilot_internal/v2/token")))
        .header("authorization", format!("token {github_token}"));
    let (status, document) = http::send(request, "token", RESPONSE_LIMIT).await?;
    match status {
        200 => parse_token(&document),
        401 => Err(CopilotError::Revoked),
        403 | 404 => Err(CopilotError::NoSubscription),
        status => Err(CopilotError::Status {
            stage: "token",
            status,
        }),
    }
}

fn parse_token(document: &Value) -> Result<CopilotToken, CopilotError> {
    let api = document["endpoints"]["api"]
        .as_str()
        .ok_or(CopilotError::NoSubscription)?;
    Ok(CopilotToken {
        api: secure(api, "endpoints.api")?,
        token: http::token(document, "token").ok_or(CopilotError::Body("no Copilot token"))?,
        expires_at: document["expires_at"]
            .as_u64()
            .ok_or(CopilotError::Body("no token expiry"))?,
    })
}

/// The GitHub login name `github_token` belongs to (`GET <api>/user`).
pub async fn user(
    client: &reqwest::Client,
    urls: &Urls,
    github_token: &str,
) -> Result<String, CopilotError> {
    let request = editor(client.get(join(&urls.api, "user")))
        .header("authorization", format!("token {github_token}"));
    let (status, document) = http::send(request, "user", RESPONSE_LIMIT).await?;
    match status {
        200 => http::token(&document, "login").ok_or(CopilotError::Body("no GitHub user")),
        401 => Err(CopilotError::Revoked),
        status => Err(CopilotError::Status {
            stage: "user",
            status,
        }),
    }
}

/// What a finished device flow signs in to: the login to store and its first Copilot token.
/// Any rejected mint, or one naming no API endpoint, is [`CopilotError::NoSubscription`],
/// so nothing is stored for an account without a plan.
pub async fn confirm(
    client: &reqwest::Client,
    urls: &Urls,
    host: &super::Host,
    github_token: String,
) -> Result<(Login, CopilotToken), CopilotError> {
    let token = match mint(client, urls, &github_token).await {
        Err(CopilotError::Revoked) => Err(CopilotError::NoSubscription),
        other => other,
    }?;
    let login = Login {
        host: host.as_str().to_owned(),
        user: user(client, urls, &github_token).await?,
        github_token,
    };
    Ok((login, token))
}

/// The token minted for one GitHub token.
struct Minted {
    github_token: String,
    token: CopilotToken,
}

/// The stored login, where its host's endpoints are, and this process's minted token.
#[derive(Clone)]
pub struct Session {
    pub file: AuthFile,
    pub overrides: Overrides,
    minted: Arc<tokio::sync::Mutex<Option<Minted>>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("file", &self.file)
            .field("overrides", &self.overrides)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn new(file: AuthFile, overrides: Overrides) -> Self {
        Self {
            file,
            overrides,
            minted: Arc::default(),
        }
    }

    /// The stored login of the data directory `env` names, or `None` when there is none.
    /// An invalid override is reported on stderr and disables the provider.
    pub fn stored(env: &dyn cyber_core::env::EnvSource) -> Option<Self> {
        let file = AuthFile::from_env(env)?;
        if !file.exists() {
            return None;
        }
        match Overrides::from_env(env) {
            Ok(overrides) => Some(Self::new(file, overrides)),
            Err(error) => {
                eprintln!("warning: {error}; the copilot provider is unavailable");
                None
            }
        }
    }

    /// The model-list cache beside the login.
    pub fn models(&self) -> ModelsFile {
        ModelsFile::beside(self.file.path())
    }

    /// The protocol the cached model list names for `slug`; Chat Completions, which every
    /// model serves, when it is not listed.
    pub fn protocol(&self, slug: &str) -> super::Protocol {
        self.models()
            .load()
            .into_iter()
            .find(|model| model.slug == slug)
            .map_or(super::Protocol::Chat, |model| model.protocol)
    }

    /// The URLs of `login`'s host.
    pub fn urls(&self, login: &Login) -> Result<Urls, CopilotError> {
        self.overrides.urls(&login.host()?)
    }

    /// A Copilot API token for the stored login: the one this process minted while it is
    /// usable, else a new one. Concurrent callers share one mint.
    pub async fn token(&self, client: &reqwest::Client) -> Result<CopilotToken, CopilotError> {
        let login = self.file.load_blocking().await?;
        let mut minted = self.minted.lock().await;
        let reusable = minted
            .as_ref()
            .filter(|m| m.github_token == login.github_token && m.token.usable(now()));
        if let Some(reused) = reusable {
            return Ok(reused.token.clone());
        }
        let token = mint(client, &self.urls(&login)?, &login.github_token).await?;
        *minted = Some(Minted {
            github_token: login.github_token,
            token: token.clone(),
        });
        Ok(token)
    }

    /// Refresh the cached model list when it is missing or stale. A failure keeps the old
    /// list for another day.
    pub async fn refresh_models(&self, client: &reqwest::Client) {
        let models = self.models();
        if !models.stale() {
            return;
        }
        let fetched = match self.token(client).await {
            Ok(token) => super::fetch_models(client, &token).await,
            Err(error) => Err(error),
        };
        models.record(fetched);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_token_needs_a_secure_api_endpoint_and_an_expiry() {
        let token = parse_token(&json!({"token": "tid=1;exp=2", "expires_at": 2_000,
            "endpoints": {"api": "https://api.individual.githubcopilot.com"}}))
        .unwrap();
        assert_eq!(
            token.api.as_str(),
            "https://api.individual.githubcopilot.com/"
        );
        assert!(!format!("{token:?}").contains("tid="));
        assert_eq!(
            parse_token(&json!({"token": "t", "expires_at": 1})),
            Err(CopilotError::NoSubscription)
        );
        assert!(
            parse_token(&json!({"token": "t", "expires_at": 1,
                "endpoints": {"api": "http://evil.example"}}))
            .is_err()
        );
        assert!(
            parse_token(&json!({"token": "t",
                "endpoints": {"api": "https://api.githubcopilot.com"}}))
            .is_err()
        );
    }

    #[test]
    fn a_token_is_reused_until_five_minutes_before_expiry() {
        let token = CopilotToken {
            token: "t".into(),
            expires_at: 1_000 + REUSE_WINDOW + 1,
            api: Url::parse("https://api.githubcopilot.com").unwrap(),
        };
        assert!(token.usable(1_000));
        assert!(!token.usable(1_001));
    }
}
