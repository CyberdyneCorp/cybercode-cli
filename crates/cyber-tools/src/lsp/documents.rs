use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{LspError, StdioConnection};

pub(crate) const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DOCUMENTS: usize = 128;

#[derive(Default)]
pub(super) struct Documents(BTreeMap<PathBuf, (i32, [u8; 32])>);

impl Documents {
    pub async fn open(
        &mut self,
        connection: &mut StdioConnection,
        path: PathBuf,
        text: String,
        language: String,
    ) -> Result<(), LspError> {
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let document_uri = uri(&path)?;
        let (method, params, version) = match self.0.get(&path) {
            Some((_, previous)) if previous == &digest => return Ok(()),
            Some((version, _)) => {
                let version = version
                    .checked_add(1)
                    .ok_or(LspError::Protocol("document version exhausted"))?;
                (
                    "textDocument/didChange",
                    json!({"textDocument":{"uri":document_uri,"version":version},"contentChanges":[{"text":text}]}),
                    version,
                )
            }
            None => {
                if self.0.len() == MAX_DOCUMENTS {
                    let first = self.0.first_key_value().unwrap().0.clone();
                    notify(
                        connection,
                        "textDocument/didClose",
                        json!({"textDocument":{"uri":uri(&first)?}}),
                    )
                    .await?;
                    self.0.remove(&first);
                }
                (
                    "textDocument/didOpen",
                    json!({"textDocument":{"uri":document_uri,"languageId":language,"version":1,"text":text}}),
                    1,
                )
            }
        };
        notify(connection, method, params).await?;
        self.0.insert(path, (version, digest));
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
