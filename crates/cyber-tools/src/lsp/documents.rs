use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
    time::Duration,
};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{LspError, StdioConnection};

pub(crate) const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DOCUMENTS: usize = 128;

pub(super) async fn verify_origin(
    path: PathBuf,
    expected: Vec<cyber_core::worktrees::Managed>,
) -> Result<(), LspError> {
    tokio::task::spawn_blocking(move || {
        if !canonical_shape(&path)
            || path.canonicalize().ok().as_ref() != Some(&path)
            || !path.is_file()
            || super::locations::checkout_records(&path)? != expected
        {
            return Err(LspError::Protocol("document creation changed"));
        }
        Ok(())
    })
    .await
    .map_err(|_| LspError::Protocol("document observation failed"))?
}

pub(super) async fn verify_removed(
    path: PathBuf,
    expected: Vec<cyber_core::worktrees::Managed>,
) -> Result<(), LspError> {
    tokio::task::spawn_blocking(move || {
        let parent = path.parent().ok_or(LspError::Protocol("removed document parent unavailable"))?;
        if !canonical_shape(&path)
            || parent.canonicalize().ok().as_deref() != Some(parent)
            || !matches!(std::fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            || super::locations::checkout_records(parent)? != expected {
            return Err(LspError::Protocol("removed document creation changed"));
        }
        Ok(())
    }).await.map_err(|_| LspError::Protocol("removed document observation failed"))?
}

fn canonical_shape(path: &std::path::Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

pub(super) async fn verify_scope(
    path: PathBuf,
    expected: Vec<cyber_core::worktrees::Managed>,
    removed: bool,
) -> Result<(), LspError> {
    if removed {
        verify_removed(path, expected).await
    } else {
        verify_origin(path, expected).await
    }
}

#[derive(Default)]
pub(super) struct Documents {
    entries: BTreeMap<PathBuf, (i32, [u8; 32])>,
    // Eviction must not let delayed diagnostics collide with a reopened document's version.
    next_version: i32,
    saved: BTreeMap<PathBuf, tokio::time::Instant>,
}

impl Documents {
    pub async fn removed(
        &mut self,
        connection: &mut StdioConnection,
        path: &std::path::Path,
    ) -> Result<(), LspError> {
        let document_uri = uri(path)?;
        if self.entries.contains_key(path) {
            notify(
                connection,
                "textDocument/didClose",
                json!({"textDocument":{"uri":document_uri}}),
            )
            .await?;
            self.entries.remove(path);
            self.saved.remove(path);
        }
        notify(
            connection,
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":document_uri,"type":3}]}),
        )
        .await
    }

    pub async fn save(
        &mut self,
        connection: &mut StdioConnection,
        path: PathBuf,
        text: String,
        created: bool,
    ) -> Result<i32, LspError> {
        let language = language(&path);
        let include_text = connection
            .capabilities()
            .and_then(|capabilities| capabilities.pointer("/textDocumentSync/save/includeText"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut params = json!({"textDocument":{"uri":uri(&path)?}});
        if include_text {
            params["text"] = json!(text);
        }
        self.open(connection, path.clone(), text, language).await?;
        notify(connection, "textDocument/didSave", params).await?;
        notify(
            connection,
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(&path)?,"type":if created { 1 } else { 2 }}]}),
        )
        .await?;
        self.saved.insert(path.clone(), tokio::time::Instant::now());
        self.version(&path)
            .ok_or(LspError::Protocol("saved document unavailable"))
    }

    pub fn quiet_deadline(&self, path: &std::path::Path) -> Option<tokio::time::Instant> {
        self.saved
            .get(path)
            .map(|saved| *saved + Duration::from_millis(150))
    }

    pub fn version(&self, path: &std::path::Path) -> Option<i32> {
        self.entries.get(path).map(|(version, _)| *version)
    }

    pub fn matches_digest(&self, path: &std::path::Path, digest: &[u8; 32]) -> bool {
        self.entries
            .get(path)
            .is_none_or(|(_, current)| current == digest)
    }

    pub async fn open(
        &mut self,
        connection: &mut StdioConnection,
        path: PathBuf,
        text: String,
        language: String,
    ) -> Result<(), LspError> {
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let document_uri = uri(&path)?;
        if self
            .entries
            .get(&path)
            .is_some_and(|(_, previous)| previous == &digest)
        {
            return Ok(());
        }
        let version = self
            .next_version
            .checked_add(1)
            .ok_or(LspError::Protocol("document version exhausted"))?;
        let (method, params) = match self.entries.get(&path) {
            Some(_) => (
                "textDocument/didChange",
                json!({"textDocument":{"uri":document_uri,"version":version},"contentChanges":[{"text":text}]}),
            ),
            None => {
                if self.entries.len() == MAX_DOCUMENTS {
                    let first = self.entries.first_key_value().unwrap().0.clone();
                    notify(
                        connection,
                        "textDocument/didClose",
                        json!({"textDocument":{"uri":uri(&first)?}}),
                    )
                    .await?;
                    self.entries.remove(&first);
                    self.saved.remove(&first);
                }
                (
                    "textDocument/didOpen",
                    json!({"textDocument":{"uri":document_uri,"languageId":language,"version":version,"text":text}}),
                )
            }
        };
        notify(connection, method, params).await?;
        self.next_version = version;
        self.entries.insert(path, (version, digest));
        Ok(())
    }
}

fn uri(path: &std::path::Path) -> Result<String, LspError> {
    reqwest::Url::from_file_path(path)
        .map(String::from)
        .map_err(|_| LspError::Protocol("document URI unavailable"))
}

async fn notify(
    connection: &mut StdioConnection,
    method: &str,
    params: serde_json::Value,
) -> Result<(), LspError> {
    tokio::time::timeout(Duration::from_secs(30), connection.notify(method, params))
        .await
        .unwrap_or(Err(LspError::Timeout))
}

pub(super) fn language(path: &std::path::Path) -> String {
    match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "java" => "java",
        "lua" => "lua",
        "zig" => "zig",
        "sh" | "bash" => "shellscript",
        "yaml" | "yml" => "yaml",
        "svelte" => "svelte",
        "vue" => "vue",
        "sol" => "solidity",
        "sv" | "svh" => "systemverilog",
        other => other,
    }
    .into()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::Value;

    #[tokio::test]
    async fn version_exhaustion_refuses_changed_snapshots_without_wrapping_or_dispatch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file.rs");
        std::fs::write(&path, "unchanged").unwrap();
        let path = path.canonicalize().unwrap();
        let mut connection = StdioConnection::connect_with_timeout(
            crate::lsp::connection::tests::process("normal").await,
            root.path(),
            json!({"fixture":true}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        let mut documents = Documents {
            next_version: i32::MAX - 1,
            ..Documents::default()
        };
        documents
            .open(
                &mut connection,
                path.clone(),
                "unchanged".into(),
                "rust".into(),
            )
            .await
            .unwrap();
        documents
            .open(
                &mut connection,
                path.clone(),
                "unchanged".into(),
                "rust".into(),
            )
            .await
            .unwrap();
        assert!(matches!(
            documents
                .open(
                    &mut connection,
                    path.clone(),
                    "changed".into(),
                    "rust".into()
                )
                .await,
            Err(LspError::Protocol("document version exhausted"))
        ));
        assert_eq!(documents.next_version, i32::MAX);
        assert_eq!(documents.entries[&path].0, i32::MAX);
        connection
            .request("fixture", json!({}), Duration::from_secs(3))
            .await
            .unwrap();
        let events: Vec<Value> = std::fs::read_to_string(root.path().join("document-events"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["method"], "textDocument/didOpen");
        assert_eq!(events[0]["params"]["textDocument"]["version"], i32::MAX);
        assert!(connection.shutdown().await.acknowledged);
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    #[tokio::test]
    async fn removal_observation_refuses_recreated_and_noncanonical_paths() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let path = directory.join("deleted.rs");
        verify_removed(path.clone(), vec![]).await.unwrap();
        std::fs::write(&path, "replacement").unwrap();
        verify_origin(path.clone(), vec![]).await.unwrap();
        assert!(
            verify_origin(
                directory.join("missing").join("..").join("deleted.rs"),
                vec![]
            )
            .await
            .is_err()
        );
        assert!(
            verify_removed(PathBuf::from("deleted.rs"), vec![])
                .await
                .is_err()
        );
        assert!(verify_removed(path.clone(), vec![]).await.is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(
            verify_removed(
                directory.join("missing").join("..").join("deleted.rs"),
                vec![]
            )
            .await
            .is_err()
        );
        assert!(verify_removed(directory.clone(), vec![]).await.is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.join("absent"), &path).unwrap();
            assert!(verify_removed(path, vec![]).await.is_err());
        }
    }
}
