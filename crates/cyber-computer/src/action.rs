use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Geometry, ImagePoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyChord {
    pub modifiers: Vec<Modifier>,
    pub key: String,
}

impl KeyChord {
    pub fn validate(&self) -> Result<(), Error> {
        if self.modifiers.len() > 4 {
            return Err(Error::Input("too many key modifiers"));
        }
        for (index, modifier) in self.modifiers.iter().enumerate() {
            if self.modifiers[..index].contains(modifier) {
                return Err(Error::Input("duplicate key modifier"));
            }
        }
        let single =
            self.key.len() == 1 && self.key.bytes().all(|byte| byte.is_ascii_alphanumeric());
        let named = matches!(
            self.key.as_str(),
            "enter"
                | "escape"
                | "tab"
                | "space"
                | "backspace"
                | "delete"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "page_up"
                | "page_down"
        );
        if !single && !named {
            return Err(Error::Input("unsupported symbolic key"));
        }
        Ok(())
    }
}

/// Raw text is deliberately not Debug/Serialize: durable records use audit().
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Move {
        point: ImagePoint,
    },
    Click {
        point: ImagePoint,
        button: Button,
        clicks: u8,
    },
    Drag {
        from: ImagePoint,
        to: ImagePoint,
        duration_ms: u64,
    },
    Scroll {
        point: ImagePoint,
        dx: i32,
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        chord: KeyChord,
    },
    Wait {
        milliseconds: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Audit {
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_sha256: Option<String>,
}

impl Action {
    pub fn audit_value(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Move { point } => json!({"action":"move","point":point}),
            Self::Click {
                point,
                button,
                clicks,
            } => json!({"action":"click","point":point,"button":button,"clicks":clicks}),
            Self::Drag {
                from,
                to,
                duration_ms,
            } => json!({"action":"drag","from":from,"to":to,"duration_ms":duration_ms}),
            Self::Scroll { point, dx, dy } => {
                json!({"action":"scroll","point":point,"dx":dx,"dy":dy})
            }
            Self::Type { .. } => serde_json::to_value(self.audit()).expect("typed audit is JSON"),
            Self::Key { chord } => {
                json!({"action":"key","modifiers":chord.modifiers,"key_category":if chord.key.len()==1 {"character"} else {"named"},"key_sha256":format!("{:x}",Sha256::digest(chord.key.as_bytes()))})
            }
            Self::Wait { milliseconds } => json!({"action":"wait","milliseconds":milliseconds}),
        }
    }

    pub fn validate(&self, geometry: &Geometry) -> Result<(), Error> {
        match self {
            Self::Move { point } => {
                geometry.desktop(*point)?;
            }
            Self::Click { point, clicks, .. } => {
                geometry.desktop(*point)?;
                if !(1..=2).contains(clicks) {
                    return Err(Error::Input("click count must be 1 or 2"));
                }
            }
            Self::Drag {
                from,
                to,
                duration_ms,
            } => {
                geometry.desktop(*from)?;
                geometry.desktop(*to)?;
                if !(1..=5000).contains(duration_ms) {
                    return Err(Error::Input("drag duration must be 1..5000 ms"));
                }
            }
            Self::Scroll { point, dx, dy } => {
                geometry.desktop(*point)?;
                if dx.unsigned_abs() > 2000 || dy.unsigned_abs() > 2000 {
                    return Err(Error::Input("scroll distance exceeds 2000"));
                }
            }
            Self::Type { text } => {
                if text.is_empty() || text.len() > 16 * 1024 || text.contains('\0') {
                    return Err(Error::Input(
                        "text must be 1..16384 UTF-8 bytes without NUL",
                    ));
                }
            }
            Self::Key { chord } => chord.validate()?,
            Self::Wait { milliseconds } => {
                if !(1..=5000).contains(milliseconds) {
                    return Err(Error::Input("wait must be 1..5000 ms"));
                }
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Move { .. } => "move",
            Self::Click { .. } => "click",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Wait { .. } => "wait",
        }
    }

    /// This summary never contains raw text. Frame, target and coordinate records
    /// are supplied by the trusted runtime after validated action admission.
    pub fn audit(&self) -> Audit {
        let (text_bytes, text_sha256) = match self {
            Self::Type { text } => (
                Some(text.len()),
                Some(format!("{:x}", Sha256::digest(text.as_bytes()))),
            ),
            _ => (None, None),
        };
        Audit {
            action: self.kind(),
            text_bytes,
            text_sha256,
        }
    }
}

/// Safe for durable tool records and hooks, including malformed calls. Native text
/// remains only in the original in-memory invocation; it is never reconstructed.
pub fn redacted_input(input: Option<&serde_json::Value>) -> serde_json::Value {
    use serde_json::json;
    let Some(input) = input else {
        return json!({"action":"invalid","arguments_redacted":true});
    };
    let mut payload = input.clone();
    let frame = payload.get("frame_id").cloned();
    if let Some(map) = payload.as_object_mut() {
        map.remove("frame_id");
    }
    let mut value = match serde_json::from_value::<Action>(payload) {
        Ok(action) => action.audit_value(),
        Err(_)
            if matches!(
                input.get("action").and_then(serde_json::Value::as_str),
                Some("status" | "screenshot")
            ) =>
        {
            json!({"action":input["action"]})
        }
        Err(_) => json!({"action":"invalid","arguments_redacted":true}),
    };
    if let Some(frame) = frame.and_then(|value| {
        value
            .as_str()
            .filter(|value| {
                value.len() <= 80
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .map(str::to_owned)
    }) {
        value["frame_id"] = json!(frame);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    fn geometry() -> Geometry {
        Geometry::new(
            100,
            100,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            0,
        )
        .unwrap()
    }

    #[test]
    fn rejects_unbounded_or_malformed_input_before_native_dispatch() {
        let point = ImagePoint { x: 50.0, y: 50.0 };
        for action in [
            Action::Type {
                text: String::new(),
            },
            Action::Type {
                text: "x".repeat(16385),
            },
            Action::Type {
                text: "bad\0text".into(),
            },
            Action::Wait { milliseconds: 5001 },
            Action::Click {
                point,
                button: Button::Left,
                clicks: 3,
            },
            Action::Drag {
                from: point,
                to: ImagePoint { x: 100.0, y: 0.0 },
                duration_ms: 10,
            },
            Action::Scroll {
                point,
                dx: i32::MIN,
                dy: 0,
            },
        ] {
            assert!(action.validate(&geometry()).is_err());
        }
    }

    #[test]
    fn unicode_text_is_bounded_by_encoded_bytes_and_key_chords_are_symbolic() {
        assert!(
            Action::Type {
                text: "編集 🦀".into()
            }
            .validate(&geometry())
            .is_ok()
        );
        assert!(
            KeyChord {
                modifiers: vec![Modifier::Command],
                key: "a".into()
            }
            .validate()
            .is_ok()
        );
        for key in ["osascript", "cmd+a; rm -rf /", "", "\n"] {
            assert!(
                KeyChord {
                    modifiers: vec![],
                    key: key.into()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            KeyChord {
                modifiers: vec![Modifier::Shift, Modifier::Shift],
                key: "a".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn durable_action_summary_excludes_typed_text() {
        let action = Action::Type {
            text: "private fixture text".into(),
        };
        let summary = serde_json::to_string(&action.audit()).unwrap();
        assert!(!summary.contains("private fixture text"));
        assert_eq!(action.audit().text_bytes, Some(20));
        assert_eq!(action.audit().text_sha256.as_ref().unwrap().len(), 64);
    }

    #[test]
    fn malformed_and_valid_tool_records_never_persist_raw_typed_text() {
        let secret = "private typing fixture";
        for input in [
            serde_json::json!({"action":"type","text":secret}),
            serde_json::json!({"action":"bad","text":secret,"unrecognized":secret}),
            serde_json::json!({"action":"screenshot","text":secret}),
        ] {
            assert!(!redacted_input(Some(&input)).to_string().contains(secret));
        }
        assert!(
            redacted_input(None)["arguments_redacted"]
                .as_bool()
                .unwrap()
        );
    }
}
