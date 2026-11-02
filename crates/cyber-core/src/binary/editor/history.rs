//! Shared acknowledged source history across private document namespaces. A page
//! visits at most 64 matching namespaces; empty pages may carry a continuation.
use super::*;
use crate::binary::{receipts::Versions, storage::StorageRoot};

impl StorageRoot {
    pub fn source_versions(
        &self,
        source: &Document,
        after: Option<&str>,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Versions, Error> {
        if limit == 0 || limit > 64 {
            return Err(Error::Limit);
        }
        let (position, prior) = after.map(cursor).transpose()?.unwrap_or((0, None));
        let ids = self.source_document_ids(source, &cancelled)?;
        if position > ids.len() {
            return Err(Error::Invalid);
        }
        let mut found = Vec::new();
        let mut continuation = None;
        for (visited, (index, id)) in ids.iter().enumerate().skip(position).take(64).enumerate() {
            let after = if index == position { prior } else { None };
            let versions = self.versions(source, id, after, limit - found.len(), &cancelled)?;
            found.extend(versions.versions);
            if let Some(version) = versions.next_cursor {
                continuation = Some(format!("{index}:{version}"));
                break;
            }
            if found.len() == limit || visited == 63 {
                // Positions reveal no private namespace identity for unsaved
                // candidates. Only acknowledged versions return document IDs.
                continuation = (index + 1 < ids.len()).then(|| format!("{}:", index + 1));
                break;
            }
        }
        source.validate()?;
        self.directory()?;
        Ok(Versions {
            versions: found,
            next_cursor: continuation,
        })
    }
}
fn cursor(value: &str) -> Result<(usize, Option<&str>), Error> {
    let (position, version) = value.split_once(':').ok_or(Error::Invalid)?;
    let index = position.parse::<usize>().map_err(|_| Error::Invalid)?;
    if index > 4096
        || index.to_string() != position
        || (!version.is_empty() && !super::versions::valid_version(version))
    {
        return Err(Error::Invalid);
    }
    Ok((index, (!version.is_empty()).then_some(version)))
}
