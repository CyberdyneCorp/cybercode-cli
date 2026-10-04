//! Response envelopes and Location routing (`server-api` → Location routing, Pagination).

use std::path::{Path, PathBuf};

use axum::http::HeaderMap;
use axum::http::request::Parts;
use schemars::JsonSchema;
use serde::Serialize;

use super::error::ApiError;

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "Data_{T}")]
pub struct Data<T> {
    pub data: T,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Cursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "Page_{T}")]
pub struct Page<T> {
    pub data: Vec<T>,
    pub cursor: Cursor,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProjectInfo {
    pub id: String,
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LocationInfo {
    pub directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub project: ProjectInfo,
}

impl LocationInfo {
    pub fn of(directory: &Path) -> Self {
        let project = cyber_core::project::identify(directory);
        Self {
            directory: directory.display().to_string(),
            workspace: None,
            project: ProjectInfo {
                id: project.id,
                directory: project.worktree.display().to_string(),
            },
        }
    }
}

/// A Location-scoped response.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "Located_{T}")]
pub struct Located<T> {
    pub location: LocationInfo,
    pub data: T,
}

/// The Location of a request: `location[directory]`, then `x-cyber-directory`, then the default.
pub fn location(parts: &Parts, default: &Path) -> Result<PathBuf, ApiError> {
    let from_query = parts
        .uri
        .query()
        .and_then(|q| query_value(q, "location[directory]"));
    let from_header = header(&parts.headers, "x-cyber-directory").map(|h| percent_decode(&h));
    let directory = from_query
        .or(from_header)
        .map_or_else(|| default.to_path_buf(), PathBuf::from);
    std::fs::canonicalize(&directory)
        .map_err(|_| ApiError::invalid(format!("no such directory: {}", directory.display())))
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// One decoded query parameter by its exact (decoded) name.
pub fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (query_decode(k) == name).then(|| query_decode(v))
    })
}

/// URI-decode `%XX` escapes; malformed escapes stay literal.
pub fn percent_decode(s: &str) -> String {
    decode(s, false)
}

/// Decode a query component, where `+` also means a space.
fn query_decode(s: &str) -> String {
    decode(s, true)
}

fn decode(s: &str, plus_is_space: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_decode_bracketed_names() {
        let q = "location%5Bdirectory%5D=%2Ftmp%2Fa%20b&x=1";
        assert_eq!(
            query_value(q, "location[directory]").as_deref(),
            Some("/tmp/a b")
        );
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a+b"), "a+b");
        assert_eq!(query_value("q=a+b", "q").as_deref(), Some("a b"));
    }
}
