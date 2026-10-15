//! Tagged errors (`server-api` → Tagged error model).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Serialize;

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
            RuntimeError::Store(cyber_store::StoreError::Busy) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                message,
            ),
            other => Self::unknown(other),
        }
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
