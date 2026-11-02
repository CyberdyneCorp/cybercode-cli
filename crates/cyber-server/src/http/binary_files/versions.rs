//! Saved source history is shared by authorized readers. Discovery and review
//! never open another owner's command journal or admit its unsaved draft.
use super::*;
use cyber_core::binary::draft::Review;
use cyber_core::binary::receipts::Versions;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionsQuery {
    pub session_id: String,
    pub after: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewQuery {
    pub session_id: String,
    pub start: Option<usize>,
    pub limit: Option<usize>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Query(query): Query<VersionsQuery>,
) -> Result<Json<Located<Versions>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, query.session_id, query.after, query.limit);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &query.session_id, &id).await?;
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(refused)?;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let _cancel = cancellation.clone().drop_guard();
        let result = work(&state, move || {
            let content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let source = source(&entry)?;
            storage.source_versions(
                &source,
                query.after.as_deref(),
                query.limit.unwrap_or(64),
                || cancellation.is_cancelled(),
            )
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}
pub(super) async fn review(
    State(state): State<AppState>,
    parts: Parts,
    Path((id, document, version)): Path<(String, String, String)>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<Located<Review>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (
            state,
            parts,
            id,
            document,
            version,
            query.session_id,
            query.start,
            query.limit,
        );
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &query.session_id, &id).await?;
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(refused)?;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let _cancel = cancellation.clone().drop_guard();
        let result = work(&state, move || {
            let content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let source = source(&entry)?;
            let view = storage
                .open_version(&source, &document, &version, || cancellation.is_cancelled())?;
            let result =
                view.review(query.start.unwrap_or(0), query.limit.unwrap_or(128), || {
                    cancellation.is_cancelled()
                })?;
            source.validate()?;
            Ok(result)
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn source(entry: &Entry) -> Result<Document, Error> {
    let path = entry
        .resource
        .path()
        .strip_prefix(&entry.root)
        .map_err(|_| Error::File)?;
    Document::open(&entry.root, &cyber_core::vcs::slash_path(path))
}
