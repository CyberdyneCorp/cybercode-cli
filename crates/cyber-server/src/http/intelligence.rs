//! Authenticated Location code-intelligence status and lifecycle.
use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LspState {
    Starting,
    Connected,
    Broken,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LspStatus {
    pub id: String,
    pub root: std::path::PathBuf,
    pub status: LspState,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LspTransition {
    pub closed: bool,
    /// Admission is reopened lazily; this operation starts no language server.
    pub reloaded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_queries_repeat_focus_and_symbol() {
        let request = RepoMapRequest::parse(
            "path=src%2Fauth&focus=a.rs&focus=b+c.rs&symbol=Run&tokens=512&x=1",
        )
        .unwrap();
        assert_eq!(request.path.as_deref(), Some("src/auth"));
        assert_eq!(request.focus, vec!["a.rs", "b c.rs"]);
        assert_eq!(request.symbol, vec!["Run"]);
        assert_eq!(request.tokens, Some(512));
        assert!(RepoMapRequest::parse("tokens=lots").is_err());
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormatterStatus {
    pub id: String,
    pub extensions: Vec<String>,
    pub enabled: bool,
    pub installed: bool,
    pub detected_by: String,
}
impl From<cyber_core::intelligence::DetectedFormatter> for FormatterStatus {
    fn from(formatter: cyber_core::intelligence::DetectedFormatter) -> Self {
        Self {
            id: formatter.definition.id,
            extensions: formatter.definition.extensions,
            enabled: formatter.enabled,
            installed: formatter.installed,
            detected_by: formatter.detected_by,
        }
    }
}
/// A repository map symbol (`code-intelligence` → Repository map inspection surfaces).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct RepoMapSymbol {
    pub name: String,
    /// `class`, `struct`, `enum`, `interface`, `type`, `function`, `method`,
    /// `constant`, `macro` or `module`.
    pub kind: String,
    pub line: u32,
    pub signature: String,
}

/// A file shown in a repository map, with its PageRank and selected definitions.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct RepoMapFile {
    pub path: String,
    pub rank: f64,
    pub symbols: Vec<RepoMapSymbol>,
}

/// Repository index status. Counts only: no path, name or signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct RepoMapIndex {
    /// `complete`, `partial`, `truncated`, `stale` or `building`.
    pub status: String,
    pub files: usize,
    pub symbols: usize,
    /// Excluded files by reason.
    pub excluded: std::collections::BTreeMap<String, u64>,
    /// Indexed files by language.
    pub languages: std::collections::BTreeMap<String, u64>,
    /// Milliseconds since the Unix epoch of the last finished refresh.
    pub refreshed_at: Option<i64>,
    pub build_ms: Option<i64>,
}

/// A rendered repository map.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct RepoMapView {
    pub text: String,
    pub files: Vec<RepoMapFile>,
    pub index: RepoMapIndex,
}

/// `GET /lsp/map` query: `path`, repeated `focus` and `symbol`, and `tokens`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoMapRequest {
    pub path: Option<String>,
    pub focus: Vec<String>,
    pub symbol: Vec<String>,
    pub tokens: Option<usize>,
}

impl RepoMapRequest {
    pub fn parse(query: &str) -> Result<Self, ApiError> {
        let mut request = Self::default();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = super::envelope::query_decode(value);
            match super::envelope::query_decode(key).as_str() {
                "path" => request.path = Some(value),
                "focus" => request.focus.push(value),
                "symbol" => request.symbol.push(value),
                "tokens" => {
                    request.tokens = Some(
                        value
                            .parse()
                            .map_err(|_| ApiError::invalid("tokens must be a positive integer"))?,
                    )
                }
                _ => {}
            }
        }
        Ok(request)
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/formatters", get(formatters))
        .route("/lsp", get(lsp))
        .route("/lsp/close", post(close_lsp))
        .route("/lsp/reload", post(reload_lsp))
        .route("/lsp/map", get(repo_map))
        .route("/lsp/map/rebuild", post(rebuild_repo_map))
}

async fn repo_map(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<RepoMapView>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid(
            "Repository map Location must be a directory",
        ));
    }
    let request = RepoMapRequest::parse(parts.uri.query().unwrap_or_default())?;
    let data = state.services.repo_map(directory.clone(), request).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn rebuild_repo_map(
    State(state): State<AppState>,
    parts: Parts,
    body: Bytes,
) -> Result<(StatusCode, Json<Located<RepoMapIndex>>), ApiError> {
    if !body.is_empty() {
        return Err(ApiError::invalid(
            "Repository map rebuild takes no request body",
        ));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid(
            "Repository map Location must be a directory",
        ));
    }
    let data = state.services.rebuild_repo_map(directory.clone()).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(Located {
            location: LocationInfo::of(&directory),
            data,
        }),
    ))
}
async fn close_lsp(
    State(state): State<AppState>,
    parts: Parts,
    body: Bytes,
) -> Result<Json<Located<LspTransition>>, ApiError> {
    transition_lsp(state, parts, body, false).await
}
async fn reload_lsp(
    State(state): State<AppState>,
    parts: Parts,
    body: Bytes,
) -> Result<Json<Located<LspTransition>>, ApiError> {
    transition_lsp(state, parts, body, true).await
}
async fn transition_lsp(
    state: AppState,
    parts: Parts,
    body: Bytes,
    reload: bool,
) -> Result<Json<Located<LspTransition>>, ApiError> {
    if !body.is_empty() {
        return Err(ApiError::invalid("LSP lifecycle takes no request body"));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("LSP Location must be a directory"));
    }
    state
        .services
        .transition_lsp(directory.clone(), reload)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: LspTransition {
            closed: true,
            reloaded: reload,
        },
    }))
}
async fn lsp(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<LspStatus>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("LSP Location must be a directory"));
    }
    let data = state.services.lsp_status(directory.clone()).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
async fn formatters(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<FormatterStatus>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("Formatter Location must be a directory"));
    }
    let data = state.services.formatters(directory.clone()).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
