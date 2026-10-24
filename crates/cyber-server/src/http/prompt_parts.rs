//! The parts a client may send as a prompt (`server-api` → Prompt attachment parts):
//! text and media only. Inline media is decoded here and stored as references before
//! admission; any other part type is refused.

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use base64::Engine as _;
use cyber_llm::Content;
use schemars::JsonSchema;
use serde::Deserialize;

use super::error::ApiError;
use crate::runtime::Runtime;
use crate::runtime::attachments::{InputPart, MAX_MEDIA_PARTS, StoredParts};

/// Request bodies of the prompt routes may carry this much (base64 media included).
pub const PROMPT_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// One prompt part.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptPart {
    Text {
        text: String,
    },
    /// A base64 image. The stored media type is sniffed from the bytes.
    Image {
        /// Advisory only: the stored type is sniffed from the bytes.
        #[allow(dead_code)]
        media_type: String,
        data: String,
        name: Option<String>,
    },
    /// A base64 PDF. The stored media type is sniffed from the bytes.
    Document {
        /// Advisory only: the stored type is sniffed from the bytes.
        #[allow(dead_code)]
        media_type: String,
        data: String,
        name: Option<String>,
    },
    /// A file the server reads through the `read` tool's checks; local API only.
    File {
        path: String,
        name: Option<String>,
    },
    /// A blob this Session already stores, by its SHA-256.
    Attachment {
        sha256: String,
    },
}

impl PromptPart {
    fn is_media(&self) -> bool {
        !matches!(self, Self::Text { .. })
    }

    fn input(self) -> Result<InputPart, ApiError> {
        match self {
            Self::Text { text } => Ok(InputPart::Content(Content::Text { text })),
            Self::Image { data, name, .. } | Self::Document { data, name, .. } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|_| ApiError::invalid("attachment data is not valid base64"))?;
                Ok(InputPart::Inline { bytes, name })
            }
            Self::File { path, name } => Ok(InputPart::File { path, name }),
            Self::Attachment { sha256 } => Ok(InputPart::Stored { sha256 }),
        }
    }
}

/// Check the media count and decode inline media.
pub fn inputs(parts: Vec<PromptPart>) -> Result<Vec<InputPart>, ApiError> {
    if parts.is_empty() {
        return Err(ApiError::invalid("parts must not be empty"));
    }
    let media = parts.iter().filter(|part| part.is_media()).count();
    if media > MAX_MEDIA_PARTS {
        return Err(ApiError::invalid(format!(
            "a prompt carries at most {MAX_MEDIA_PARTS} attachments; this one has {media}"
        )));
    }
    parts.into_iter().map(PromptPart::input).collect()
}

/// The parts of a route that admits media without reading paths (a subtask's
/// attachments, an edited queued input), stored for Session `id` as a prompt's are:
/// inline media becomes references and an `attachment` part is rebuilt from its blob.
/// A `file` part is refused. The caller removes `created` when the request fails.
pub async fn stored(
    runtime: &Runtime,
    id: &str,
    parts: Vec<PromptPart>,
) -> Result<StoredParts, ApiError> {
    if parts
        .iter()
        .any(|part| matches!(part, PromptPart::File { .. }))
    {
        return Err(ApiError::invalid(
            "file parts are accepted by the prompt route only",
        ));
    }
    Ok(runtime.store_prompt_parts(id, inputs(parts)?).await?)
}

/// A body the prompt routes cannot read: `400` `InvalidRequestError` (an unknown part
/// type among them), or `413` over [`PROMPT_BODY_LIMIT`].
pub fn rejected(rejection: JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "MessageTooLargeError",
            "The prompt body exceeds 64 MiB",
        )
        .with_field("limit", PROMPT_BODY_LIMIT as u64);
    }
    ApiError::invalid(rejection.body_text())
}
