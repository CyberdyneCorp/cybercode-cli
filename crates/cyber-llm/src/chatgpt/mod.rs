//! ChatGPT subscription login (`provider-credentials` → ChatGPT subscription login, ChatGPT
//! token refresh).
//!
//! The sign-in is OpenAI's OAuth authorization-code flow with PKCE, using the Codex public
//! client as other third-party harnesses do; requests then go to the Codex backend and are
//! paid by the user's ChatGPT plan. The interactive part (PKCE, `state`, the loopback
//! callback) belongs to the CLI; this module owns the requests, the stored login and its
//! refresh, so the adapter can renew a token right before it is used, and the list of
//! models the plan serves. Diagnostics never contain a token or an authorization code.
mod models;
mod oauth;
mod store;

use std::path::PathBuf;

use cyber_core::env::EnvSource;
use reqwest::Url;

use crate::subscription::StoreError;

pub use models::{CLIENT_VERSION, ModelsFile, PlanModel, fetch as fetch_models};
pub use oauth::{Claims, authorize_url, exchange_code, refresh};
pub use store::{AuthFile, Login};

/// The provider ID the catalog lists the login under.
pub const PROVIDER_ID: &str = "chatgpt";
/// OpenAI's OAuth issuer.
pub const ISSUER: &str = "https://auth.openai.com";
/// The Codex public client, the only one OpenAI offers for this flow.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// The loopback port registered for [`CLIENT_ID`].
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// The Codex backend that bills the ChatGPT plan.
pub const BACKEND_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Sent as `originator`, so requests are attributed to Cyber Code, never to Codex.
pub const ORIGINATOR: &str = "cyber";
/// Overrides [`ISSUER`] (tests and proxies); HTTPS, or HTTP only on loopback.
pub const ISSUER_ENV: &str = "CYBER_CHATGPT_ISSUER";
/// Overrides [`BACKEND_URL`] (tests and proxies); HTTPS, or HTTP only on loopback.
pub const BACKEND_ENV: &str = "CYBER_CHATGPT_BACKEND";

/// Why a login, refresh or stored-login operation failed. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChatGptError {
    #[error("not signed in to ChatGPT; run cyber providers login chatgpt")]
    NotSignedIn,
    #[error("ChatGPT login expired or was revoked; run cyber providers login chatgpt")]
    Rejected(String),
    #[error("This OpenAI account has no ChatGPT workspace")]
    NoAccount,
    #[error("ChatGPT {stage} failed with HTTP {status}")]
    Status { stage: &'static str, status: u16 },
    #[error("ChatGPT {0} request failed")]
    Transport(&'static str),
    #[error("ChatGPT token response is invalid: {0}")]
    Body(&'static str),
    #[error("ChatGPT login storage: {0}")]
    Storage(&'static str),
    #[error("ChatGPT login is being refreshed by another process; try again")]
    Busy,
    #[error("{0} must be an HTTPS URL (plain HTTP only on loopback)")]
    Insecure(&'static str),
}

impl From<StoreError> for ChatGptError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotSignedIn => Self::NotSignedIn,
            StoreError::Storage(what) => Self::Storage(what),
            StoreError::Busy => Self::Busy,
        }
    }
}

impl From<ChatGptError> for crate::LlmError {
    fn from(error: ChatGptError) -> Self {
        use crate::ErrorKind;
        let kind = match &error {
            ChatGptError::NotSignedIn | ChatGptError::Rejected(_) | ChatGptError::NoAccount => {
                ErrorKind::Authentication
            }
            ChatGptError::Status { status, .. } if *status >= 500 => ErrorKind::ProviderInternal,
            ChatGptError::Transport(_) | ChatGptError::Busy => ErrorKind::Transport,
            _ => ErrorKind::InvalidRequest,
        };
        crate::LlmError::new(kind, error.to_string())
    }
}

/// Where the login lives, which issuer renews it and which backend serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub file: AuthFile,
    pub issuer: Issuer,
    pub backend: String,
}

impl Session {
    /// The stored login of the data directory `env` names, or `None` when there is none.
    /// An invalid override is reported on stderr and disables the provider.
    pub fn stored(env: &dyn EnvSource) -> Option<Self> {
        let file = AuthFile::from_env(env)?;
        if !file.exists() {
            return None;
        }
        match Issuer::from_env(env).and_then(|issuer| Ok((issuer, backend_url(env)?))) {
            Ok((issuer, backend)) => Some(Self {
                file,
                issuer,
                backend,
            }),
            Err(error) => {
                eprintln!("warning: {error}; the chatgpt provider is unavailable");
                None
            }
        }
    }

    /// The model-list cache beside the login.
    pub fn models(&self) -> ModelsFile {
        ModelsFile::beside(self.file.path())
    }
}

/// [`BACKEND_URL`], or the [`BACKEND_ENV`] override.
pub fn backend_url(env: &dyn EnvSource) -> Result<String, ChatGptError> {
    let value = env.get(BACKEND_ENV).unwrap_or_else(|| BACKEND_URL.into());
    secure(&value, BACKEND_ENV)?;
    Ok(value.trim_end_matches('/').to_owned())
}

/// HTTPS, or plain HTTP only on loopback.
fn secure(value: &str, name: &'static str) -> Result<Url, ChatGptError> {
    let url = Url::parse(value).map_err(|_| ChatGptError::Insecure(name))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(ChatGptError::Insecure(name)),
    }
}

/// The OAuth issuer base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issuer(Url);

impl Issuer {
    pub fn parse(value: &str) -> Result<Self, ChatGptError> {
        secure(value, ISSUER_ENV).map(Self)
    }

    /// [`ISSUER`], or the [`ISSUER_ENV`] override.
    pub fn from_env(env: &dyn EnvSource) -> Result<Self, ChatGptError> {
        Self::parse(&env.get(ISSUER_ENV).unwrap_or_else(|| ISSUER.into()))
    }

    pub fn authorize_endpoint(&self) -> Url {
        self.join("oauth/authorize")
    }

    pub fn token_endpoint(&self) -> Url {
        self.join("oauth/token")
    }

    fn join(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/{path}"));
        url
    }
}

impl Default for Issuer {
    fn default() -> Self {
        Self::parse(ISSUER).expect("constant issuer is valid")
    }
}

/// `<data>/chatgpt-auth.json`.
pub fn auth_path(data: &std::path::Path) -> PathBuf {
    data.join("chatgpt-auth.json")
}

pub(crate) use crate::subscription::now;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_must_be_https_or_loopback_http() {
        assert!(Issuer::parse("https://auth.openai.com").is_ok());
        assert!(Issuer::parse("http://127.0.0.1:9999").is_ok());
        assert!(Issuer::parse("http://localhost:9999/base").is_ok());
        assert_eq!(
            Issuer::parse("http://auth.example.com"),
            Err(ChatGptError::Insecure(ISSUER_ENV))
        );
        assert_eq!(
            Issuer::parse("nope"),
            Err(ChatGptError::Insecure(ISSUER_ENV))
        );
    }

    #[test]
    fn issuer_endpoints_keep_a_base_path() {
        let issuer = Issuer::parse("http://127.0.0.1:1/base/").unwrap();
        assert_eq!(
            issuer.token_endpoint().as_str(),
            "http://127.0.0.1:1/base/oauth/token"
        );
        assert_eq!(
            Issuer::default().authorize_endpoint().as_str(),
            "https://auth.openai.com/oauth/authorize"
        );
    }
}
