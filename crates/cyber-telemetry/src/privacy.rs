//! The privacy gate (`observability-costs` → Telemetry privacy): every content attribute
//! goes through [`Content`], which yields nothing unless `telemetry.log_prompts` is on and
//! always redacts what it yields. No other code path adds content to a signal.

use cyber_core::config::redact_secrets;
use cyber_core::memory::redact_credentials_with;
use serde_json::Value;

/// The largest content value, in bytes; a longer one is cut and marked.
pub const LIMIT: usize = 16 * 1024;
const MARKER: &str = "***";
const TRUNCATED: &str = "…[truncated]";

/// The content gate and its redaction.
#[derive(Debug, Clone, Default)]
pub struct Content {
    enabled: bool,
    /// Exact secret values (the configured header values), replaced wherever they appear.
    exact: Vec<String>,
}

impl Content {
    pub fn new(log_prompts: bool, exact: impl IntoIterator<Item = String>) -> Self {
        Self {
            enabled: log_prompts,
            exact: exact
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    /// Whether content is exported at all.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// `text` redacted and capped, or nothing unless `log_prompts` is on.
    pub fn value(&self, text: &str) -> Option<String> {
        self.enabled.then(|| cap(self.redact(text)))
    }

    /// A structured value (tool input): secret keys masked, then every string leaf
    /// redacted, capped as JSON text. Nothing unless `log_prompts` is on.
    pub fn json(&self, value: &Value) -> Option<String> {
        self.enabled
            .then(|| cap(self.leaves(redact_secrets(value)).to_string()))
    }

    /// `text` with the exact secret values and the credential patterns replaced by `***`.
    pub fn redact(&self, text: &str) -> String {
        let exact = self
            .exact
            .iter()
            .fold(text.to_string(), |text, value| text.replace(value, MARKER));
        redact_credentials_with(&exact, MARKER)
    }

    fn leaves(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.redact(&text)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.leaves(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, self.leaves(value)))
                    .collect(),
            ),
            other => other,
        }
    }
}

/// `text` cut to [`LIMIT`] bytes on a character boundary, ending with the marker when cut.
fn cap(mut text: String) -> String {
    if text.len() <= LIMIT {
        return text;
    }
    let mut end = LIMIT - TRUNCATED.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(TRUNCATED);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nothing_is_exported_by_default() {
        let content = Content::new(false, ["s3cr3t".to_string()]);
        assert_eq!(content.value("hello"), None);
        assert_eq!(content.json(&json!({"command": "ls"})), None);
    }

    #[test]
    fn credentials_and_exact_header_values_become_the_marker() {
        let content = Content::new(true, ["hdr-value-42".to_string(), String::new()]);
        let text = content
            .value("key sk-abcdefghijklmnop, token ghp_abcdefghijklmnop, sent hdr-value-42")
            .unwrap();
        assert_eq!(text, "key ***, token ***, sent ***");
        let input = content
            .json(&json!({
                "authorization": "Bearer plain-value",
                "command": "curl -H 'x: hdr-value-42' sk-abcdefghijklmnop",
                "count": 3,
            }))
            .unwrap();
        let input: Value = serde_json::from_str(&input).unwrap();
        assert_eq!(input["authorization"], "***");
        assert_eq!(input["command"], "curl -H 'x: ***' ***");
        assert_eq!(input["count"], 3);
        assert_eq!(content.value("plain words"), Some("plain words".into()));
    }

    #[test]
    fn long_values_are_capped_on_a_character_boundary() {
        let content = Content::new(true, []);
        let text = content.value(&"é".repeat(LIMIT)).unwrap();
        assert!(text.len() <= LIMIT, "{}", text.len());
        assert!(text.ends_with(TRUNCATED));
        let short = "a".repeat(LIMIT);
        assert_eq!(content.value(&short).unwrap(), short);
    }
}
