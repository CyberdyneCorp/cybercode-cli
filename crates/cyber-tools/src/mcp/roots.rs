//! Immutable, caller-authorized workspace roots; these do not grant filesystem access.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::McpError;

#[derive(Debug, Clone)]
pub struct McpRoots {
    result: Value,
}

impl McpRoots {
    /// Capture existing canonical writable paths, keeping the Location directory first.
    /// Relative additional writable roots resolve against the Location.
    pub fn new(location: &Path, writable: &[PathBuf]) -> Result<Self, McpError> {
        let location = canonical(location)?;
        if !location.is_dir() {
            return Err(McpError::Protocol("MCP Location is not a directory"));
        }
        let mut seen = BTreeSet::new();
        let mut roots = Vec::new();
        let mut bytes = 16usize;
        for path in
            std::iter::once(location.clone()).chain(writable.iter().map(|path| location.join(path)))
        {
            let path = canonical(&path)?;
            if !seen.insert(path.clone()) {
                continue;
            }
            let uri = if path.is_dir() {
                reqwest::Url::from_directory_path(&path)
            } else {
                reqwest::Url::from_file_path(&path)
            }
            .map_err(|_| McpError::Protocol("invalid MCP root URI"))?;
            let name = path.file_name().map_or_else(
                || "Workspace".into(),
                |name| name.to_string_lossy().into_owned(),
            );
            let root = json!({"uri":uri.as_str(),"name":name});
            bytes = bytes.saturating_add(
                serde_json::to_vec(&root)
                    .map_err(|_| McpError::Protocol("invalid MCP roots"))?
                    .len()
                    + 1,
            );
            // Reserve room for the server request identity and JSON-RPC envelope.
            if bytes > super::LIMIT / 2 {
                return Err(McpError::Protocol("MCP roots exceed frame budget"));
            }
            roots.push(root);
        }
        let result = json!({"roots":roots});
        Ok(Self { result })
    }

    pub(super) fn result(&self) -> &Value {
        &self.result
    }
}

fn canonical(path: &Path) -> Result<PathBuf, McpError> {
    let path = path
        .canonicalize()
        .map_err(|_| McpError::Protocol("MCP root unavailable"))?;
    if !path.is_dir() && !path.is_file() {
        return Err(McpError::Protocol("MCP root is not a file or directory"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_roots_are_unique_encoded_and_location_relative() {
        let dir = tempfile::tempdir().unwrap();
        let location = dir.path().join("project # Δ");
        std::fs::create_dir_all(location.join("extra")).unwrap();
        let roots = McpRoots::new(
            &location,
            &["extra".into(), "extra/..".into(), location.clone()],
        )
        .unwrap();
        let entries = roots.result()["roots"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let uri = reqwest::Url::parse(entries[0]["uri"].as_str().unwrap()).unwrap();
        assert_eq!(uri.scheme(), "file");
        assert!(uri.fragment().is_none() && uri.query().is_none());
        assert!(uri.as_str().contains("%23") && uri.as_str().contains("%20"));
        assert_eq!(entries[0]["name"], "project # Δ");
        assert_eq!(entries[1]["name"], "extra");
    }
    #[test]
    fn missing_roots_and_nondirectory_locations_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "contents").unwrap();
        assert!(McpRoots::new(dir.path(), &["missing".into()]).is_err());
        let roots = McpRoots::new(dir.path(), &["file".into()]).unwrap();
        assert_eq!(roots.result()["roots"][1]["name"], "file");
        assert!(
            !roots.result()["roots"][1]["uri"]
                .as_str()
                .unwrap()
                .ends_with('/')
        );
        assert!(McpRoots::new(&dir.path().join("file"), &[]).is_err());
    }
}
