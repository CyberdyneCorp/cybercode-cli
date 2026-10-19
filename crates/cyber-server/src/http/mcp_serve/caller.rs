//! The recursion guard (`mcp-server` → MCP recursion guard): who is calling and at what
//! depth, from the `X-Cyber-MCP-Depth`/`X-Cyber-MCP-Caller` headers the stdio bridge
//! forwards and the `_meta["dev.cyber-code/caller"]` marker Cyber's own MCP client adds.
//! The markers are accident guards for trusted local clients, not a security boundary.

use axum::http::HeaderMap;
use serde_json::Value;

use super::super::ApiError;
use super::connection::Connection;

/// The `_meta` key of the caller marker.
pub(super) const META: &str = "dev.cyber-code/caller";

/// The markers of one request.
#[derive(Debug, Clone, Default)]
pub(super) struct Caller {
    /// The calling Session: the `_meta` caller when present, else the header.
    pub session_id: Option<String>,
    /// The largest depth either marker states.
    pub marked: u32,
}

impl Caller {
    pub fn new(headers: &HeaderMap, params: &Value) -> Self {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let meta = &params["_meta"][META];
        let meta_depth = meta["depth"].as_u64().and_then(|d| u32::try_from(d).ok());
        let header_depth = header("x-cyber-mcp-depth").and_then(|d| d.parse::<u32>().ok());
        let session_id = meta["session_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .or_else(|| header("x-cyber-mcp-caller"))
            .map(str::to_owned);
        Self {
            session_id,
            marked: meta_depth.max(header_depth).unwrap_or(0),
        }
    }

    /// The request depth: the marked depth, raised to the caller Session's run depth + 1.
    pub fn depth(&self, connection: &Connection) -> u32 {
        let runtime = &connection.state.runtime;
        let inherited = self
            .session_id
            .as_deref()
            .map_or(0, |id| runtime.effective_mcp_depth(id).saturating_add(1));
        self.marked.max(inherited)
    }

    /// `ForbiddenError depth_exceeded` at depth ≥ `max_depth`; otherwise the depth.
    pub fn check_depth(&self, connection: &Connection) -> Result<u32, ApiError> {
        let depth = self.depth(connection);
        let max_depth = connection.state.services.mcp_serve()?.limits.max_depth;
        if depth < max_depth {
            return Ok(depth);
        }
        Err(ApiError::forbidden(format!(
            "MCP serve call depth {depth} reaches the limit {max_depth}"
        ))
        .with_reason("depth_exceeded")
        .with_field("depth", depth)
        .with_field("max_depth", max_depth))
    }

    /// `ForbiddenError self_target` when `target` is the caller Session or one of its
    /// ancestors.
    pub fn check_target(&self, connection: &Connection, target: &str) -> Result<(), ApiError> {
        let Some(caller) = self.session_id.as_deref() else {
            return Ok(());
        };
        let runtime = &connection.state.runtime;
        if caller == target || runtime.ancestor_ids(caller).iter().any(|id| id == target) {
            return Err(self_target());
        }
        Ok(())
    }
}

/// `ForbiddenError self_target`: the call targets the caller Session or an ancestor.
pub(super) fn self_target() -> ApiError {
    ApiError::forbidden("a Session cannot drive itself or its ancestors through MCP serve")
        .with_reason("self_target")
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;
    use serde_json::json;

    use super::*;

    #[test]
    fn markers_from_headers_and_meta() {
        assert_eq!(Caller::new(&HeaderMap::new(), &json!({})).marked, 0);
        let mut headers = HeaderMap::new();
        headers.insert("x-cyber-mcp-depth", HeaderValue::from_static("1"));
        headers.insert("x-cyber-mcp-caller", HeaderValue::from_static("ses_header"));
        let plain = Caller::new(&headers, &json!({}));
        assert_eq!(plain.session_id.as_deref(), Some("ses_header"));
        assert_eq!(plain.marked, 1);
        let meta = json!({"_meta": {META: {"session_id": "ses_meta", "depth": 3}}});
        let both = Caller::new(&headers, &meta);
        assert_eq!(both.session_id.as_deref(), Some("ses_meta"), "_meta wins");
        assert_eq!(both.marked, 3, "the larger depth");
        headers.insert("x-cyber-mcp-depth", HeaderValue::from_static("junk"));
        assert_eq!(Caller::new(&headers, &json!({})).marked, 0);
    }
}
