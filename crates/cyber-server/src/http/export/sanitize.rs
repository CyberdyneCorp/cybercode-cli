//! `--sanitize` for Session exports (`storage-events` → Session export): every string
//! that can carry user content becomes `[redacted:<kind>:<n>]`, where `<kind>` is `text`,
//! `path`, `url`, `tool` or `metadata` and `<n>` numbers the distinct values of that kind
//! in order of appearance, so equal values keep equal markers. Numbers, booleans, nulls,
//! object keys and the shape of arrays and objects are kept, and so are the strings that
//! only identify or classify: IDs, statuses, kinds, sources, agents, models and modes.

use std::collections::HashMap;

use serde_json::Value;

/// Strings kept as they are, outside tool inputs and outputs and metadata.
const KEPT: &[&str] = &[
    "id",
    "message_id",
    "call_id",
    "goal_id",
    "session_id",
    "parent_id",
    "parent_message_id",
    "run_id",
    "agent_id",
    "job_id",
    "worktree_id",
    "workflow_run_id",
    "subagent_name",
    "kind",
    "type",
    "role",
    "status",
    "verdict",
    "last_verdict",
    "delivery",
    "source",
    "agent",
    "model",
    "provider",
    "mode",
    "name",
    "retry_safety",
    "error_kind",
    "enforcement",
    "digest",
    "config_digest",
    "tier",
    "media_type",
    "mime",
    "branches",
];

/// Keys whose whole value is a tool input or output.
const TOOL: &[&str] = &[
    "input",
    "arguments",
    "output",
    "structured_output",
    "result",
];
/// Keys whose whole value is metadata.
const METADATA: &[&str] = &[
    "metadata",
    "args",
    "rules",
    "origin",
    "messaging",
    "hook_context",
];
/// Keys holding a path.
const PATH: &[&str] = &[
    "directory",
    "path",
    "source_directory",
    "worktree_origin",
    "script_file",
    "cwd",
    "output_file",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Text,
    Path,
    Url,
    Tool,
    Metadata,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Path => "path",
            Self::Url => "url",
            Self::Tool => "tool",
            Self::Metadata => "metadata",
        }
    }
}

/// Numbers redacted values per kind.
#[derive(Debug, Default)]
pub struct Sanitizer {
    markers: HashMap<(Kind, String), String>,
    counts: HashMap<Kind, usize>,
}

impl Sanitizer {
    /// Redact `export` in place.
    pub fn apply(&mut self, export: &mut Value) {
        self.walk(export, None, None);
    }

    /// `within` is the kind every string under a tool or metadata key takes.
    fn walk(&mut self, value: &mut Value, key: Option<&str>, within: Option<Kind>) {
        match value {
            Value::String(text) => {
                if let Some(kind) = self.kind_of(key, within, text) {
                    *text = self.marker(kind, text);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, key, within);
                }
            }
            Value::Object(map) => {
                for (field, item) in map.iter_mut() {
                    let within = within.or_else(|| scope(field));
                    self.walk(item, Some(field), within);
                }
            }
            _ => {}
        }
    }

    /// The kind a string is redacted as; `None` keeps it.
    fn kind_of(&self, key: Option<&str>, within: Option<Kind>, text: &str) -> Option<Kind> {
        if text.is_empty() {
            return None;
        }
        if let Some(kind) = within {
            return Some(kind);
        }
        let key = key.unwrap_or_default();
        if KEPT.contains(&key) {
            return None;
        }
        if PATH.contains(&key) {
            return Some(Kind::Path);
        }
        Some(classify(text))
    }

    fn marker(&mut self, kind: Kind, text: &str) -> String {
        let key = (kind, text.to_owned());
        if let Some(marker) = self.markers.get(&key) {
            return marker.clone();
        }
        let n = self.counts.entry(kind).or_default();
        *n += 1;
        let marker = format!("[redacted:{}:{n}]", kind.as_str());
        self.markers.insert(key, marker.clone());
        marker
    }
}

/// The kind every string under `field` takes, when it is a tool or metadata key.
fn scope(field: &str) -> Option<Kind> {
    if TOOL.contains(&field) {
        Some(Kind::Tool)
    } else if METADATA.contains(&field) {
        Some(Kind::Metadata)
    } else {
        None
    }
}

/// A URL, an absolute or home-relative path, or text.
fn classify(text: &str) -> Kind {
    let trimmed = text.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Kind::Url;
    }
    let windows_drive = trimmed.len() > 2
        && trimmed.as_bytes()[1] == b':'
        && matches!(trimmed.as_bytes()[2], b'\\' | b'/');
    let single_line = !trimmed.contains('\n');
    if single_line && (trimmed.starts_with('/') || trimmed.starts_with("~/") || windows_drive) {
        return Kind::Path;
    }
    Kind::Text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn content_is_redacted_and_structure_ids_and_counts_are_kept() {
        let mut export = json!({
            "session": {"id": "ses_1", "title": "fix /home/leo/app", "directory": "/home/leo/app",
                "model": "corp/main", "created_ms": 5},
            "totals": {"usage": {"input": 120, "output": 30}, "cost": 0.5},
            "branches": "all",
            "messages": [
                {"kind": "user", "id": "msg_1", "source": "session",
                    "parts": [{"type": "text", "text": "edit /home/leo/app/main.rs"}]},
                {"kind": "assistant", "id": "msg_2", "parent_message_id": "msg_1",
                    "text": "see https://example.com/x",
                    "started_ms": 10, "ended_ms": 20,
                    "tools": [{"call_id": "c1", "name": "read", "status": "ok",
                        "input": {"path": "/home/leo/app/main.rs", "limit": 5},
                        "arguments": "{\"path\":\"/home/leo/app/main.rs\"}", "output": "fn main() {}"}]}
            ],
            "goals": [{"goal_id": "gol_1", "condition": "tests pass", "status": "met",
                "reason": "tests pass", "turns": 2}]
        });
        Sanitizer::default().apply(&mut export);
        let text = export.to_string();
        for secret in [
            "/home/leo",
            "main.rs",
            "example.com",
            "tests pass",
            "fn main",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(export["session"]["id"], "ses_1");
        assert_eq!(export["session"]["model"], "corp/main");
        assert_eq!(export["session"]["directory"], "[redacted:path:1]");
        assert_eq!(export["totals"]["usage"]["input"], 120);
        assert_eq!(export["branches"], "all");
        assert_eq!(export["messages"][1]["parent_message_id"], "msg_1");
        assert_eq!(export["messages"][1]["started_ms"], 10);
        assert_eq!(export["messages"][1]["text"], "[redacted:text:3]");
        let tool = &export["messages"][1]["tools"][0];
        assert_eq!(
            (&tool["name"], &tool["status"]),
            (&json!("read"), &json!("ok"))
        );
        assert_eq!(tool["input"]["path"], "[redacted:tool:1]");
        assert_eq!(tool["input"]["limit"], 5);
        // Equal values keep equal markers.
        let goal = &export["goals"][0];
        assert_eq!(goal["condition"], goal["reason"]);
        assert_eq!(goal["status"], "met");
    }

    #[test]
    fn strings_are_classified_by_their_content() {
        assert_eq!(classify("https://a.b/c"), Kind::Url);
        assert_eq!(classify("/etc/hosts"), Kind::Path);
        assert_eq!(classify("~/notes.md"), Kind::Path);
        assert_eq!(classify(r"C:\repo\x"), Kind::Path);
        assert_eq!(classify("hello /etc/hosts"), Kind::Text);
        let mut empty = json!({"text": ""});
        Sanitizer::default().apply(&mut empty);
        assert_eq!(empty["text"], "");
    }
}
