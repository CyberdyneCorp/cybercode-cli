//! Verified Git directory metadata. No source mutations or journal authority.
use super::{Shadow, history, images};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
pub(super) type CatalogCache =
    BTreeMap<(std::path::PathBuf, std::path::PathBuf, String), Arc<images::Catalog>>;
use tokio::io::{AsyncRead, AsyncReadExt};

async fn bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("Git snapshot metadata output exceeds limit".into());
    }
    Ok(bytes)
}

impl Shadow {
    pub(super) async fn run_bounded(&self, args: &[&str], limit: usize) -> Result<Vec<u8>, String> {
        let mut child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child.stdout.take().ok_or("Missing Git metadata output")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("Missing Git metadata diagnostics")?;
        let reads = tokio::try_join!(bounded(stdout, limit), bounded(stderr, 64 * 1024));
        let (bytes, diagnostics) = match reads {
            Ok(reads) => reads,
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(error);
            }
        };
        let status = child.wait().await.map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!(
                "Git snapshot metadata: {}",
                String::from_utf8_lossy(&diagnostics).trim()
            ));
        }
        Ok(bytes)
    }

    pub(super) async fn image_catalog(
        &self,
        snapshot: &str,
    ) -> Result<Arc<images::Catalog>, String> {
        let key = (
            self.git_dir.clone(),
            self.worktree.join(&self.pathspec),
            snapshot.to_owned(),
        );
        if let Some(catalog) = self
            .catalogs
            .lock()
            .map_err(|_| "Git image catalog lock failed")?
            .get(&key)
            .cloned()
        {
            return Ok(catalog);
        }
        // Verifies immutable exclusion-tag bytes and the Location before resolving
        // its tree. Bare legacy trees remain readable without invented exclusions.
        let (root, excluded) = self.snapshot_metadata(snapshot).await?;
        let excluded = excluded.unwrap_or_default();
        let catalog = Arc::new(self.read_catalog(root, &excluded).await?);
        let mut cache = self
            .catalogs
            .lock()
            .map_err(|_| "Git image catalog lock failed")?;
        // Per-operation immutable inputs; eviction cannot invalidate held images.
        if cache.len() >= 4 {
            cache.clear();
        }
        cache.insert(key, catalog.clone());
        Ok(catalog)
    }

    async fn read_catalog(
        &self,
        root: String,
        excluded: &[String],
    ) -> Result<images::Catalog, String> {
        let mut pending = vec![(String::new(), root, 0usize)];
        let mut catalog = BTreeMap::new();
        let mut size = 0usize;
        let mut directories = 0usize;
        let mut catalog_bytes = 0usize;
        let mut pending_bytes = 0usize;
        while let Some((prefix, object, depth)) = pending.pop() {
            pending_bytes = pending_bytes.saturating_sub(prefix.len() + object.len());
            directories += 1;
            if depth > 64 || directories > 65536 {
                return Err("Git snapshot directory traversal exceeds limit".into());
            }
            let bytes = self
                .run_bounded(&["cat-file", "tree", &object], images::TREE_LIMIT)
                .await?;
            images::verify_object(&object, "tree", &bytes)?;
            size = size
                .checked_add(bytes.len())
                .ok_or("Git metadata size overflow")?;
            if size > images::GRAPH_LIMIT {
                return Err("Git snapshot metadata graph exceeds limit".into());
            }
            for (name, mode, child) in images::tree_entries(&bytes, object.len() / 2)? {
                let path = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                images::validate_path(&path)?;
                if history::excludes(excluded, &path) {
                    continue;
                }
                let within = self.pathspec == "."
                    || path == self.pathspec
                    || path.starts_with(&format!("{}/", self.pathspec));
                let ancestor = self.pathspec.starts_with(&format!("{path}/"));
                if mode == "040000" {
                    if within || ancestor {
                        pending_bytes = pending_bytes
                            .checked_add(path.len() + child.len())
                            .ok_or("Git metadata size overflow")?;
                        if pending_bytes > images::GRAPH_LIMIT {
                            return Err("Git directory work queue exceeds limit".into());
                        }
                        pending.push((path, child, depth + 1));
                    }
                } else if within {
                    catalog_bytes = catalog_bytes
                        .checked_add(path.len() + mode.len() + child.len())
                        .ok_or("Git image catalog size overflow")?;
                    if catalog_bytes > images::GRAPH_LIMIT {
                        return Err("Git image catalog byte limit exceeded".into());
                    }
                    if catalog.len() >= 65536 || catalog.insert(path, (mode, child)).is_some() {
                        return Err(
                            "Git image catalog exceeds limits or contains duplicates".into()
                        );
                    }
                }
            }
        }
        Ok(catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn over_limit_git_output_returns_failure_and_the_reader_remains_usable() {
        let temp = tempfile::tempdir().unwrap();
        let shadow = Shadow {
            git_dir: temp.path().join("shadow"),
            worktree: temp.path().to_path_buf(),
            common_dir: temp.path().join("common"),
            pathspec: ".".into(),
            catalogs: Default::default(),
            private_roots: vec![],
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            capture_settings: super::super::prepared::CaptureSettings {
                ignore: vec![],
                max_untracked: 2 * 1024 * 1024,
            },
        };
        assert!(
            shadow
                .run_bounded(&["--version"], 0)
                .await
                .unwrap_err()
                .contains("exceeds limit")
        );
        assert!(
            shadow
                .run_bounded(&["--version"], 1024)
                .await
                .unwrap()
                .starts_with(b"git version ")
        );
    }

    #[tokio::test]
    async fn metadata_reads_reject_overflow_without_accepting_a_prefix() {
        assert_eq!(bounded(&b"abc"[..], 3).await.unwrap(), b"abc");
        assert!(bounded(&b"abcd"[..], 3).await.is_err());
    }
}
