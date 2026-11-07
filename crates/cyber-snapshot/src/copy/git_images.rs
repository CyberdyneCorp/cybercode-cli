//! Materialize old copy images only from their original Location-bound manifests.
use super::{ENTRY_LIMIT, Store, storage};
use crate::git::prepared::{Image, ImageSource, PreparedRestore, PreparedTree};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Imported<'a> {
    pub target: Cow<'a, PreparedTree>,
    pub recorded: Cow<'a, PreparedTree>,
    pub excluded: BTreeSet<String>,
    pub reports: Vec<String>,
}

impl Store {
    pub(super) fn import_git_images<'a>(
        &self,
        prepared: &'a PreparedRestore,
    ) -> storage::Result<Imported<'a>> {
        let mut remaining = self.options.max_total;
        for tree in [&prepared.target, &prepared.recorded, &prepared.expected] {
            for image in tree.images.values() {
                remaining = remaining
                    .checked_sub(image.bytes.len() as u64)
                    .ok_or("Mixed image handoff exceeds byte limit")?;
            }
        }
        let mut excluded = BTreeSet::new();
        let mut reports = vec![];
        let mut load =
            |tree: &'a PreparedTree, label: &str| -> storage::Result<Cow<'a, PreparedTree>> {
                if ImageSource::for_id(&tree.id)? == ImageSource::Git {
                    return Ok(Cow::Borrowed(tree));
                }
                if !tree.images.is_empty() {
                    return Err("Copy image authority requires its original manifest".into());
                }
                let manifest = self.load(&tree.id)?;
                for skipped in manifest.skipped {
                    let (_, path) = skipped
                        .split_once(": ")
                        .ok_or("Invalid historical copy exclusion")?;
                    super::tree::safe_path(path)?;
                    excluded.insert(path.to_owned());
                    reports.push(format!("{label}: {skipped}"));
                }
                let mut images = BTreeMap::new();
                for (path, entry) in manifest.files {
                    super::tree::safe_path(&path)?;
                    if entry.length > self.options.max_file || images.len() >= ENTRY_LIMIT {
                        return Err("Copy journal image exceeds file/count limit".into());
                    }
                    remaining = remaining
                        .checked_sub(entry.length)
                        .ok_or("Mixed image handoff exceeds byte limit")?;
                    images.insert(
                        path,
                        Image {
                            source: ImageSource::Copy,
                            mode: if entry.mode & 0o111 == 0 {
                                "100644"
                            } else {
                                "100755"
                            }
                            .into(),
                            bytes: self.bytes(&entry)?,
                            object: entry.object,
                        },
                    );
                }
                Ok(Cow::Owned(PreparedTree {
                    id: tree.id.clone(),
                    images,
                }))
            };
        let target = load(&prepared.target, "target")?;
        let recorded = load(&prepared.recorded, "recorded")?;
        Ok(Imported {
            target,
            recorded,
            excluded,
            reports,
        })
    }
}

impl Imported<'_> {
    pub fn excludes(&self, prepared: &PreparedRestore, path: &str) -> bool {
        let full = if prepared.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", prepared.prefix)
        };
        crate::git::history::excludes(&prepared.historical_excluded, &full)
            || self
                .excluded
                .iter()
                .any(|excluded| path == excluded || path.starts_with(&format!("{excluded}/")))
    }
}
