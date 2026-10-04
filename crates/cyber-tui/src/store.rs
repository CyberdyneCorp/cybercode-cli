//! Prompt history, drafts and preferences under the state directory
//! (`tui` → Prompt editor, Themes and appearance).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const HISTORY_LIMIT: usize = 500;

#[derive(Debug, Clone)]
pub struct LocalStore {
    dir: PathBuf,
}

impl LocalStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.to_path_buf(),
        }
    }

    /// The last 500 prompts for a project, oldest first.
    pub fn history(&self, project: &str) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.join("history.jsonl")).unwrap_or_default();
        let mut items: Vec<String> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["project"] == project)
            .filter_map(|v| v["text"].as_str().map(str::to_string))
            .collect();
        let excess = items.len().saturating_sub(HISTORY_LIMIT);
        items.drain(..excess);
        items
    }

    pub fn push_history(&self, project: &str, text: &str) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("history.jsonl"))
        {
            let _ = writeln!(f, "{}", json!({ "project": project, "text": text }));
        }
    }

    fn kv(&self) -> BTreeMap<String, Value> {
        std::fs::read_to_string(self.dir.join("kv.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.kv().remove(key)
    }

    pub fn set(&self, key: &str, value: Value) {
        let mut kv = self.kv();
        if value.is_null() {
            kv.remove(key);
        } else {
            kv.insert(key.into(), value);
        }
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(
            self.dir.join("kv.json"),
            serde_json::to_string_pretty(&kv).unwrap_or_default(),
        );
    }

    pub fn draft(&self, session: &str) -> String {
        self.get(&format!("draft:{session}"))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    pub fn save_draft(&self, session: &str, text: &str) {
        let value = if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        };
        self.set(&format!("draft:{session}"), value);
    }

    /// Record a model as most recent (`model.json`, at most 10).
    pub fn push_recent_model(&self, model: &str) {
        let path = self.dir.join("model.json");
        let mut doc: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        let mut recent: Vec<String> = doc["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|m| *m != model)
            .map(str::to_string)
            .collect();
        recent.insert(0, model.into());
        recent.truncate(10);
        doc["recent"] = json!(recent);
        let _ = std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_and_drafts_persist_per_project_and_session() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LocalStore::new(tmp.path());
        store.push_history("/a", "one");
        store.push_history("/b", "other");
        store.push_history("/a", "two");
        assert_eq!(store.history("/a"), vec!["one", "two"]);
        store.save_draft("ses_1", "half typed");
        assert_eq!(LocalStore::new(tmp.path()).draft("ses_1"), "half typed");
        store.save_draft("ses_1", "");
        assert_eq!(store.draft("ses_1"), "");
        store.push_recent_model("a/1");
        store.push_recent_model("b/2");
        store.push_recent_model("a/1");
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("model.json")).unwrap())
                .unwrap();
        assert_eq!(doc["recent"], json!(["a/1", "b/2"]));
    }
}
