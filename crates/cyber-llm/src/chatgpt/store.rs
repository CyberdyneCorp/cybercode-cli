//! The stored login: one owner-only document (`<data>/chatgpt-auth.json`, mode 0600,
//! replaced atomically) and its cross-process refresh lock
//! (`<data>/chatgpt-auth.json.refresh.lock`), both kept by the shared
//! [`crate::subscription::LoginFile`].
//!
//! A refresh re-reads the document under the lock, so a login another process already
//! renewed is adopted without a request and a rotating refresh token is never presented
//! twice. The new tokens are written before the lock is released and before they are
//! used. A rejected refresh leaves the document as it is, so a concurrent
//! `cyber providers login` is never overwritten. A document readable by other users, not
//! regular or larger than 64 KiB is refused rather than trusted. Windows has no
//! owner-only file here yet, so storing a login is refused there.
use std::path::{Path, PathBuf};

use cyber_core::env::EnvSource;
use serde::{Deserialize, Serialize};

use super::oauth::{TokenResponse, visible};
use super::{ChatGptError, Claims, Issuer, auth_path, now, refresh};
use crate::subscription::{Credential, LoginFile, refresh_locked};

const VERSION: u32 = 1;
/// An access token expiring within this window is refreshed before it is used.
pub const REFRESH_WINDOW: u64 = 5 * 60;

/// One signed-in ChatGPT account. `Debug` never shows a token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Unix seconds; `None` means the token is never treated as due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("account_id", &self.account_id)
            .field("email", &self.email)
            .field("plan", &self.plan)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl Login {
    /// The login an authorization-code exchange produced. It needs all three tokens and a
    /// ChatGPT account ID.
    pub fn from_exchange(response: TokenResponse) -> Result<Self, ChatGptError> {
        let missing = |name| ChatGptError::Body(name);
        let access_token = response
            .access_token
            .ok_or_else(|| missing("no access token"))?;
        let refresh_token = response
            .refresh_token
            .ok_or_else(|| missing("no refresh token"))?;
        let id_token = response.id_token.ok_or_else(|| missing("no ID token"))?;
        let (id, access) = (Claims::of(&id_token), Claims::of(&access_token));
        Ok(Self {
            account_id: id
                .account_id
                .or(access.account_id)
                .ok_or(ChatGptError::NoAccount)?,
            email: id.email.or(access.email),
            plan: id.plan.or(access.plan),
            expires_at: expiry(access.expires_at, response.expires_in),
            access_token,
            refresh_token,
            id_token: Some(id_token),
        })
    }

    /// This login with what a refresh returned; omitted tokens keep their old values.
    pub fn refreshed(&self, response: TokenResponse) -> Result<Self, ChatGptError> {
        let access_token = response
            .access_token
            .ok_or(ChatGptError::Body("no access token"))?;
        let access = Claims::of(&access_token);
        let id = response
            .id_token
            .as_deref()
            .map(Claims::of)
            .unwrap_or_default();
        Ok(Self {
            account_id: id
                .account_id
                .or(access.account_id)
                .unwrap_or_else(|| self.account_id.clone()),
            email: id.email.or_else(|| self.email.clone()),
            plan: id.plan.or_else(|| self.plan.clone()),
            expires_at: expiry(access.expires_at, response.expires_in),
            refresh_token: response
                .refresh_token
                .unwrap_or_else(|| self.refresh_token.clone()),
            id_token: response.id_token.or_else(|| self.id_token.clone()),
            access_token,
        })
    }

    /// Whether the access token expires within [`REFRESH_WINDOW`] of `now`.
    pub fn due(&self, now: u64) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= now.saturating_add(REFRESH_WINDOW))
    }
}

/// The access token's own `exp`, else `expires_in` from now.
fn expiry(claimed: Option<u64>, expires_in: Option<u64>) -> Option<u64> {
    claimed.or_else(|| expires_in.map(|seconds| now().saturating_add(seconds)))
}

impl Credential for Login {
    fn valid(&self) -> bool {
        visible(&self.access_token) && visible(&self.refresh_token) && visible(&self.account_id)
    }
}

/// The stored-login document and its refresh lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFile {
    file: LoginFile<Login>,
}

impl AuthFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            file: LoginFile::new(path, VERSION),
        }
    }

    /// `<data>/chatgpt-auth.json` of the data directory `env` resolves to; `None` when `env`
    /// names neither a home nor a data directory.
    pub fn from_env(env: &dyn EnvSource) -> Option<Self> {
        let home = cyber_core::paths::home_dir(env);
        let relocated = ["CYBER_HOME", "XDG_DATA_HOME"]
            .iter()
            .any(|name| env.get(name).is_some());
        if home.is_none() && !relocated {
            return None;
        }
        let paths = cyber_core::paths::Paths::resolve(env, &home.unwrap_or_default());
        Some(Self::new(auth_path(&paths.data)))
    }

    pub fn path(&self) -> &Path {
        self.file.path()
    }

    pub fn exists(&self) -> bool {
        self.file.exists()
    }

    /// The stored login, or `None` when there is none.
    pub fn load(&self) -> Result<Option<Login>, ChatGptError> {
        Ok(self.file.load()?)
    }

    /// Replace the stored login atomically with an owner-only file.
    pub fn save(&self, login: &Login) -> Result<(), ChatGptError> {
        if cfg!(not(unix)) {
            return Err(ChatGptError::Storage(
                "ChatGPT login is not supported on Windows yet",
            ));
        }
        Ok(self.file.save(login)?)
    }

    /// Forget the login; whether one was stored. Tokens are not revoked remotely.
    pub fn remove(&self) -> Result<bool, ChatGptError> {
        Ok(self.file.remove()?)
    }

    /// The stored login with an access token outside its refresh window, renewing it under
    /// the refresh lock when needed. File operations run on the blocking pool.
    pub async fn fresh(
        &self,
        http: &reqwest::Client,
        issuer: &Issuer,
    ) -> Result<Login, ChatGptError> {
        refresh_locked(
            &self.file,
            |login: &Login| login.due(now()),
            |login| async move {
                let response = refresh(http, issuer, &login.refresh_token).await?;
                login.refreshed(response)
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::oauth::test_jwt;
    use super::*;

    fn login(expires_at: Option<u64>) -> Login {
        Login {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            id_token: None,
            account_id: "acct".into(),
            email: Some("a@b.c".into()),
            plan: Some("plus".into()),
            expires_at,
        }
    }

    #[test]
    fn exchange_needs_tokens_and_an_account() {
        let id = test_jwt(&json!({"email": "a@b.c",
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct", "chatgpt_plan_type": "pro"}}));
        let access = test_jwt(&json!({"exp": 100}));
        let response = TokenResponse {
            access_token: Some(access.clone()),
            refresh_token: Some("r".into()),
            id_token: Some(id),
            expires_in: None,
        };
        let stored = Login::from_exchange(response.clone()).unwrap();
        assert_eq!(stored.account_id, "acct");
        assert_eq!(stored.plan.as_deref(), Some("pro"));
        assert_eq!(stored.expires_at, Some(100));
        let no_account = TokenResponse {
            id_token: Some(test_jwt(&json!({}))),
            ..response.clone()
        };
        assert_eq!(
            Login::from_exchange(no_account),
            Err(ChatGptError::NoAccount)
        );
        let no_refresh = TokenResponse {
            refresh_token: None,
            ..response
        };
        assert!(Login::from_exchange(no_refresh).is_err());
    }

    #[test]
    fn refresh_keeps_what_the_response_omits() {
        let next = login(Some(1))
            .refreshed(TokenResponse {
                access_token: Some(test_jwt(&json!({"exp": 500}))),
                ..TokenResponse::default()
            })
            .unwrap();
        assert_eq!(next.refresh_token, "refresh");
        assert_eq!(next.account_id, "acct");
        assert_eq!(next.email.as_deref(), Some("a@b.c"));
        assert_eq!(next.expires_at, Some(500));
    }

    #[test]
    fn due_inside_the_five_minute_window() {
        assert!(!login(None).due(1_000));
        assert!(!login(Some(1_000 + REFRESH_WINDOW + 1)).due(1_000));
        assert!(login(Some(1_000 + REFRESH_WINDOW)).due(1_000));
    }

    #[cfg(unix)]
    #[test]
    fn save_is_owner_only_and_a_widened_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = AuthFile::new(dir.path().join("chatgpt-auth.json"));
        assert_eq!(file.load(), Ok(None));
        file.save(&login(Some(9))).unwrap();
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(file.load().unwrap(), Some(login(Some(9))));
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(file.load().is_err());
        assert_eq!(file.remove(), Ok(true));
        assert_eq!(file.remove(), Ok(false));
    }

    #[cfg(unix)]
    #[test]
    fn an_unknown_document_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = AuthFile::new(dir.path().join("chatgpt-auth.json"));
        cyber_core::trust::write_private_atomic(file.path(), br#"{"version":2}"#).unwrap();
        assert!(file.load().is_err());
    }
}
