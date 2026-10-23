//! Account protocol building blocks, separate from provider credentials and local use.
//!
//! Discovery and PKCE preparation do not authenticate a user. Token exchange,
//! signature/nonce validation and protected storage must complete before login succeeds.

mod discovery;
mod pkce;

pub use discovery::{DiscoveryClient, Provider, Transport};
pub use pkce::{AuthorizationAttempt, CodeGrant, ScopeSelection};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("InvalidIssuerError: expected an HTTPS issuer without credentials, query or fragment")]
    InvalidIssuer,
    #[error("IssuerMismatchError: discovery does not match the configured issuer")]
    IssuerMismatch,
    #[error("InvalidDiscoveryError: malformed or unsafe provider metadata")]
    InvalidDiscovery,
    #[error("DiscoveryUnavailableError: provider request failed")]
    DiscoveryUnavailable,
    #[error("DiscoveryTooLargeError: provider metadata exceeds 256 KiB")]
    DiscoveryTooLarge,
    #[error("PkceUnsupportedError: provider must support S256 and authorization code login")]
    PkceUnsupported,
    #[error("MissingScopeError: provider does not advertise openid")]
    OpenIdUnsupported,
    #[error("InvalidClientError: client ID must be nonempty")]
    InvalidClient,
    #[error(
        "InvalidCallbackError: expected a loopback /callback URL and unique response parameters"
    )]
    InvalidCallback,
    #[error("OAuthStateMismatchError: callback state is missing or does not match")]
    StateMismatch,
    #[error("OAuthAuthorizationDeniedError: provider rejected authorization")]
    AuthorizationDenied,
    #[error("LoginExpiredError: start a new login attempt")]
    LoginExpired,
    #[error("RandomUnavailableError: secure random generation failed")]
    RandomUnavailable,
}
