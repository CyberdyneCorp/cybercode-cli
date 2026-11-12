//! Metadata captured while context is observed, without retaining duplicate text.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context::{Observation, Observed};

pub const OBSERVED: &str = "session.context.observed.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// No observation metadata was recorded by the historical writer.
    Unknown,
    Active,
    Absent,
    RetainedUnavailable,
    Unavailable,
}

/// Size and modification time of a file when it was read. Inspection compares them with
/// the file's current metadata to report an edit without reading the file again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileStamp {
    pub bytes: u64,
    /// Nanoseconds since the Unix epoch; absent where the platform reports none.
    pub modified_ns: Option<u64>,
}

impl FileStamp {
    pub fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            bytes: metadata.len(),
            modified_ns: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| u64::try_from(since.as_nanos()).ok()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceMetadata {
    pub origin: String,
    pub status: SourceStatus,
    #[serde(default)]
    pub pending_update: bool,
    pub estimated_tokens: Option<u64>,
    pub file_paths: Vec<String>,
    pub omitted_file_paths: usize,
    /// Stamps of the listed files taken while they were read. Historical records and
    /// sources that are not read from files have none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_stamps: BTreeMap<String, FileStamp>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourcesObserved {
    pub epoch: u32,
    pub sources: BTreeMap<String, SourceMetadata>,
}

/// Observe only at the ordinary request boundary. Inspection uses the committed
/// result and never invokes a source to refresh these records.
pub fn capture(
    observed: &Observation,
    snapshot: &BTreeMap<String, String>,
    previous: &BTreeMap<String, SourceMetadata>,
) -> BTreeMap<String, SourceMetadata> {
    let mut records = previous.clone();
    records.retain(|id, _| snapshot.contains_key(id) || observed.sources.contains_key(id));
    records.extend(observed.sources.iter().map(|(id, value)| {
        let status = match value {
            Observed::Value(_) => SourceStatus::Active,
            Observed::Absent => SourceStatus::Absent,
            Observed::Unavailable(_) if snapshot.contains_key(id) => {
                SourceStatus::RetainedUnavailable
            }
            Observed::Unavailable(_) => SourceStatus::Unavailable,
        };
        let (file_paths, omitted_file_paths, file_stamps) = if status
            == SourceStatus::RetainedUnavailable
        {
            previous.get(id).map_or_else(
                || (Vec::new(), 0, BTreeMap::new()),
                |record| {
                    (
                        record.file_paths.clone(),
                        record.omitted_file_paths,
                        record.file_stamps.clone(),
                    )
                },
            )
        } else {
            let (paths, omitted) = bounded_paths(observed.files.get(id).map_or(&[], Vec::as_slice));
            let stamps = observed
                .stamps
                .get(id)
                .map_or_else(BTreeMap::new, |stamps| {
                    paths
                        .iter()
                        .filter_map(|path| Some((path.clone(), *stamps.get(path)?)))
                        .collect()
                });
            (paths, omitted, stamps)
        };
        (
            id.clone(),
            SourceMetadata {
                origin: if status == SourceStatus::RetainedUnavailable {
                    previous.get(id).map(|record| record.origin.clone())
                } else {
                    observed.origins.get(id).cloned()
                }
                .unwrap_or_else(|| "unknown".into()),
                status,
                pending_update: observed.pending.contains(id),
                estimated_tokens: snapshot.get(id).map(|text| (text.len() / 4) as u64),
                file_paths,
                omitted_file_paths,
                file_stamps,
            },
        )
    }));
    records
}

fn bounded_paths(paths: &[String]) -> (Vec<String>, usize) {
    let mut bytes = 0usize;
    let kept: Vec<_> = paths
        .iter()
        .take(128)
        .take_while(|path| {
            bytes = bytes.saturating_add(path.len());
            bytes <= 16_384
        })
        .cloned()
        .collect();
    let omitted = paths.len() - kept.len();
    (kept, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_bounds_report_every_omitted_path() {
        let paths = vec!["x".repeat(1024); 129];
        let (kept, omitted) = bounded_paths(&paths);
        assert_eq!(kept.len(), 16);
        assert_eq!(kept.iter().map(String::len).sum::<usize>(), 16_384);
        assert_eq!(kept.len() + omitted, paths.len());
    }
}
