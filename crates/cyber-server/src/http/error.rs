//! Tagged errors (`server-api` → Tagged error model).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Serialize;

use crate::runtime::messaging::{AddressError, PeerAuthReason, PeerError, PeerErrorTag, PeerScope};
use crate::runtime::{CommandPlanError, RuntimeError};

/// The JSON body of every declared failure. Some failures add `reason`, a machine-readable
/// refinement of `_tag`, and fields of their own such as `retryAfterMs` or `path`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ErrorBody {
    #[serde(rename = "_tag")]
    pub tag: String,
    pub message: String,
    /// `err_<id>` for `UnknownError`, logged server-side.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<Box<str>>,
    /// Paths for rewind conflicts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// Capability group unavailable in this host or phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<Box<str>>,
    /// `reason`, a machine-readable refinement of `_tag` (for example `loopback_only` or
    /// `schema`), and fields specific to a tag or reason, such as `retryAfterMs`, `limit`
    /// or `path`. Boxed to keep every `Result<_, ApiError>` small.
    #[serde(flatten)]
    pub details: Option<Box<serde_json::Map<String, serde_json::Value>>>,
}

/// `422` from `v1.command.run` when a review target cannot be resolved; nothing is admitted.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReviewTargetError {
    /// Always `ReviewTargetError`.
    #[serde(rename = "_tag")]
    pub tag: String,
    pub code: cyber_core::review::ErrorCode,
    pub message: String,
}

/// The body of a direct peer failure (`cross-session-messaging` → Direct peer transport):
/// `PeerUnreachableError` and `PeerAuthError` (`502`), `PeerNotConfiguredError` (`404`)
/// and, on the peer listener, `PeerScopeError` (`403`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PeerErrorBody {
    #[serde(rename = "_tag")]
    pub tag: PeerErrorTag,
    pub message: String,
    /// The remote name of `peers.remotes` (all but `PeerScopeError`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// What failed (`PeerUnreachableError`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// Why the peer refused (`PeerAuthError`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PeerAuthReason>,
    /// The scope the grant lacks (`PeerScopeError`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<PeerScope>,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
    /// `code` of a `ReviewTargetError`, added to the body when present.
    pub code: Option<cyber_core::review::ErrorCode>,
}

impl ApiError {
    pub fn new(status: StatusCode, tag: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                tag: tag.into(),
                message: message.into(),
                reference: None,
                paths: None,
                service: None,
                details: None,
            },
            code: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "InvalidRequestError", message)
    }

    pub fn not_found(tag: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, tag, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "ConflictError", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "UnauthorizedError",
            "missing or invalid credentials",
        )
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "ForbiddenError", message)
    }

    /// `503` for a capability group this host or configuration does not serve.
    pub fn service_unavailable(service: &str, message: impl Into<String>) -> Self {
        let mut error = Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            message,
        );
        error.body.service = Some(service.into());
        error
    }

    /// `429` `RateLimitedError`: the caller may retry after `retry_after_ms`.
    pub fn rate_limited(reason: &str, retry_after_ms: u64, message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "RateLimitedError", message)
            .with_reason(reason)
            .with_field("retryAfterMs", retry_after_ms)
    }

    pub fn with_reason(self, reason: &str) -> Self {
        self.with_field("reason", reason)
    }

    pub fn with_field(mut self, name: &str, value: impl Into<serde_json::Value>) -> Self {
        self.body
            .details
            .get_or_insert_default()
            .insert(name.into(), value.into());
        self
    }

    pub fn review_target(error: cyber_core::review::TargetError) -> Self {
        let mut e = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ReviewTargetError",
            error.message,
        );
        e.code = Some(error.code);
        e
    }

    /// An unexpected failure: the detail is logged under a reference, never returned.
    pub fn unknown(detail: impl std::fmt::Display) -> Self {
        let reference = cyber_core::ids::new_id("err");
        cyber_core::log::error(
            "http",
            &detail.to_string(),
            serde_json::json!({ "ref": reference }),
        );
        let mut e = Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "UnknownError",
            "internal error",
        );
        e.body.reference = Some(reference.into());
        e
    }
}

impl From<RuntimeError> for ApiError {
    fn from(e: RuntimeError) -> Self {
        let message = e.to_string();
        match e {
            RuntimeError::SessionNotFound(_) => Self::not_found("SessionNotFoundError", message),
            RuntimeError::PromptConflict(_) => Self::conflict(message),
            RuntimeError::BudgetExceeded { .. } => {
                Self::new(StatusCode::CONFLICT, "BudgetExceededError", message)
            }
            RuntimeError::Busy(_) => Self::new(StatusCode::CONFLICT, "SessionBusyError", message),
            RuntimeError::Invalid(m) => Self::invalid(m),
            RuntimeError::Conflict(m) => Self::conflict(m),
            RuntimeError::ShuttingDown => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServerShuttingDownError",
                message,
            ),
            RuntimeError::McpRequired(_) => {
                Self::new(StatusCode::CONFLICT, "McpRequiredError", message)
            }
            RuntimeError::ContextBlocked(_) => Self::conflict(message),
            RuntimeError::RewindConflict(paths) => {
                let mut err = Self::new(StatusCode::CONFLICT, "RewindConflictError", message);
                err.body.paths = Some(paths);
                err
            }
            RuntimeError::Address(e) => Self::address(e),
            RuntimeError::Peer(e) => e.into(),
            RuntimeError::PermissionDenied(m) => {
                Self::new(StatusCode::FORBIDDEN, "PermissionDeniedError", m)
            }
            RuntimeError::MessageTooLarge { size, limit } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "MessageTooLargeError",
                message,
            )
            .with_field("size", size)
            .with_field("limit", limit),
            RuntimeError::Disabled(service) => Self::service_unavailable(service, message),
            RuntimeError::GoalNotFound(_) => Self::not_found("GoalNotFoundError", message),
            RuntimeError::GoalQueueFull(_) => {
                Self::new(StatusCode::CONFLICT, "GoalQueueFullError", message)
            }
            RuntimeError::GoalCondition(m) => {
                Self::new(StatusCode::BAD_REQUEST, "GoalConditionTooLongError", m)
            }
            RuntimeError::MessageNotFound(_) => Self::not_found("MessageNotFoundError", message),
            RuntimeError::LoopNotFound(_) => Self::not_found("LoopNotFoundError", message),
            RuntimeError::LoopLimit(_) => {
                Self::new(StatusCode::CONFLICT, "LoopLimitError", message)
            }
            RuntimeError::FeatureDisabled(m) => Self::not_found("FeatureDisabledError", m),
            RuntimeError::TeamNotFound(_) => Self::not_found("TeamNotFoundError", message),
            RuntimeError::TeamLimit(_) => {
                Self::new(StatusCode::CONFLICT, "TeamLimitError", message)
            }
            RuntimeError::TaskClaimed(_) => {
                Self::new(StatusCode::CONFLICT, "TaskClaimedError", message)
            }
            RuntimeError::TaskDependency(_) => {
                Self::new(StatusCode::CONFLICT, "TaskDependencyError", message)
            }
            RuntimeError::MessageImmutable(_) => {
                Self::conflict(message).with_reason("message_immutable")
            }
            // A quota frees up when a subscription fires or expires, not after a delay.
            RuntimeError::RateLimited { reason, .. } => Self::rate_limited(reason, 60_000, message),
            RuntimeError::Forbidden { reason, message } => {
                Self::forbidden(message).with_reason(reason)
            }
            RuntimeError::Store(cyber_store::StoreError::Busy) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                message,
            ),
            other => Self::unknown(other),
        }
    }
}

impl ApiError {
    /// A message address that resolves to no target; nothing was recorded.
    fn address(e: AddressError) -> Self {
        let message = e.to_string();
        match e {
            AddressError::NotFound(address) => {
                Self::not_found("AddressNotFoundError", message).with_field("address", address)
            }
            AddressError::Ambiguous {
                address,
                candidates,
            } => Self::new(StatusCode::CONFLICT, "AmbiguousAddressError", message)
                .with_field("address", address)
                .with_field(
                    "candidates",
                    serde_json::to_value(candidates).unwrap_or_default(),
                ),
            AddressError::AccountRequired(address) => {
                Self::new(StatusCode::FORBIDDEN, "AccountRequiredError", message)
                    .with_field("address", address)
            }
            AddressError::SelfTarget => {
                Self::invalid("Cannot send a message to the current Session")
                    .with_reason("self_target")
            }
        }
    }
}

impl PeerErrorBody {
    fn new(error: PeerError) -> Self {
        let mut body = Self {
            tag: error.tag(),
            message: error.to_string(),
            peer: None,
            cause: None,
            reason: None,
            scope: None,
        };
        match error {
            PeerError::Unreachable { peer, cause } => {
                body.peer = Some(peer);
                body.cause = Some(cause);
            }
            PeerError::Auth { peer, reason } => {
                body.peer = Some(peer);
                body.reason = Some(reason);
            }
            PeerError::NotConfigured { peer } => body.peer = Some(peer),
            PeerError::Scope { scope } => body.scope = Some(scope),
        }
        body
    }
}

impl From<PeerError> for ApiError {
    fn from(e: PeerError) -> Self {
        let status = match e {
            PeerError::Unreachable { .. } | PeerError::Auth { .. } => StatusCode::BAD_GATEWAY,
            PeerError::NotConfigured { .. } => StatusCode::NOT_FOUND,
            PeerError::Scope { .. } => StatusCode::FORBIDDEN,
        };
        let body = PeerErrorBody::new(e);
        let fields = serde_json::to_value(&body).unwrap_or_default();
        let mut error = Self::new(status, body.tag.as_str(), body.message);
        for (name, value) in fields.as_object().into_iter().flatten() {
            if !matches!(name.as_str(), "_tag" | "message") {
                error = error.with_field(name, value.clone());
            }
        }
        error
    }
}

impl From<CommandPlanError> for ApiError {
    fn from(e: CommandPlanError) -> Self {
        match e {
            CommandPlanError::Forbidden(message) => Self::forbidden(message),
            CommandPlanError::Review(error) => Self::review_target(error),
            CommandPlanError::Unavailable(message) => {
                let mut error = Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ServiceUnavailableError",
                    message,
                );
                error.body.service = Some("review".into());
                error
            }
            CommandPlanError::Invalid(message) => Self::invalid(message),
            CommandPlanError::McpUnavailable(message) => {
                let mut error = Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ServiceUnavailableError",
                    message,
                );
                error.body.service = Some("mcp".into());
                error
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let Some(code) = self.code else {
            return (self.status, Json(self.body)).into_response();
        };
        let mut body = serde_json::to_value(&self.body).unwrap_or_default();
        body["code"] = serde_json::json!(code);
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn body(error: impl Into<ApiError>) -> (u16, serde_json::Value) {
        let error = error.into();
        (
            error.status.as_u16(),
            serde_json::to_value(&error.body).unwrap(),
        )
    }

    /// Each peer failure keeps its tag, status and fields through the runtime error.
    #[test]
    fn peer_errors_map_to_tagged_bodies() {
        let peer = |e: PeerError| body(RuntimeError::Peer(e));
        let (status, b) = peer(PeerError::Unreachable {
            peer: "desk".into(),
            cause: "connect timed out after 5 s".into(),
        });
        assert_eq!(status, 502);
        assert_eq!(
            (&b["_tag"], &b["peer"], &b["cause"]),
            (
                &json!("PeerUnreachableError"),
                &json!("desk"),
                &json!("connect timed out after 5 s")
            )
        );
        let (status, b) = peer(PeerError::Auth {
            peer: "desk".into(),
            reason: PeerAuthReason::FingerprintMismatch,
        });
        assert_eq!(status, 502);
        assert_eq!(
            (&b["_tag"], &b["reason"]),
            (&json!("PeerAuthError"), &json!("fingerprint_mismatch"))
        );
        let (status, b) = peer(PeerError::NotConfigured { peer: "nas".into() });
        assert_eq!(status, 404);
        assert_eq!(
            (&b["_tag"], &b["peer"]),
            (&json!("PeerNotConfiguredError"), &json!("nas"))
        );
        assert!(
            b["message"]
                .as_str()
                .unwrap()
                .contains("cyber peers add nas")
        );
        let (status, b) = body(PeerError::Scope {
            scope: PeerScope::Attach,
        });
        assert_eq!(status, 403);
        assert_eq!(
            (&b["_tag"], &b["scope"]),
            (&json!("PeerScopeError"), &json!("attach"))
        );
    }
}
