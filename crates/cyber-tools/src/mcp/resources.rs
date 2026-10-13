//! MCP resource requests and validation of server replies (`mcp` → Resources).
//!
//! Every reply is untrusted: listings keep only typed, known fields, and read contents
//! must carry a URI and exactly one of `text` or a valid base64 `blob`. Decoded content
//! is capped at [`CONTENT_LIMIT`] per read, below the 1 MiB transport frame, so an
//! over-cap reply fails closed while the connection stays usable.
use base64::Engine as _;
use serde_json::{Map, Value, json};

use super::McpError;

/// Decoded bytes one `resources/read` may return across all its contents. A reply must
/// also fit the 1 MiB transport frame, which base64 and JSON escaping inflate, so the
/// spec's 10 MiB blob size is unreachable; this is the effective cap.
pub const CONTENT_LIMIT: usize = 512 * 1024;

/// One resource RPC. `resources/subscribe` is never sent.
#[derive(Debug, Clone, Copy)]
pub enum ResourceRequest<'a> {
    List(Option<&'a str>),
    Templates(Option<&'a str>),
    Read(&'a str),
}

/// An owned [`ResourceRequest`], for a request that outlives its caller.
#[derive(Debug, Clone)]
pub(crate) enum OwnedResourceRequest {
    List(Option<String>),
    Templates(Option<String>),
    Read(String),
}

impl OwnedResourceRequest {
    pub(crate) fn of(request: ResourceRequest<'_>) -> Self {
        match request {
            ResourceRequest::List(cursor) => Self::List(cursor.map(str::to_owned)),
            ResourceRequest::Templates(cursor) => Self::Templates(cursor.map(str::to_owned)),
            ResourceRequest::Read(uri) => Self::Read(uri.to_owned()),
        }
    }

    pub(crate) fn as_request(&self) -> ResourceRequest<'_> {
        match self {
            Self::List(cursor) => ResourceRequest::List(cursor.as_deref()),
            Self::Templates(cursor) => ResourceRequest::Templates(cursor.as_deref()),
            Self::Read(uri) => ResourceRequest::Read(uri),
        }
    }
}

impl ResourceRequest<'_> {
    pub(super) fn method(&self) -> &'static str {
        match self {
            Self::List(_) => "resources/list",
            Self::Templates(_) => "resources/templates/list",
            Self::Read(_) => "resources/read",
        }
    }

    pub(super) fn params(&self) -> Value {
        match self {
            Self::List(cursor) | Self::Templates(cursor) => cursor
                .map(|cursor| json!({"cursor":cursor}))
                .unwrap_or_else(|| json!({})),
            Self::Read(uri) => json!({"uri":uri}),
        }
    }
}

/// A validated reply: one listing page, or the contents of one read.
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceReply {
    /// Sanitized entries under `resources` or `resourceTemplates`, plus any `nextCursor`.
    Listing(Value),
    Contents(Vec<ResourceContent>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub body: ResourceBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceBody {
    Text(String),
    Blob(Vec<u8>),
}

pub(super) fn validate(
    request: ResourceRequest<'_>,
    result: &Value,
) -> Result<ResourceReply, McpError> {
    match request {
        ResourceRequest::List(_) => listing(result, "resources", "uri"),
        ResourceRequest::Templates(_) => listing(result, "resourceTemplates", "uriTemplate"),
        ResourceRequest::Read(_) => contents(result).map(ResourceReply::Contents),
    }
}

const ENTRY_FIELDS: &[&str] = &["name", "title", "description", "mimeType"];

fn listing(result: &Value, key: &str, locator: &str) -> Result<ResourceReply, McpError> {
    let invalid = McpError::Protocol("invalid MCP resource listing");
    let entries = result[key].as_array().ok_or(invalid)?;
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut item = Map::new();
        for field in std::iter::once(locator).chain(ENTRY_FIELDS.iter().copied()) {
            match entry.get(field) {
                Some(Value::String(text)) => {
                    item.insert(field.into(), text.clone().into());
                }
                None if field != locator && field != "name" => {}
                _ => return Err(McpError::Protocol("invalid MCP resource listing")),
            }
        }
        if let Some(size) = entry.get("size") {
            item.insert(
                "size".into(),
                size.as_u64()
                    .ok_or(McpError::Protocol("invalid MCP resource listing"))?
                    .into(),
            );
        }
        kept.push(Value::Object(item));
    }
    let mut page = json!({ key: kept });
    match result.get("nextCursor") {
        None | Some(Value::Null) => {}
        Some(Value::String(cursor)) => page["nextCursor"] = cursor.clone().into(),
        Some(_) => return Err(McpError::Protocol("invalid MCP resource listing cursor")),
    }
    Ok(ResourceReply::Listing(page))
}

fn contents(result: &Value) -> Result<Vec<ResourceContent>, McpError> {
    let invalid = || McpError::Protocol("invalid MCP resource contents");
    let entries = result["contents"].as_array().ok_or_else(invalid)?;
    let mut total = 0usize;
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let uri = entry["uri"].as_str().ok_or_else(invalid)?;
        let mime_type = match entry.get("mimeType") {
            None => None,
            Some(Value::String(mime)) => Some(mime.clone()),
            Some(_) => return Err(invalid()),
        };
        let body = match (entry.get("text"), entry.get("blob")) {
            (Some(Value::String(text)), None) => ResourceBody::Text(text.clone()),
            (None, Some(Value::String(blob))) => ResourceBody::Blob(decode(blob, total)?),
            _ => return Err(invalid()),
        };
        total += match &body {
            ResourceBody::Text(text) => text.len(),
            ResourceBody::Blob(bytes) => bytes.len(),
        };
        if total > CONTENT_LIMIT {
            return Err(McpError::Protocol("MCP resource exceeds transport limit"));
        }
        kept.push(ResourceContent {
            uri: uri.into(),
            mime_type,
            body,
        });
    }
    Ok(kept)
}

/// Decode a base64 blob, refusing before allocation when it cannot fit the remaining cap.
fn decode(blob: &str, used: usize) -> Result<Vec<u8>, McpError> {
    if blob.len() / 4 * 3 > CONTENT_LIMIT.saturating_sub(used) + 2 {
        return Err(McpError::Protocol("MCP resource exceeds transport limit"));
    }
    base64::engine::general_purpose::STANDARD
        .decode(blob)
        .map_err(|_| McpError::Protocol("invalid MCP resource blob"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(result: Value) -> Result<ResourceReply, McpError> {
        validate(ResourceRequest::Read("file:///x"), &result)
    }

    #[test]
    fn contents_require_uri_and_exactly_one_valid_body() {
        for result in [
            json!({}),
            json!({"contents":[{"text":"no uri"}]}),
            json!({"contents":[{"uri":"file:///x"}]}),
            json!({"contents":[{"uri":"file:///x","text":"a","blob":"YQ=="}]}),
            json!({"contents":[{"uri":"file:///x","blob":"not base64!"}]}),
            json!({"contents":[{"uri":"file:///x","text":1}]}),
            json!({"contents":[{"uri":"file:///x","text":"a","mimeType":7}]}),
        ] {
            assert!(
                matches!(read(result.clone()), Err(McpError::Protocol(_))),
                "{result}"
            );
        }
        assert_eq!(
            read(json!({"contents":[
                {"uri":"file:///a","mimeType":"text/plain","text":"hi"},
                {"uri":"file:///b","blob":"AAEC"}
            ]}))
            .unwrap(),
            ResourceReply::Contents(vec![
                ResourceContent {
                    uri: "file:///a".into(),
                    mime_type: Some("text/plain".into()),
                    body: ResourceBody::Text("hi".into()),
                },
                ResourceContent {
                    uri: "file:///b".into(),
                    mime_type: None,
                    body: ResourceBody::Blob(vec![0, 1, 2]),
                },
            ])
        );
    }

    #[test]
    fn contents_over_the_cap_fail_closed() {
        let text = "x".repeat(CONTENT_LIMIT / 2 + 1);
        let error = read(json!({"contents":[
            {"uri":"file:///a","text":text},{"uri":"file:///b","text":text}
        ]}))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "MCP protocol error: MCP resource exceeds transport limit"
        );
        let blob = base64::engine::general_purpose::STANDARD.encode(vec![0; CONTENT_LIMIT + 1]);
        assert!(read(json!({"contents":[{"uri":"file:///a","blob":blob}]})).is_err());
        let blob = base64::engine::general_purpose::STANDARD.encode(vec![0; CONTENT_LIMIT]);
        assert!(read(json!({"contents":[{"uri":"file:///a","blob":blob}]})).is_ok());
    }

    #[test]
    fn listings_keep_typed_known_fields_and_the_cursor() {
        let page = validate(
            ResourceRequest::List(None),
            &json!({"resources":[{"uri":"file:///a","name":"a","size":3,"extra":{"x":1}}],"nextCursor":"n"}),
        )
        .unwrap();
        assert_eq!(
            page,
            ResourceReply::Listing(
                json!({"resources":[{"uri":"file:///a","name":"a","size":3}],"nextCursor":"n"})
            )
        );
        for result in [
            json!({"resources":[{"name":"a"}]}),
            json!({"resources":[{"uri":"file:///a"}]}),
            json!({"resources":[{"uri":"file:///a","name":"a","description":5}]}),
            json!({"resources":[],"nextCursor":5}),
            json!({"resourceTemplates":[]}),
        ] {
            assert!(
                validate(ResourceRequest::List(None), &result).is_err(),
                "{result}"
            );
        }
        assert!(
            validate(
                ResourceRequest::Templates(None),
                &json!({"resourceTemplates":[{"uriTemplate":"file:///{x}","name":"x"}]})
            )
            .is_ok()
        );
    }
}
