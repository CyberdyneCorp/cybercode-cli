//! Account protocol building blocks, separate from provider credentials and local use.
//!
//! Discovery and PKCE preparation do not authenticate a user. Token exchange,
//! signature/nonce validation and protected storage must complete before login succeeds.

mod config;
mod device;
mod discovery;
mod loopback;
mod pkce;
mod store;
mod tokens;
mod verification;

pub use config::AccountSettings;
pub use device::DeviceAuthorization;
pub use discovery::{DiscoveryClient, Provider, Transport};
pub use loopback::{CallbackHandle, LoopbackLogin};
pub use pkce::{AuthorizationAttempt, CodeGrant, ScopeSelection};
pub use store::{
    AccountKey, AccountStore, CredentialVault, HostSecrets, StorageKind, credential_vault,
};
pub use tokens::{Account, Organization, Secret, TokenSet};
pub use verification::{IdentityLease, OidcClient, ServiceCredential};

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
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
    #[error("AccountStorageError: {0}")]
    Storage(&'static str),
    #[error("AccountBusyError: another process is updating the account")]
    Busy,
    #[error("NotLoggedInError: run cyber login")]
    NotLoggedIn,
    #[error("InvalidAudienceError: a nonempty API audience is required")]
    InvalidAudience,
    #[error("InvalidTokenError: signature or required token claims are invalid")]
    InvalidToken,
    #[error("UnknownSigningKeyError: signing key is not in the issuer key set")]
    UnknownSigningKey,
    #[error("SessionExpiredError: run cyber login again")]
    SessionExpired,
    #[error("ServiceCredentialsUnsupportedError: issuer does not advertise client_credentials")]
    ServiceCredentialsUnsupported,
    #[error("TokenExchangeError: token exchange failed")]
    TokenExchange,
    #[error("MissingScopeError: cyber:collaborate is required; run cyber login again")]
    MissingCollaborationScope,
    #[error("InactiveIdentityError: active human identity could not be verified")]
    InactiveIdentity,
    #[error("LiveValidationUnavailableError: cannot authorize without live introspection")]
    LiveValidationUnavailable,
    #[error("StepUpRequiredError: fresh authentication with the required factors is needed")]
    StepUpRequired,
    #[error("RandomUnavailableError: secure random generation failed")]
    RandomUnavailable,
}
