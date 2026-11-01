//! Exact, bounded byte access. Live revisions are observations, not immutable snapshots.
//! Authorization belongs to the caller; the source pins regular-file and directory
//! identities across that authorization wait and every later read.

mod source;
pub use source::Document;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE: usize = 64 * 1024;
pub const MAX_PAGE: usize = 1024 * 1024;
pub const MAX_PATTERN: usize = 4096;
pub const MAX_SEARCH_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MATCHES: usize = 100;
pub const MAX_EDIT_SIZE: u64 = 512 * 1024 * 1024;

/// Lossless JSON decimal integer. Never accepts JSON numbers or noncanonical strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct Offset(pub u64);

impl TryFrom<String> for Offset {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 20
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("expected a canonical unsigned decimal string");
        }
        value
            .parse()
            .map(Self)
            .map_err(|_| "byte offset exceeds u64")
    }
}
impl From<Offset> for String {
    fn from(value: Offset) -> Self {
        value.0.to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid binary request")]
    Invalid,
    #[error("binary resource limit exceeded")]
    Limit,
    #[error("file or directory binding changed; reopen the document")]
    Changed,
    #[error("binary access requires a regular file within the project")]
    File,
    #[error("binary operation cancelled")]
    Cancelled,
    #[error("binary file access failed")]
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Metadata {
    pub path: String,
    pub length: Offset,
    /// Observed metadata generation, not an immutable whole-file content digest.
    pub revision: String,
    pub source: Source,
    pub within_edit_size_limit: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live,
}

/// Internal bytes; HTTP adapters choose their bounded transport encoding.
#[derive(Debug)]
pub struct Page {
    pub offset: Offset,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub eof: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub revision: String,
    /// Literal encoded bytes, never a regular expression.
    pub hex: String,
    pub offset: Offset,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
    pub revision: String,
    pub source: Source,
    pub offsets: Vec<Offset>,
    /// Next unexamined candidate offset. Reuse the same revision and pattern.
    pub next_offset: Option<Offset>,
    pub scanned: Offset,
}

/// Decode a bounded, even-length literal hex string. No whitespace or implicit UTF-8.
pub fn decode_hex(value: &str, max: usize) -> Result<Vec<u8>, Error> {
    if value.len() > max.saturating_mul(2) {
        return Err(Error::Limit);
    }
    if !value.len().is_multiple_of(2) {
        return Err(Error::Invalid);
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16).ok_or(Error::Invalid)?;
            let lo = (pair[1] as char).to_digit(16).ok_or(Error::Invalid)?;
            Ok(((hi << 4) | lo) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests;
