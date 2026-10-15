//! `server-api` operations called in-process through the connection's router, by
//! operation ID, so route semantics (idempotency, tagged errors, admission) stay the
//! routes' own.

use axum::body::Body;
use axum::http::Request;
use serde_json::Value;

use super::super::{ApiError, openapi, rpc};
use super::connection::Connection;
use super::tools::ToolError;

/// One operation call: path and query parameters, an optional JSON body and headers.
#[derive(Default)]
pub(super) struct Call<'a> {
    pub path: &'a [(&'a str, &'a str)],
    pub query: &'a [(&'a str, &'a str)],
    pub body: Option<&'a Value>,
    pub headers: &'a [(&'a str, &'a str)],
}

/// A body-less operation, as the reading tools call them.
pub(super) async fn route(
    connection: &Connection,
    operation: &str,
    path: &[(&str, &str)],
    query: &[(&str, &str)],
) -> Result<Value, ToolError> {
    send(
        connection,
        operation,
        Call {
            path,
            query,
            ..Call::default()
        },
    )
    .await
}

/// Call an operation; a non-2xx answer is its tagged error body, unchanged.
pub(super) async fn send(
    connection: &Connection,
    operation: &str,
    call: Call<'_>,
) -> Result<Value, ToolError> {
    let (method, template) = openapi::lookup(operation)
        .ok_or_else(|| ApiError::unknown(format!("no operation {operation}")))?;
    let mut uri = format!("http://cyber.internal/api/v1{template}");
    for (name, value) in call.path {
        uri = uri.replace(&format!("{{{name}}}"), &encode(value));
    }
    let pairs: Vec<String> = call
        .query
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect();
    if !pairs.is_empty() {
        uri = format!("{uri}?{}", pairs.join("&"));
    }
    let mut request = Request::builder()
        .method(method.to_uppercase().as_str())
        .uri(uri);
    for (name, value) in call.headers {
        request = request.header(*name, *value);
    }
    let body = match call.body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let request = request.body(body).map_err(ApiError::unknown)?;
    let (status, body) = rpc::send(&connection.client, request).await;
    answer(operation, status, body)
}

/// A route answer as a tool outcome: 2xx is the body, otherwise its tagged error.
pub(super) fn answer(operation: &str, status: u16, body: Value) -> Result<Value, ToolError> {
    if (200..300).contains(&status) {
        Ok(body)
    } else if body["_tag"].is_string() {
        Err(ToolError(body))
    } else {
        Err(ApiError::unknown(format!("{operation} answered {status}")).into())
    }
}

/// Percent-encode everything but unreserved characters, so a value stays one segment.
pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_encoded() {
        assert_eq!(encode("ses_1-a.b~c"), "ses_1-a.b~c");
        assert_eq!(encode("a/../b?c"), "a%2F..%2Fb%3Fc");
    }
}
