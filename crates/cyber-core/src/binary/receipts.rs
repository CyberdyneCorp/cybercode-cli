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
