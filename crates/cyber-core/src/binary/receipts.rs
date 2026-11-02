//! Common wire receipts/reviews. Their availability does not imply native write
//! support on every platform; the service advertises admission separately.
use super::draft::Review;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BinaryOutcome")]
pub enum Outcome {
    Acknowledged,
    Conflict,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BinaryFileEffect")]
pub enum FileEffect {
    None,
    Changed,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "BinaryOperation")]
pub struct Operation {
    pub id: String,
    pub kind: String,
    pub payload_sha256: String,
    pub outcome: Outcome,
    pub revision: String,
    pub file_effect: FileEffect,
    pub reason: Option<String>,
    pub notification_pending: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "BinarySaveReview")]
pub struct SaveReview {
    pub draft: Review,
    pub permissions: u32,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub metadata_sha256: String,
    pub effects: String,
    pub fingerprint: String,
}

/// Shared saved-source metadata. It never contains unsaved command buffers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryVersion")]
pub struct Version {
    pub version_id: String,
    pub document_id: String,
    pub operation_id: String,
    pub kind: String,
    pub path: String,
    pub before_sha256: String,
    pub after_sha256: String,
    pub before_length: super::Offset,
    pub after_length: super::Offset,
    pub original_bytes: super::Offset,
    pub proposed_bytes: super::Offset,
    pub range_count: usize,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryVersions")]
pub struct Versions {
    pub versions: Vec<Version>,
    pub next_cursor: Option<String>,
}
