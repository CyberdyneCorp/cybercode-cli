//! The stored login: one owner-only document (`<data>/copilot-auth.json`, mode 0600,
//! replaced atomically, refused when other users can read it), kept by the shared
//! [`crate::subscription::LoginFile`]. It holds the GitHub host, the GitHub OAuth token and
//! the GitHub user. The token does not rotate, so no refresh lock is needed; minted Copilot
//! tokens are never written here.
use std::path::{Path, PathBuf};

use cyber_core::env::EnvSource;
use serde::{Deserialize, Serialize};

use super::http::visible;
use super::{CopilotError, Host, auth_path};
use crate::subscription::{Credential, LoginFile};

const VERSION: u32 = 1;

/// One signed-in GitHub account. `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    /// `github.com`, or the GitHub Enterprise hostname signed in to.
    pub host: String,
    pub github_token: String,
    /// The GitHub login name.
    pub user: String,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("host", &self.host)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl Login {
    /// The validated host.
    pub fn host(&self) -> Result<Host, CopilotError> {
        Host::parse(&self.host)
    }
}

impl Credential for Login {
    fn valid(&self) -> bool {
        Host::parse(&self.host).is_ok_and(|host| host.as_str() == self.host)
            && visible(&self.github_token)
            && visible(&self.user)
    }
}

/// The stored-login document.
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

    /// `<data>/copilot-auth.json` of the data directory `env` resolves to; `None` when `env`
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
    pub fn load(&self) -> Result<Option<Login>, CopilotError> {
        Ok(self.file.load()?)
    }

    /// The stored login, read on the blocking pool; [`CopilotError::NotSignedIn`] when there
    /// is none.
    pub async fn load_blocking(&self) -> Result<Login, CopilotError> {
        Ok(self.file.load_blocking().await?)
    }

    /// Replace the stored login atomically with an owner-only file.
    pub fn save(&self, login: &Login) -> Result<(), CopilotError> {
        if cfg!(not(unix)) {
            return Err(CopilotError::Storage(
                "GitHub Copilot login is not supported on Windows yet",
            ));
        }
        Ok(self.file.save(login)?)
    }

    /// Forget the login; whether one was stored. The GitHub token is not revoked.
    pub fn remove(&self) -> Result<bool, CopilotError> {
        Ok(self.file.remove()?)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn login() -> Login {
        Login {
            host: "github.com".into(),
            github_token: "gho_secret".into(),
            user: "octocat".into(),
        }
    }

    #[test]
    fn save_is_owner_only_and_a_widened_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = AuthFile::new(dir.path().join("copilot-auth.json"));
        assert_eq!(file.load(), Ok(None));
        file.save(&login()).unwrap();
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["host"], "github.com");
        assert_eq!(raw["user"], "octocat");
        assert_eq!(file.load().unwrap(), Some(login()));
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(file.load().is_err());
        assert_eq!(file.remove(), Ok(true));
        assert_eq!(file.remove(), Ok(false));
    }

    #[test]
    fn a_login_with_an_unsafe_host_or_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = AuthFile::new(dir.path().join("copilot-auth.json"));
        for (host, token) in [
            ("evil.example/x", "t"),
            ("GitHub.com", "t"),
            ("github.com", "a b"),
        ] {
            let body = serde_json::json!({"version": 1, "host": host,
                "github_token": token, "user": "u"});
            cyber_core::trust::write_private_atomic(file.path(), body.to_string().as_bytes())
                .unwrap();
            assert!(file.load().is_err(), "{host} {token}");
        }
    }

    #[test]
    fn debug_hides_the_token() {
        assert!(!format!("{:?}", login()).contains("gho_secret"));
    }
}
