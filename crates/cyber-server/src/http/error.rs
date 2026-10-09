//! Tagged errors (`server-api` → Tagged error model).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Serialize;

use crate::runtime::RuntimeError;

/// The JSON body of every declared failure.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ErrorBody {
    #[serde(rename = "_tag")]
    pub tag: String,
    pub message: String,
    /// `err_<id>` for `UnknownError`, logged server-side.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Paths for rewind conflicts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// Capability group unavailable in this host or phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<Box<str>>,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
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
            },
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
        e.body.reference = Some(reference);
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
