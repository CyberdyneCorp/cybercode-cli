//! Immutable Git objects handed to the synchronous native journal worker.
use super::{Shadow, history, images};
use cyber_server::runtime::SnapshotRestoreRequest;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Image {
    pub mode: String,
    pub object: String,
    pub bytes: Vec<u8>,
}

impl Image {
    pub(crate) fn verify(&self) -> Result<(), String> {
        images::verify(&self.object, &self.bytes)
    }
}

pub(crate) struct PreparedTree {
    pub id: String,
    /// Paths relative to the exact Location, rather than the Git worktree root.
    pub images: BTreeMap<String, Image>,
}

pub(crate) struct PreparedRestore {
    pub directory: PathBuf,
    pub worktree: PathBuf,
    pub prefix: String,
    pub target: PreparedTree,
    pub recorded: PreparedTree,
    pub expected: PreparedTree,
    pub exclusions: Vec<String>,
    pub allowed: Option<BTreeSet<String>>,
}

impl Shadow {
    /// No source reads, Git index writes or merge subprocesses occur here.
    pub(crate) async fn prepare_restore(
        &self,
        request: &SnapshotRestoreRequest,
        max_file: u64,
        max_total: u64,
    ) -> Result<PreparedRestore, String> {
        let prefix = if self.pathspec == "." {
            ""
        } else {
            &self.pathspec
        };
        let directory = self.worktree.join(prefix);
        if directory != std::path::Path::new(&request.directory) {
            return Err("Git image handoff belongs to another Location".into());
        }
        let ids = [&request.target, &request.recorded, &request.expected];
        let mut excluded = BTreeSet::new();
        for id in ids {
            excluded.extend(self.recorded_exclusions(id).await?.unwrap_or_default());
        }
        let exclusions = self
            .historical_exclusions(&ids.map(|id| id.to_owned()))
            .await?;
        let allowed = request
            .paths
            .as_ref()
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| relative(prefix, path))
                    .collect::<Result<_, _>>()
            })
            .transpose()?;
        let mut remaining = max_total;
        let mut trees = Vec::new();
        for id in ids {
            let mut entries = BTreeMap::new();
            for (path, (mode, object)) in self.image_catalog(id).await?.iter() {
                if history::excludes(&excluded, path) {
                    continue;
                }
                if !matches!(mode.as_str(), "100644" | "100755" | "120000") {
                    return Err("Unsupported Git journal image kind".into());
                }
                if entries.len() >= 4096 {
                    return Err("Git journal image count exceeds limit".into());
                }
                let limit = usize::try_from(max_file.min(remaining).min(64 * 1024 * 1024))
                    .map_err(|_| "Git journal image limit overflow")?;
                let bytes = self
                    .run_bounded(&["cat-file", "blob", object], limit)
                    .await?;
                images::verify(object, &bytes)?;
                remaining = remaining
                    .checked_sub(bytes.len() as u64)
                    .ok_or("Git journal image handoff exceeds limit")?;
                entries.insert(
                    relative(prefix, path)?,
                    Image {
                        mode: mode.clone(),
                        object: object.clone(),
                        bytes,
                    },
                );
            }
            trees.push(PreparedTree {
                id: id.clone(),
                images: entries,
            });
        }
        let mut trees = trees.into_iter();
        Ok(PreparedRestore {
            directory,
            worktree: self.worktree.clone(),
            prefix: prefix.to_owned(),
            target: trees.next().ok_or("Missing target images")?,
            recorded: trees.next().ok_or("Missing recorded images")?,
            expected: trees.next().ok_or("Missing expected images")?,
            exclusions,
            allowed,
        })
    }
}

fn relative(prefix: &str, path: &str) -> Result<String, String> {
    images::validate_path(path)?;
    let path = if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix)
            .and_then(|path| path.strip_prefix('/'))
            .ok_or("Git image path is outside the Location")?
    };
    images::validate_path(path)?;
    Ok(path.to_owned())
}
