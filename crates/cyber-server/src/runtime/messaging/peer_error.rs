//! Failures of the direct peer transport (`cross-session-messaging` → Direct peer
//! transport, Peer grants). Each is a tagged error: the HTTP mapping is in
//! `http::error`, and its fields (`peer`, `cause`, `reason`, `scope`) are in the body.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Why a peer refused this machine's request, or why its certificate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeerAuthReason {
    /// The peer presented a certificate whose SPKI hash is not the pinned fingerprint;
    /// no request body was sent.
    FingerprintMismatch,
    /// The peer answered `401`: the token is unknown, revoked or expired.
    Unauthorized,
    /// The peer answered `403 PeerScopeError`: the grant lacks the route's scope.
    Scope,
}

impl PeerAuthReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FingerprintMismatch => "fingerprint_mismatch",
            Self::Unauthorized => "unauthorized",
            Self::Scope => "scope",
        }
    }
}

/// A grant scope (`message`, `attach`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PeerScope {
    Message,
    Attach,
}

impl PeerScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Attach => "attach",
        }
    }

    pub fn parse(scope: &str) -> Option<Self> {
        match scope {
            "message" => Some(Self::Message),
            "attach" => Some(Self::Attach),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    /// The peer did not answer in time, the connection or TLS failed, or it answered
    /// `5xx`; nothing is queued for later delivery.
    #[error("PeerUnreachableError: peer `{peer}` is unreachable: {cause}")]
    Unreachable { peer: String, cause: String },
    /// The peer's certificate or its answer refused this machine.
    #[error("PeerAuthError: peer `{peer}` refused the request ({})", reason.as_str())]
    Auth {
        peer: String,
        reason: PeerAuthReason,
    },
    /// `X@P` names a peer `P` that no `peers.remotes` entry has; nothing was sent.
    #[error(
        "PeerNotConfiguredError: no peer named `{peer}` is configured; pair it with `cyber peers add {peer} '<code>'`"
    )]
    NotConfigured { peer: String },
    /// On the peer listener: the grant does not include the route's scope.
    #[error("PeerScopeError: this grant does not include the `{}` scope", scope.as_str())]
    Scope { scope: PeerScope },
}

/// The `_tag` of a direct peer failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[allow(clippy::enum_variant_names)]
pub enum PeerErrorTag {
    PeerUnreachableError,
    PeerAuthError,
    PeerNotConfiguredError,
    PeerScopeError,
}

impl PeerErrorTag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PeerUnreachableError => "PeerUnreachableError",
            Self::PeerAuthError => "PeerAuthError",
            Self::PeerNotConfiguredError => "PeerNotConfiguredError",
            Self::PeerScopeError => "PeerScopeError",
        }
    }
}

impl PeerError {
    /// The `_tag` of the error body.
    pub fn tag(&self) -> PeerErrorTag {
        match self {
            Self::Unreachable { .. } => PeerErrorTag::PeerUnreachableError,
            Self::Auth { .. } => PeerErrorTag::PeerAuthError,
            Self::NotConfigured { .. } => PeerErrorTag::PeerNotConfiguredError,
            Self::Scope { .. } => PeerErrorTag::PeerScopeError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_start_with_the_tag() {
        for error in [
            PeerError::Unreachable {
                peer: "desk".into(),
                cause: "timed out".into(),
            },
            PeerError::Auth {
                peer: "desk".into(),
                reason: PeerAuthReason::FingerprintMismatch,
            },
            PeerError::NotConfigured { peer: "nas".into() },
            PeerError::Scope {
                scope: PeerScope::Attach,
            },
        ] {
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("{}: ", error.tag().as_str())),
                "{error}"
            );
        }
    }
}
