//! Owner-authenticated, Session-scoped binary live views. All content work runs on
//! bounded blocking workers; each request refreshes ordinary read authority.
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use cyber_core::binary::{self, Document, Error, Offset, SearchRequest, SearchResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::envelope::{Data, Located, LocationInfo};
use super::{ApiError, AppState, files};
use crate::runtime::binary_documents::Entry;
use crate::runtime::{FileDecision, TurnContext};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub session_id: String,
    pub path: String,
    pub expected_root: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct OpenedDocument {
    pub document_id: String,
    #[serde(flatten)]
    pub metadata: binary::Metadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentQuery {
    pub session_id: String,
    pub revision: Option<String>,
    pub offset: Option<Offset>,
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchBody {
    pub session_id: String,
    pub revision: String,
    pub hex: String,
    pub offset: Offset,
}

#[derive(Serialize, JsonSchema)]
pub struct BytePage {
    pub revision: String,
    pub source: binary::Source,
    pub offset: Offset,
    pub length: Offset,
    pub base64: String,
    pub page_sha256: String,
    pub eof: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/files/binary/documents", post(open_document))
        .route("/files/binary/documents/{id}/bytes", get(bytes))
        .route("/files/binary/documents/{id}/search", post(search))
        .route("/files/binary/documents/{id}", delete(close))
}

fn refused(error: Error) -> ApiError {
    let reason = match error {
        Error::Invalid => "invalid_binary_request",
        Error::Limit => "binary_limit",
        Error::Changed => "binary_changed",
        Error::File => "binary_regular_file_required",
        Error::Cancelled => "binary_cancelled",
        Error::Io => "binary_io",
    };
    ApiError::invalid(error.to_string()).with_reason(reason)
}

async fn work<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = state.runtime.binary_documents().worker().map_err(refused)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|_| refused(Error::Io))?
    .map_err(refused)
}

struct ReadAuthority {
    turn: TurnContext,
    root: PathBuf,
    grant: FileDecision,
}

async fn verify_read(
    state: &AppState,
    parts: &Parts,
    authority: &ReadAuthority,
    path: &FilePath,
) -> Result<(), ApiError> {
    let (now, root, _) = files::scope(state, parts, Some(&authority.turn.session_id)).await?;
    let prior = &authority.turn;
    if &now != prior || root != authority.root {
        return Err(
            ApiError::forbidden("Binary read scope changed; repeat the request")
                .with_reason("binary_authority_changed"),
        );
    }
    let decision = state
        .runtime
        .file_decisions(&now, "read", &[path.to_path_buf()])
        .await
        .first()
        .copied()
        .unwrap_or(FileDecision::Deny);
    if decision == FileDecision::Deny
        || (decision == FileDecision::Ask && authority.grant != FileDecision::Ask)
    {
        return Err(
            ApiError::forbidden("Binary read permission changed; repeat the request")
                .with_reason("binary_authority_changed")
                .with_field(
                    "decision",
                    if decision == FileDecision::Deny {
                        "deny"
                    } else {
                        "ask"
                    },
                ),
        );
    }
    Ok(())
}

async fn open_document(
    State(state): State<AppState>,
    parts: Parts,
    Json(input): Json<OpenRequest>,
) -> Result<Json<Located<OpenedDocument>>, ApiError> {
    let _admission = state.runtime.binary_documents().worker().map_err(refused)?;
    if input.session_id.is_empty() {
        return Err(ApiError::invalid("session_id is required"));
    }
    let (turn, root, _) = files::scope(&state, &parts, Some(&input.session_id)).await?;
    if input
        .expected_root
        .as_ref()
        .is_some_and(|expected| expected != &root.display().to_string())
    {
        return Err(
            ApiError::forbidden("The selected Session belongs to another project")
                .with_reason("binary_scope"),
        );
    }
    let at = root.clone();
    let document = work(&state, move || Document::open(&at, &input.path)).await?;
    let path = document.path();
    let grant = files::permit(&state, &turn, "read", &path).await?;
    let authority = ReadAuthority { turn, root, grant };
    verify_read(&state, &parts, &authority, &path).await?;
    let document = work(&state, move || {
        document.validate()?;
        Ok(document)
    })
    .await?;
    verify_read(&state, &parts, &authority, &path).await?;
    let location = LocationInfo::of(FilePath::new(&authority.turn.directory));
    let (document_id, metadata) = state
        .runtime
        .binary_documents()
        .insert(
            authority.turn.session_id,
            authority.turn.directory,
            authority.root,
            document,
        )
        .map_err(refused)?;
    Ok(Json(Located {
        location,
        data: OpenedDocument {
            document_id,
            metadata,
        },
    }))
}

/// Refresh Session and Location scope on every operation. Cross-owner IDs have the
/// same not-found response as absent IDs. No bytes are read before this check.
async fn owned(
    state: &AppState,
    parts: &Parts,
    session_id: &str,
    id: &str,
) -> Result<(Arc<Entry>, LocationInfo, ReadAuthority), ApiError> {
    let (turn, root, _) = files::scope(state, parts, Some(session_id)).await?;
    let entry = state
        .runtime
        .binary_documents()
        .get(id)
        .filter(|entry| {
            entry.session_id == turn.session_id
                && entry.directory == turn.directory
                && entry.root == root
        })
        .ok_or_else(|| ApiError::not_found("NotFoundError", "No such binary document"))?;
    let grant = files::permit(state, &turn, "read", &entry.path).await?;
    let location = LocationInfo::of(FilePath::new(&turn.directory));
    let authority = ReadAuthority { turn, root, grant };
    verify_read(state, parts, &authority, &entry.path).await?;
    Ok((entry, location, authority))
}

async fn bytes(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Query(query): Query<DocumentQuery>,
) -> Result<Json<Located<BytePage>>, ApiError> {
    let revision = query
        .revision
        .clone()
        .ok_or_else(|| ApiError::invalid("revision is required"))?;
    let (entry, location, authority) = owned(&state, &parts, &query.session_id, &id).await?;
    let path = entry.path.clone();
    let data = work(&state, move || {
        let page = entry.document.lock().map_err(|_| Error::Io)?.page(
            &revision,
            query.offset.unwrap_or(Offset(0)),
            query.limit.unwrap_or(binary::DEFAULT_PAGE),
        )?;
        Ok(BytePage {
            revision,
            source: binary::Source::Live,
            offset: page.offset,
            length: Offset(page.bytes.len() as u64),
            base64: base64::engine::general_purpose::STANDARD.encode(page.bytes),
            page_sha256: page.sha256,
            eof: page.eof,
        })
    })
    .await?;
    verify_read(&state, &parts, &authority, &path).await?;
    Ok(Json(Located { location, data }))
}

async fn search(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<SearchBody>,
) -> Result<Json<Located<SearchResult>>, ApiError> {
    let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
    let path = entry.path.clone();
    let input = SearchRequest {
        revision: input.revision,
        hex: input.hex,
        offset: input.offset,
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let result = work(&state, move || {
        entry
            .document
            .lock()
            .map_err(|_| Error::Io)?
            .search(&input, || cancellation.is_cancelled())
    })
    .await?;
    verify_read(&state, &parts, &authority, &path).await?;
    Ok(Json(Located {
        location,
        data: result,
    }))
}

/// Disposal returns no file metadata and cannot widen read authority. The owner
/// may release its old view after moving the Session or losing read permission.
async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DocumentQuery>,
) -> Result<Json<Data<bool>>, ApiError> {
    let entry = state
        .runtime
        .binary_documents()
        .get(&id)
        .filter(|entry| entry.session_id == query.session_id)
        .ok_or_else(|| ApiError::not_found("NotFoundError", "No such binary document"))?;
    state.runtime.binary_documents().remove(&id, &entry);
    Ok(Json(Data { data: true }))
}
