//! JSON-RPC 2.0 framing shared by the WebSocket/stdio transport (`rpc`) and MCP serve.

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// What one incoming JSON-RPC message is.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A call that expects a response; `params` is `Null` when absent.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A response to a request the other side received.
    Response,
    /// Not a valid message; `id` is the request ID when one could be read, else `Null`.
    Invalid {
        id: Value,
        message: &'static str,
    },
}

/// Classify a decoded message. IDs are strings or integers (never `null`), and `params`,
/// when present, is an object or an array.
pub fn classify(frame: &Value) -> Frame {
    let Some(map) = frame.as_object() else {
        return invalid(Value::Null, "a JSON-RPC message must be one object");
    };
    let id = map.get("id").filter(|id| valid_id(id)).cloned();
    if map.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid(id.unwrap_or_default(), "jsonrpc must be \"2.0\"");
    }
    let params = map.get("params").cloned().unwrap_or_default();
    if !(params.is_null() || params.is_object() || params.is_array()) {
        return invalid(
            id.unwrap_or_default(),
            "params must be an object or an array",
        );
    }
    match (map.get("method"), map.get("id")) {
        (Some(Value::String(method)), None) => Frame::Notification {
            method: method.clone(),
            params,
        },
        (Some(Value::String(method)), Some(_)) => match id {
            Some(id) => Frame::Request {
                id,
                method: method.clone(),
                params,
            },
            None => invalid(Value::Null, "id must be a string or an integer"),
        },
        (Some(_), _) => invalid(id.unwrap_or_default(), "method must be a string"),
        (None, _) if id.is_some() && map.contains_key("result") != map.contains_key("error") => {
            Frame::Response
        }
        (None, _) => invalid(Value::Null, "not a request, notification or response"),
    }
}

fn valid_id(id: &Value) -> bool {
    id.is_string() || id.is_i64() || id.is_u64()
}

fn invalid(id: Value, message: &'static str) -> Frame {
    Frame::Invalid { id, message }
}

/// A success response.
pub fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// An error response.
pub fn error(id: Value, code: i64, message: &str, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message, "data": data } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_classified() {
        assert_eq!(
            classify(&json!({"jsonrpc":"2.0","id":1,"method":"ping"})),
            Frame::Request {
                id: json!(1),
                method: "ping".into(),
                params: Value::Null
            }
        );
        assert_eq!(
            classify(&json!({"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"x":1}})),
            Frame::Request {
                id: json!("a"),
                method: "tools/call".into(),
                params: json!({"x":1})
            }
        );
        assert_eq!(
            classify(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
            Frame::Notification {
                method: "notifications/initialized".into(),
                params: Value::Null
            }
        );
        assert_eq!(
            classify(&json!({"jsonrpc":"2.0","id":4,"result":{}})),
            Frame::Response
        );
        assert_eq!(
            classify(&json!({"jsonrpc":"2.0","id":4,"error":{"code":1,"message":"x"}})),
            Frame::Response
        );
    }

    #[test]
    fn malformed_messages_keep_a_readable_id() {
        for (frame, id) in [
            (json!([]), Value::Null),
            (json!({"id":3,"method":"ping"}), json!(3)),
            (
                json!({"jsonrpc":"1.0","id":"x","method":"ping"}),
                json!("x"),
            ),
            (
                json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
                Value::Null,
            ),
            (
                json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
                Value::Null,
            ),
            (json!({"jsonrpc":"2.0","id":2,"method":7}), json!(2)),
            (
                json!({"jsonrpc":"2.0","id":2,"method":"ping","params":3}),
                json!(2),
            ),
            (
                json!({"jsonrpc":"2.0","id":2,"result":{},"error":{}}),
                Value::Null,
            ),
            (json!({"jsonrpc":"2.0"}), Value::Null),
        ] {
            match classify(&frame) {
                Frame::Invalid { id: got, .. } => assert_eq!(got, id, "{frame}"),
                other => panic!("{frame}: {other:?}"),
            }
        }
    }

    #[test]
    fn envelopes_follow_json_rpc() {
        assert_eq!(
            result(json!(1), json!({})),
            json!({"jsonrpc":"2.0","id":1,"result":{}})
        );
        assert_eq!(
            error(json!("a"), METHOD_NOT_FOUND, "no", json!({"k":1})),
            json!({"jsonrpc":"2.0","id":"a","error":{"code":-32601,"message":"no","data":{"k":1}}})
        );
    }
}
