//! Owner recovery contracts. A filesystem outcome alone never acknowledges native work.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotRecoveryAction {
    Keep,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotRecoveryImage {
    pub hash: String,
    pub bytes: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// Expected images and proposed copies need not identify a live inode.
    pub native_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotRecoveryPath {
    pub path: String,
    /// `applied`, `unapplied`, or `foreign`; foreign files are never rollback targets.
    pub state: String,
    pub current: Option<SnapshotRecoveryImage>,
    pub recorded_before: Option<SnapshotRecoveryImage>,
    pub recorded_after: Option<SnapshotRecoveryImage>,
    /// Current private slot: staged target or displaced preimage, without inventing ownership.
    pub retained: Option<SnapshotRecoveryImage>,
    pub proposed: Option<SnapshotRecoveryImage>,
    pub changes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotRecoveryReview {
    pub journal_id: String,
    pub action: SnapshotRecoveryAction,
    pub origin: Option<super::SnapshotOrigin>,
    pub filesystem_complete: bool,
    pub acknowledged: bool,
    /// Absent when conflicts or unavailable historical evidence prohibit confirmation.
    pub confirmation: Option<String>,
    pub paths: Vec<SnapshotRecoveryPath>,
    /// None means historical exclusions were not retained; not an empty exclusion set.
    pub exclusions: Option<Vec<String>>,
    /// Recorded created parents remain protected until acknowledged safe cleanup.
    pub recorded_created_parents: Option<Vec<String>>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecoveryConfirmation {
    pub journal_id: String,
    pub action: SnapshotRecoveryAction,
    pub confirmation: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecoverySelection {
    pub journal_id: String,
    pub action: SnapshotRecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotRecoveryReceipt {
    pub journal_id: String,
    pub resolution_id: String,
    pub request_id: String,
    pub action: SnapshotRecoveryAction,
    pub confirmation: String,
    pub original: super::SnapshotOrigin,
    pub filesystem_complete: bool,
}
