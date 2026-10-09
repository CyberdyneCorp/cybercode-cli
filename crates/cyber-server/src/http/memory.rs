//! Explicit authenticated memory review; model permissions govern model dispatch separately.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::request::Parts,
    routing::get,
};
use cyber_core::memory::{MemoryCatalog, MemoryDocument};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location, query_value},
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    #[default]
    Project,
    Global,
}

impl MemoryScope {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "project" => Ok(Self::Project),
            "global" => Ok(Self::Global),
            _ => Err(ApiError::invalid("Memory scope must be project or global")),
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memory", get(list))
        .route("/memory/requests", get(request_status))
        .route("/memory/edit/{scope}/{name}", get(edit_review))
        .route("/memory/recovery/{scope}", get(recovery).post(recover))
        .route("/memory/recovery/requests", get(recovery_request))
        .route("/memory/{scope}/{name}", get(read).put(put).delete(delete))
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<MemoryCatalog>>, ApiError> {
    let scope = query_value(parts.uri.query().unwrap_or_default(), "scope")
        .map(|value| MemoryScope::parse(&value))
        .transpose()?
        .unwrap_or_default();
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state.services.memory_list(directory.clone(), scope).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn read(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
    parts: Parts,
) -> Result<Json<Located<MemoryDocument>>, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    cyber_core::memory::validate_name(&name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .memory_read(directory.clone(), scope, name)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn edit_review(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
    parts: Parts,
) -> Result<Json<Located<cyber_core::memory::MemoryEditReview>>, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    cyber_core::memory::validate_name(&name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .memory_edit_review(directory.clone(), scope, name)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

pub fn validate_edit_fingerprint(value: Option<&str>) -> Result<(), ApiError> {
    if let Some(value) = value
        && (value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        return Err(ApiError::invalid(
            "Memory edit fingerprint must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

pub(super) fn unavailable() -> ApiError {
    let mut error = ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "ServiceUnavailableError",
        "Memory review is unavailable in this host",
    );
    error.body.service = Some("memory".into());
    error
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PutMemory {
    /// Complete Markdown document with YAML name, description and type fields.
    pub content: String,
    /// Fingerprint returned by editReview; omit for an unconditional write.
    pub review_fingerprint: Option<String>,
}

pub struct MemoryEdit {
    pub directory: std::path::PathBuf,
    pub scope: MemoryScope,
    pub name: String,
    pub content: Option<String>,
    pub review_fingerprint: Option<String>,
    pub identity: Option<super::idempotency::MemoryHttpIdentity>,
}

fn edit_request(
    parts: &Parts,
    state: &AppState,
    scope: String,
    name: String,
    content: Option<String>,
    review_fingerprint: Option<String>,
) -> Result<MemoryEdit, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    cyber_core::memory::validate_name(&name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    validate_edit_fingerprint(review_fingerprint.as_deref())?;
    Ok(MemoryEdit {
        directory: location(parts, &state.options.default_directory)?,
        scope,
        name,
        content,
        review_fingerprint,
        identity: parts
            .extensions
            .get::<super::idempotency::MemoryHttpIdentity>()
            .cloned(),
    })
}

async fn put(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
    parts: Parts,
    body: Result<Json<PutMemory>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Located<crate::runtime::MemoryChange>>, ApiError> {
    let Json(body) =
        body.map_err(|_| ApiError::invalid("Expected JSON object with memory content"))?;
    let edit = edit_request(
        &parts,
        &state,
        scope,
        name,
        Some(body.content),
        body.review_fingerprint,
    )?;
    let directory = edit.directory.clone();
    let data = state
        .services
        .memory_edit(state.runtime.clone(), edit)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
async fn delete(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
    parts: Parts,
    body: axum::body::Bytes,
) -> Result<Json<Located<crate::runtime::MemoryChange>>, ApiError> {
    if !body.is_empty() {
        return Err(ApiError::invalid("Memory delete takes no request body"));
    }
    let mut reviews = parts.headers.get_all("x-cyber-memory-review").iter();
    let review = reviews
        .next()
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| ApiError::invalid("Invalid memory review header"))?;
    if reviews.next().is_some() {
        return Err(ApiError::invalid("Expected one memory review header"));
    }
    let edit = edit_request(&parts, &state, scope, name, None, review)?;
    let directory = edit.directory.clone();
    let data = state
        .services
        .memory_edit(state.runtime.clone(), edit)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRecoveryView {
    pub storage: cyber_core::memory::MemoryRecoveryReview,
    pub admission: crate::runtime::MemoryRecoveryAdmission,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoverMemory {
    pub storage_fingerprint: String,
    pub admission_fingerprint: String,
}

impl RecoverMemory {
    pub fn validate(&self) -> Result<(), ApiError> {
        let valid = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        };
        if !valid(&self.storage_fingerprint)
            || !self
                .admission_fingerprint
                .strip_prefix("sha256:")
                .is_some_and(valid)
        {
            return Err(ApiError::invalid(
                "Expected storage and admission review fingerprints",
            ));
        }
        Ok(())
    }
}

async fn recovery(
    State(state): State<AppState>,
    Path(scope): Path<String>,
    parts: Parts,
) -> Result<Json<Located<Option<MemoryRecoveryView>>>, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .memory_recovery(state.runtime.clone(), directory.clone(), scope)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn recover(
    State(state): State<AppState>,
    Path(scope): Path<String>,
    parts: Parts,
    body: Result<Json<RecoverMemory>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Located<crate::runtime::MemoryChange>>, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    let Json(body) =
        body.map_err(|_| ApiError::invalid("Expected storage and admission review fingerprints"))?;
    body.validate()?;
    let directory = location(&parts, &state.options.default_directory)?;
    let confirmation = MemoryRecoveryConfirm {
        directory: directory.clone(),
        scope,
        review: body,
        identity: parts
            .extensions
            .get::<super::idempotency::MemoryHttpIdentity>()
            .cloned(),
    };
    let data = state
        .services
        .memory_recover(state.runtime.clone(), confirmation)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

pub struct MemoryRecoveryConfirm {
    pub directory: std::path::PathBuf,
    pub scope: MemoryScope,
    pub review: RecoverMemory,
    pub identity: Option<super::idempotency::MemoryHttpIdentity>,
}

async fn request_status(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Option<crate::runtime::MemoryRequestStatus>>>, ApiError> {
    let query = parts.uri.query().unwrap_or_default();
    let scope = query_value(query, "scope")
        .map(|value| MemoryScope::parse(&value))
        .transpose()?
        .unwrap_or_default();
    let key = query_value(query, "key")
        .ok_or_else(|| ApiError::invalid("Memory request key is required"))?;
    if !(1..=128).contains(&key.len()) || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ApiError::invalid("Invalid memory request key"));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    let runtime = state.runtime.clone();
    let data = tokio::task::spawn_blocking(move || runtime.memory_request_status(&key))
        .await
        .map_err(ApiError::unknown)?
        .map_err(ApiError::unknown)?;
    let project = match scope {
        MemoryScope::Global => "global".into(),
        MemoryScope::Project => cyber_core::project::identify(&directory).id,
    };
    if data
        .as_ref()
        .is_some_and(|status| status.directory != directory || status.project_id != project)
    {
        return Err(ApiError::not_found(
            "MemoryNotFoundError",
            "Memory request not found for this Location",
        ));
    }
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn recovery_request(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Option<crate::runtime::MemoryRecoveryRequestStatus>>>, ApiError> {
    let query = parts.uri.query().unwrap_or_default();
    let scope = query_value(query, "scope")
        .map(|value| MemoryScope::parse(&value))
        .transpose()?
        .unwrap_or_default();
    let key = query_value(query, "key")
        .ok_or_else(|| ApiError::invalid("Recovery request key is required"))?;
    if !(1..=128).contains(&key.len()) || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ApiError::invalid("Invalid recovery request key"));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    let runtime = state.runtime.clone();
    let data = tokio::task::spawn_blocking(move || runtime.memory_recovery_request_status(&key))
        .await
        .map_err(ApiError::unknown)?
        .map_err(ApiError::unknown)?;
    let project = match scope {
        MemoryScope::Global => "global".into(),
        MemoryScope::Project => cyber_core::project::identify(&directory).id,
    };
    if data
        .as_ref()
        .is_some_and(|status| status.directory != directory || status.project_id != project)
    {
        return Err(ApiError::not_found(
            "MemoryNotFoundError",
            "Recovery request not found for this Location",
        ));
    }
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
