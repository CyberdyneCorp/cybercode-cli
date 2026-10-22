//! GitHub Copilot subscription login (`provider-credentials` → GitHub Copilot subscription
//! login, `provider-catalog` → GitHub Copilot subscription provider).
//!
//! The sign-in is GitHub's OAuth device flow ([`request_code`], [`poll`]), so it also works
//! over SSH; the CLI prints the code and owns Ctrl+C, this module owns the requests. The
//! long-lived GitHub token is the stored credential ([`AuthFile`]). Each request uses a
//! short-lived Copilot API token minted from it, kept only in memory ([`Session::token`]).
//! The plan's models are listed by the Copilot API and cached ([`ModelsFile`]).
//! Diagnostics never contain a token or a device code.
mod device;
mod http;
mod models;
mod store;
mod token;

use std::path::{Path, PathBuf};

use cyber_core::env::EnvSource;
use reqwest::Url;

use crate::subscription::StoreError;

pub use device::{DeviceCode, poll, request_code};
pub use models::{CopilotModel, ModelsFile, Protocol, fetch as fetch_models};
pub use store::{AuthFile, Login};
pub use token::{CopilotToken, REUSE_WINDOW, Session, confirm, mint, user as fetch_user};

/// The provider ID the catalog lists the login under.
pub const PROVIDER_ID: &str = "copilot";
/// The GitHub host signed in to when `--enterprise` is not given.
pub const GITHUB_HOST: &str = "github.com";
/// The OAuth App client the device flow runs for. Provisional until task 1.1 confirms a
/// Cyber Code registered client is accepted by the token-mint endpoint (see `design.md`).
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// Overrides [`CLIENT_ID`].
pub const CLIENT_ID_ENV: &str = "CYBER_COPILOT_CLIENT_ID";
/// Overrides `https://<host>` (tests and proxies); HTTPS, or HTTP only on loopback.
pub const GITHUB_URL_ENV: &str = "CYBER_COPILOT_GITHUB_URL";
/// Overrides `https://api.<host>` (tests and proxies); HTTPS, or HTTP only on loopback.
pub const API_URL_ENV: &str = "CYBER_COPILOT_API_URL";
/// The Copilot API of individual plans; the catalog shows it until a token names the
/// account's own endpoint (`endpoints.api`), which requests always use.
pub const API_URL: &str = "https://api.githubcopilot.com";
/// Sent as `Copilot-Integration-Id`.
pub const INTEGRATION_ID: &str = "vscode-chat";
/// Sent as `Editor-Version` and `Editor-Plugin-Version`.
pub const EDITOR_VERSION: &str = concat!("cyber/", env!("CARGO_PKG_VERSION"));

/// Why a login, token mint or stored-login operation failed. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CopilotError {
    #[error("not signed in to GitHub Copilot; run cyber providers login copilot")]
    NotSignedIn,
    #[error("GitHub Copilot login expired or was revoked; run cyber providers login copilot")]
    Revoked,
    #[error("This GitHub account has no active Copilot subscription")]
    NoSubscription,
    #[error("GitHub sign-in was denied")]
    Denied,
    #[error("GitHub sign-in code expired; run cyber providers login copilot again")]
    Expired,
    #[error("GitHub sign-in failed: {0}")]
    Device(String),
    #[error("GitHub Copilot {stage} failed with HTTP {status}")]
    Status { stage: &'static str, status: u16 },
    #[error("GitHub Copilot {0} request failed")]
    Transport(&'static str),
    #[error("GitHub Copilot response is invalid: {0}")]
    Body(&'static str),
    #[error("GitHub Copilot login storage: {0}")]
    Storage(&'static str),
    #[error("GitHub Copilot login is being changed by another process; try again")]
    Busy,
    #[error("{0} must be an HTTPS URL (plain HTTP only on loopback)")]
    Insecure(&'static str),
    #[error("the GitHub Enterprise host must be a bare hostname such as example.ghe.com")]
    Host,
}

impl From<StoreError> for CopilotError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotSignedIn => Self::NotSignedIn,
            StoreError::Storage(what) => Self::Storage(what),
            StoreError::Busy => Self::Busy,
        }
    }
}

impl From<CopilotError> for crate::LlmError {
    fn from(error: CopilotError) -> Self {
        use crate::ErrorKind;
        let kind = match &error {
            CopilotError::NotSignedIn | CopilotError::Revoked | CopilotError::NoSubscription => {
                ErrorKind::Authentication
            }
            CopilotError::Status { status, .. } if *status >= 500 => ErrorKind::ProviderInternal,
            CopilotError::Transport(_) | CopilotError::Busy => ErrorKind::Transport,
            _ => ErrorKind::InvalidRequest,
        };
        crate::LlmError::new(kind, error.to_string())
    }
}

/// What the catalog hands a [`crate::ApiKind::CopilotChat`] endpoint: the stored login and
/// the protocol the requested model is served through.
#[derive(Debug, Clone)]
pub struct Route {
    pub session: Session,
    pub protocol: Protocol,
}

/// A GitHub host: `github.com`, or a bare GitHub Enterprise hostname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host(String);

impl Host {
    /// A bare DNS name: labels of ASCII letters, digits and `-`, with at least one dot,
    /// and no scheme, port, path or credentials. A name URLs would read as an IPv4
    /// address (`1.2.3.4`, `foo.123`, `a.0x1`) is refused, so both URLs of a parsed host
    /// always form.
    pub fn parse(value: &str) -> Result<Self, CopilotError> {
        let host = value.to_ascii_lowercase();
        let label_ok = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        let valid = host.len() <= 253 && host.contains('.') && host.split('.').all(label_ok);
        if valid
            && ["", "api."]
                .iter()
                .all(|prefix| domain_url(prefix, &host).is_ok())
        {
            Ok(Self(host))
        } else {
            Err(CopilotError::Host)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Host {
    fn default() -> Self {
        Self(GITHUB_HOST.into())
    }
}

/// Where the device flow and the Copilot token mint are sent for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Urls {
    /// `https://<host>`: device code and OAuth token endpoints.
    pub github: Url,
    /// `https://api.<host>`: the Copilot token mint.
    pub api: Url,
}

impl Urls {
    /// The URLs of `host`, or the [`GITHUB_URL_ENV`] and [`API_URL_ENV`] overrides.
    pub fn resolve(env: &dyn EnvSource, host: &Host) -> Result<Self, CopilotError> {
        Overrides::from_env(env)?.urls(host)
    }
}

/// Base URLs that replace `https://<host>` and `https://api.<host>` (tests and proxies).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub github: Option<Url>,
    pub api: Option<Url>,
}

impl Overrides {
    /// The [`GITHUB_URL_ENV`] and [`API_URL_ENV`] overrides; HTTPS, or HTTP only on loopback.
    pub fn from_env(env: &dyn EnvSource) -> Result<Self, CopilotError> {
        let pick = |name: &'static str| env.get(name).map(|value| secure(&value, name)).transpose();
        Ok(Self {
            github: pick(GITHUB_URL_ENV)?,
            api: pick(API_URL_ENV)?,
        })
    }

    /// Both URLs replaced (tests); HTTPS, or HTTP only on loopback.
    pub fn parse(github: &str, api: &str) -> Result<Self, CopilotError> {
        Ok(Self {
            github: Some(secure(github, GITHUB_URL_ENV)?),
            api: Some(secure(api, API_URL_ENV)?),
        })
    }

    /// The URLs of `host`, with these overrides applied.
    pub fn urls(&self, host: &Host) -> Result<Urls, CopilotError> {
        let pick = |set: &Option<Url>, prefix: &str| match set {
            Some(url) => Ok(url.clone()),
            None => domain_url(prefix, host.as_str()),
        };
        Ok(Urls {
            github: pick(&self.github, "")?,
            api: pick(&self.api, "api.")?,
        })
    }
}

/// `https://<prefix><host>`, only when it parses with `host` as a DNS name.
fn domain_url(prefix: &str, host: &str) -> Result<Url, CopilotError> {
    let url = Url::parse(&format!("https://{prefix}{host}")).map_err(|_| CopilotError::Host)?;
    match url.domain() {
        Some(_) => Ok(url),
        None => Err(CopilotError::Host),
    }
}

/// [`CLIENT_ID`], or the [`CLIENT_ID_ENV`] override.
pub fn client_id(env: &dyn EnvSource) -> String {
    env.get(CLIENT_ID_ENV)
        .filter(|id| http::visible(id))
        .unwrap_or_else(|| CLIENT_ID.into())
}

/// HTTPS, or plain HTTP only on loopback.
fn secure(value: &str, name: &'static str) -> Result<Url, CopilotError> {
    let url = Url::parse(value).map_err(|_| CopilotError::Insecure(name))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(CopilotError::Insecure(name)),
    }
}

/// `base` with `path` appended to its own path.
fn join(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    let prefix = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{prefix}/{path}"));
    url
}

/// `<data>/copilot-auth.json`.
pub fn auth_path(data: &Path) -> PathBuf {
    data.join("copilot-auth.json")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn enterprise_hosts_must_be_bare_hostnames() {
        assert_eq!(
            Host::parse("Octo.GHE.com").unwrap().as_str(),
            "octo.ghe.com"
        );
        assert_eq!(Host::default().as_str(), "github.com");
        for bad in [
            "https://octo.ghe.com",
            "octo.ghe.com:443",
            "octo.ghe.com/x",
            "user@octo.ghe.com",
            "localhost",
            "",
            "-a.com",
            "a..com",
            "a b.com",
            // Regression: URLs read these as IPv4 addresses, so they never form.
            "foo.123",
            "1.2.3.4",
            "a.0x1",
            "a.0X1F",
        ] {
            assert_eq!(Host::parse(bad), Err(CopilotError::Host), "{bad}");
        }
    }

    #[test]
    fn urls_follow_the_host_and_overrides_must_be_secure() {
        let host = Host::parse("octo.ghe.com").unwrap();
        let none = HashMap::<String, String>::new();
        let urls = Urls::resolve(&none, &host).unwrap();
        assert_eq!(urls.github.as_str(), "https://octo.ghe.com/");
        assert_eq!(urls.api.as_str(), "https://api.octo.ghe.com/");
        let local = HashMap::from([(GITHUB_URL_ENV.to_owned(), "http://127.0.0.1:9/gh".into())]);
        let urls = Urls::resolve(&local, &host).unwrap();
        assert_eq!(
            join(&urls.github, "login/device/code").as_str(),
            "http://127.0.0.1:9/gh/login/device/code"
        );
        let remote = HashMap::from([(API_URL_ENV.to_owned(), "http://api.example.com".into())]);
        assert_eq!(
            Urls::resolve(&remote, &host),
            Err(CopilotError::Insecure(API_URL_ENV))
        );
    }

    #[test]
    fn the_client_id_is_one_constant_with_an_override() {
        let none = HashMap::<String, String>::new();
        assert_eq!(client_id(&none), CLIENT_ID);
        let custom = HashMap::from([(CLIENT_ID_ENV.to_owned(), "Ov23custom".into())]);
        assert_eq!(client_id(&custom), "Ov23custom");
        let unsafe_id = HashMap::from([(CLIENT_ID_ENV.to_owned(), "a\nb".into())]);
        assert_eq!(client_id(&unsafe_id), CLIENT_ID);
    }

    #[test]
    fn errors_never_name_a_token_and_map_to_authentication() {
        let error: crate::LlmError = CopilotError::Revoked.into();
        assert_eq!(error.kind, crate::ErrorKind::Authentication);
        assert_eq!(
            error.message,
            "GitHub Copilot login expired or was revoked; run cyber providers login copilot"
        );
    }
}
