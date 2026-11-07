//! Read-only user-index observations for large-untracked eligibility handoff.
use super::Shadow;
use crate::copy::config_inputs::ConfigInputs;
use std::path::PathBuf;

#[derive(PartialEq, Eq)]
struct Layout {
    directory: PathBuf,
    index: PathBuf,
}

pub(super) struct UserIndex {
    pub inputs: ConfigInputs,
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
}

impl Shadow {
    pub(super) async fn user_index(&self) -> Result<UserIndex, String> {
        // Observe discovery routing before Git resolves any metadata paths.
        let marker = self.worktree.join(".git");
        let metadata = std::fs::symlink_metadata(&marker).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Unsupported user Git marker link".into());
        }
        let mut proof = ConfigInputs::capture_files(if metadata.is_dir() {
            vec![]
        } else {
            vec![marker.clone()]
        })?;
        let mut contexts = vec![self.worktree.clone()];
        if metadata.is_dir() {
            contexts.push(marker);
        }
        proof.bind_contexts(contexts)?;
        let layout = self.user_index_layout().await?;
        let mut paths = vec![
            layout.index.clone(),
            layout.directory.join("commondir"),
            layout.directory.join("gitdir"),
        ];
        let mut files = vec![layout.index.clone()];
        files.extend(shared_indexes(&layout.directory)?);
        paths.extend(files.iter().skip(1).cloned());
        let mut index = ConfigInputs::capture_files(paths)?;
        index.bind_contexts(vec![layout.directory.clone()])?;
        proof.merge(index)?;
        if self.user_index_layout().await? != layout {
            return Err("User Git index layout changed during collection".into());
        }
        proof.verify()?;
        Ok(UserIndex {
            inputs: proof,
            directory: layout.directory,
            files,
        })
    }

    async fn user_index_layout(&self) -> Result<Layout, String> {
        let mut command = self.command(&[
            "rev-parse",
            "--absolute-git-dir",
            "--path-format=absolute",
            "--git-path",
            "index",
        ]);
        command
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_OPTIONAL_LOCKS", "0");
        let bytes =
            Self::bounded_command(command, 32 * 1024, std::time::Duration::from_secs(10)).await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "Non-UTF-8 user Git index path")?;
        let lines: Vec<_> = text.lines().collect();
        if lines.len() != 2 {
            return Err("Unsupported user Git index layout".into());
        }
        let directory = PathBuf::from(lines[0]);
        let index = PathBuf::from(lines[1]);
        if !directory.is_absolute() || index != directory.join("index") {
            return Err("Unsupported user Git index locator".into());
        }
        Ok(Layout { directory, index })
    }
}

// Resolving --shared-index-path through Git refreshes shared-index timestamps.
// Observe every bounded candidate instead: this includes the active dependency
// without reading the index through Git, and the index observation binds selection.
fn shared_indexes(directory: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for (count, entry) in std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .enumerate()
    {
        if count >= 4096 {
            return Err("User Git index directory exceeds observation limit".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(b"sharedindex.")
        {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}
