//! The `--input-format stream-json` envelopes (`exec-mode` → Streaming input and partial
//! output, Bounded interactive JSONL driver): one JSON object per line. Anything that is
//! not one of these is a protocol error, never text for the model.

use serde_json::Value;

/// The longest line accepted, in bytes.
pub const MAX_LINE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A prompt: `text` or `parts`, `delivery` (`queue`, the default, or `steer`), and an
    /// optional `id` the driver correlates its result with.
    User {
        id: Option<String>,
        parts: Vec<Value>,
        delivery: &'static str,
    },
    Interrupt,
    PermissionReply {
        request_id: String,
        reply: String,
        message: Option<String>,
    },
    QuestionReply {
        request_id: String,
        answers: Option<Vec<Vec<String>>>,
    },
    End,
}

/// One line, without its newline.
pub fn parse(line: &[u8]) -> Result<Frame, String> {
    if line.len() > MAX_LINE {
        return Err("line longer than 1 MiB".into());
    }
    let value: Value =
        serde_json::from_slice(line).map_err(|e| format!("not a JSON object: {e}"))?;
    let kind = value["type"].as_str().ok_or("missing \"type\"")?.to_owned();
    match kind.as_str() {
        "user" => user(&value),
        "interrupt" => Ok(Frame::Interrupt),
        "end" => Ok(Frame::End),
        "permission_reply" => {
            let reply = text(&value, "reply")?;
            if !matches!(reply.as_str(), "once" | "always" | "reject") {
                return Err("\"reply\" must be once, always or reject".into());
            }
            Ok(Frame::PermissionReply {
                request_id: request_id(&value)?,
                reply,
                message: value["message"].as_str().map(str::to_owned),
            })
        }
        "question_reply" => Ok(Frame::QuestionReply {
            request_id: request_id(&value)?,
            answers: match &value["answers"] {
                Value::Null => None,
                answers => Some(
                    serde_json::from_value(answers.clone())
                        .map_err(|_| "\"answers\" must be lists of strings")?,
                ),
            },
        }),
        other => Err(format!("unknown type \"{other}\"")),
    }
}

fn user(value: &Value) -> Result<Frame, String> {
    let parts = match (&value["text"], &value["parts"]) {
        (Value::String(text), Value::Null) => {
            vec![serde_json::json!({ "type": "text", "text": text })]
        }
        (Value::Null, Value::Array(parts))
            if !parts.is_empty() && parts.iter().all(|p| p["type"].is_string()) =>
        {
            parts.clone()
        }
        _ => return Err("a user line needs \"text\" or a non-empty \"parts\" list".into()),
    };
    let delivery = match value["delivery"].as_str() {
        None | Some("queue") => "queue",
        Some("steer") => "steer",
        Some(other) => return Err(format!("unknown delivery \"{other}\"")),
    };
    let id = match &value["id"] {
        Value::Null => None,
        Value::String(id) if id.len() <= 128 => Some(id.clone()),
        _ => return Err("\"id\" must be a string of at most 128 bytes".into()),
    };
    Ok(Frame::User {
        id,
        parts,
        delivery,
    })
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("missing \"{key}\""))
}

/// A server request ID: `per_…` or `que_…`, safe in a URL path.
fn request_id(value: &Value) -> Result<String, String> {
    let id = text(value, "request_id")?;
    let safe = id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b));
    if safe {
        Ok(id)
    } else {
        Err("\"request_id\" is not a request ID".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_lines() {
        assert_eq!(
            parse(br#"{"type":"user","text":"hi","id":"a1"}"#).unwrap(),
            Frame::User {
                id: Some("a1".into()),
                parts: vec![serde_json::json!({"type":"text","text":"hi"})],
                delivery: "queue",
            }
        );
        let steer =
            parse(br#"{"type":"user","parts":[{"type":"text","text":"x"}],"delivery":"steer"}"#);
        assert!(matches!(
            steer,
            Ok(Frame::User {
                delivery: "steer",
                ..
            })
        ));
        for bad in [
            &br#"{"type":"user"}"#[..],
            br#"{"type":"user","text":"x","parts":[]}"#,
            br#"{"type":"user","parts":[]}"#,
            br#"{"type":"user","text":"x","delivery":"hold"}"#,
            br#"{"type":"user","text":"x","id":7}"#,
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn control_and_reply_lines() {
        assert_eq!(parse(br#"{"type":"end"}"#).unwrap(), Frame::End);
        assert_eq!(parse(br#"{"type":"interrupt"}"#).unwrap(), Frame::Interrupt);
        assert_eq!(
            parse(br#"{"type":"permission_reply","request_id":"per_1","reply":"once"}"#).unwrap(),
            Frame::PermissionReply {
                request_id: "per_1".into(),
                reply: "once".into(),
                message: None
            }
        );
        assert!(
            parse(br#"{"type":"permission_reply","request_id":"per_1","reply":"yes"}"#).is_err()
        );
        assert!(
            parse(br#"{"type":"permission_reply","request_id":"../x","reply":"once"}"#).is_err()
        );
        assert_eq!(
            parse(br#"{"type":"question_reply","request_id":"que_1","answers":[["a"]]}"#).unwrap(),
            Frame::QuestionReply {
                request_id: "que_1".into(),
                answers: Some(vec![vec!["a".into()]])
            }
        );
    }

    #[test]
    fn anything_else_is_a_protocol_error() {
        assert!(
            parse(b"plain text")
                .unwrap_err()
                .starts_with("not a JSON object")
        );
        assert_eq!(parse(br#"{"text":"x"}"#).unwrap_err(), "missing \"type\"");
        assert_eq!(
            parse(br#"{"type":"shell"}"#).unwrap_err(),
            "unknown type \"shell\""
        );
        let long = vec![b' '; MAX_LINE + 1];
        assert_eq!(parse(&long).unwrap_err(), "line longer than 1 MiB");
    }
}
