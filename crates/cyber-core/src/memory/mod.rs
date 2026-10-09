//! Validated memory documents and index snapshots; storage authority is separate.
mod secrets;
mod storage;
pub use storage::{
    InvalidMemory, MemoryCatalog, MemoryMutation, MemoryScope, MemoryStorageError, MemoryStore,
    PreparedMemory, ReviewedMemory,
};

use crate::env::EnvSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TRUNCATION_NOTICE: &str = "[memory index truncated; read MEMORY.md for more]";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("Invalid memory: {0}")]
    Invalid(&'static str),
    #[error("Memory rejected: content looks like a secret")]
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryMetadata {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: MemoryType,
}

impl MemoryMetadata {
    pub fn validate(&self) -> Result<(), MemoryError> {
        validate_name(&self.name)?;
        if self.description.trim().is_empty() || self.description.contains(['\n', '\r']) {
            return Err(MemoryError::Invalid(
                "description must be one nonempty line",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDocument {
    pub metadata: MemoryMetadata,
    pub body: String,
}

pub fn validate_name(name: &str) -> Result<(), MemoryError> {
    if name.is_empty()
        || !name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    {
        return Err(MemoryError::Invalid("name must be a kebab-case slug"));
    }
    Ok(())
}

impl MemoryDocument {
    pub fn parse(text: &str) -> Result<Self, MemoryError> {
        let normalized = text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .replace("\r\n", "\n");
        let rest = normalized
            .strip_prefix("---\n")
            .ok_or(MemoryError::Invalid("missing YAML frontmatter"))?;
        let (yaml, body) = split_frontmatter(rest).ok_or(MemoryError::Invalid(
            "missing frontmatter closing delimiter",
        ))?;
        let metadata: MemoryMetadata = serde_yaml_ng::from_str(yaml)
            .map_err(|_| MemoryError::Invalid("invalid YAML frontmatter"))?;
        Self::new(metadata, body.trim().into())
    }

    pub fn for_write(text: &str) -> Result<Self, MemoryError> {
        secrets::check(text)?;
        Self::parse(text)
    }

    pub fn new(metadata: MemoryMetadata, body: String) -> Result<Self, MemoryError> {
        metadata.validate()?;
        if body.trim().is_empty() {
            return Err(MemoryError::Invalid("body must be nonempty"));
        }
        if matches!(metadata.kind, MemoryType::Feedback | MemoryType::Project) {
            for heading in ["**Why:**", "**How to apply:**"] {
                if !body.lines().any(|line| {
                    line.trim_start()
                        .strip_prefix(heading)
                        .is_some_and(|value| !value.trim().is_empty())
                }) {
                    return Err(MemoryError::Invalid(
                        "feedback and project notes require Why and How to apply lines",
                    ));
                }
            }
        }
        Ok(Self { metadata, body })
    }

    /// Serialization is also write admission, including documents created directly.
    pub fn render_for_write(&self) -> Result<String, MemoryError> {
        Self::new(self.metadata.clone(), self.body.clone())?;
        let yaml = serde_yaml_ng::to_string(&self.metadata)
            .map_err(|_| MemoryError::Invalid("frontmatter serialization failed"))?;
        let text = format!("---\n{yaml}---\n\n{}\n", self.body.trim());
        secrets::check(&text)?;
        Ok(text)
    }
}

fn split_frontmatter(rest: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

pub fn render_index<'a>(
    documents: impl IntoIterator<Item = &'a MemoryDocument>,
) -> Result<String, MemoryError> {
    let documents: Vec<_> = documents.into_iter().collect();
    for document in &documents {
        MemoryDocument::new(document.metadata.clone(), document.body.clone())?;
    }
    render_metadata_index(documents.into_iter().map(|document| &document.metadata))
}

pub fn render_metadata_index<'a>(
    metadata: impl IntoIterator<Item = &'a MemoryMetadata>,
) -> Result<String, MemoryError> {
    let mut rows = BTreeMap::new();
    for metadata in metadata {
        metadata.validate()?;
        if rows.insert(metadata.name.as_str(), metadata).is_some() {
            return Err(MemoryError::Invalid("duplicate memory name"));
        }
    }
    let mut text = String::new();
    for metadata in rows.into_values() {
        text.push_str(&format!(
            "- [{}]({}.md) — {}\n",
            metadata.name,
            metadata.name,
            escape_index_text(&metadata.description)
        ));
    }
    secrets::check(&text)?;
    Ok(text)
}

fn escape_index_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSnapshot {
    pub text: String,
    pub truncated: bool,
}

pub fn index_snapshot(text: &str) -> IndexSnapshot {
    let line_end = text
        .split_inclusive('\n')
        .take(200)
        .map(str::len)
        .sum::<usize>();
    let mut end = line_end.min(25_000);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < text.len();
    IndexSnapshot::from_prefix(&text[..end], truncated)
}

impl IndexSnapshot {
    pub(crate) fn from_prefix(prefix: &str, truncated: bool) -> Self {
        let mut text = prefix.to_string();
        if truncated {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(TRUNCATION_NOTICE);
        }
        Self { text, truncated }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySettings {
    pub enabled: bool,
    pub generate: bool,
}
impl MemorySettings {
    pub fn from_config(config: &Value, env: &dyn EnvSource) -> Result<Self, MemoryError> {
        let settings = config.get("memory");
        if settings.is_some_and(|value| !value.is_object()) {
            return Err(MemoryError::Invalid("memory settings must be an object"));
        }
        let boolean = |key| match settings.and_then(|value| value.get(key)) {
            None => Ok(true),
            Some(Value::Bool(value)) => Ok(*value),
            _ => Err(MemoryError::Invalid(
                "memory enabled and generate settings must be boolean",
            )),
        };
        Ok(Self {
            enabled: boolean("enabled")? && !env.flag("CYBER_DISABLE_MEMORY"),
            generate: boolean("generate")?,
        })
    }
    pub fn writable(self) -> bool {
        self.enabled && self.generate
    }
}

pub fn directory(data: &Path, project_id: &str) -> Result<PathBuf, MemoryError> {
    let valid = project_id == "global"
        || project_id.strip_prefix("prj_").is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
    if !valid {
        return Err(MemoryError::Invalid("invalid project memory identity"));
    }
    Ok(data.join("memory").join(project_id))
}
