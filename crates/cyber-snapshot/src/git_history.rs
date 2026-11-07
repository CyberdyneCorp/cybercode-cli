//! Exclusion evidence is part of the immutable snapshot identity, not mutable config.
use super::Shadow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

const LIMIT: usize = 1024 * 1024;
const PATH_LIMIT: usize = 4096;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    location: PathBuf,
    tree: String,
    excluded: Vec<String>,
}
fn oid(id: &str) -> bool {
    matches!(id.len(), 40 | 64)
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
pub(crate) fn excludes(paths: impl IntoIterator<Item = impl AsRef<str>>, path: &str) -> bool {
    paths.into_iter().any(|p| {
        let p = p.as_ref();
        p == path
            || p.strip_suffix('/')
                .is_some_and(|dir| path == dir || path.starts_with(p))
    })
}
impl Shadow {
    pub(super) async fn input(&self, args: &[&str], bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut child = self
            .command(args)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut input = child.stdin.take().ok_or("Missing Git input")?;
        input.write_all(bytes).await.map_err(|e| e.to_string())?;
        drop(input);
        let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Snapshot Git input failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output.stdout)
    }
    pub(crate) async fn omitted(&self) -> Result<Vec<String>, String> {
        let output = self
            .run(&[
                "ls-files",
                "--cached",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
                "-z",
                "--",
                &self.pathspec,
            ])
            .await?;
        if output.len() > LIMIT {
            return Err("Snapshot exclusions exceed record limit".into());
        }
        let mut paths = BTreeSet::new();
        for path in output.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let path = std::str::from_utf8(path)
                .map_err(|_| "Non-UTF-8 snapshot exclusion is unsupported")?;
            if !safe_path(path) {
                return Err("Invalid snapshot exclusion path".into());
            }
            paths.insert(path.to_owned());
            if paths.len() > PATH_LIMIT {
                return Err("Snapshot exclusion path limit exceeded".into());
            }
        }
        Ok(paths.into_iter().collect())
    }
    pub(super) async fn record_snapshot(
        &self,
        tree: &str,
        excluded: Vec<String>,
    ) -> Result<String, String> {
        if !oid(tree) {
            return Err("Invalid snapshot tree identity".into());
        }
        let record = Record {
            version: 1,
            location: self.worktree.join(&self.pathspec),
            tree: tree.into(),
            excluded,
        };
        let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        if bytes.len() > LIMIT {
            return Err("Snapshot exclusions exceed record limit".into());
        }
        let mut tag = format!("object {tree}\ntype tree\ntag cyber-snapshot-v1\ntagger Cyber Snapshot <snapshot@localhost> 0 +0000\n\n").into_bytes();
        tag.extend(bytes);
        let id = String::from_utf8(self.input(&["mktag"], &tag).await?)
            .map_err(|e| e.to_string())?
            .trim()
            .to_owned();
        if !oid(&id) {
            return Err("Invalid snapshot record identity".into());
        }
        // Retain tree and evidence together. Reclamation requires the complete
        // durable-reference/lease view, never a bare Git object age guess.
        self.run(&["update-ref", &format!("refs/cyber/snapshots/{id}"), &id])
            .await?;
        Ok(id)
    }
    pub(super) async fn recorded_exclusions(
        &self,
        id: &str,
    ) -> Result<Option<Vec<String>>, String> {
        self.snapshot_metadata(id)
            .await
            .map(|(_, excluded)| excluded)
    }

    /// Return the root identity from the same verified bytes as the exclusions.
    /// Resolving the tag again would allow a later substitution to change its root.
    pub(super) async fn snapshot_metadata(
        &self,
        id: &str,
    ) -> Result<(String, Option<Vec<String>>), String> {
        if !oid(id) {
            return Err("Invalid snapshot identity".into());
        }
        let kind = self.run(&["cat-file", "-t", id]).await?;
        if kind == b"tree\n" {
            return Ok((id.to_owned(), None));
        }
        if kind != b"tag\n" {
            return Err("Snapshot identity is not a tree or exclusion record".into());
        }
        let size = self.run(&["cat-file", "-s", id]).await?;
        let size = std::str::from_utf8(&size)
            .map_err(|e| e.to_string())?
            .trim()
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        if size > LIMIT + 512 {
            return Err("Snapshot exclusion record exceeds limit".into());
        }
        let bytes = self
            .run_bounded(&["cat-file", "tag", id], LIMIT + 512)
            .await?;
        super::images::verify_object(id, "tag", &bytes)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let (header, body) = text
            .split_once("\n\n")
            .ok_or("Invalid snapshot exclusion record")?;
        let record: Record = serde_json::from_str(body).map_err(|e| e.to_string())?;
        if record.version != 1
            || record.location != self.worktree.join(&self.pathspec)
            || !oid(&record.tree)
            || record.excluded.len() > PATH_LIMIT
            || record.excluded.iter().any(|p| !safe_path(p))
            || !header.starts_with(&format!(
                "object {}\ntype tree\ntag cyber-snapshot-v1\n",
                record.tree
            ))
        {
            return Err(
                "Snapshot exclusion record belongs to another Location or is corrupt".into(),
            );
        }
        Ok((record.tree, Some(record.excluded)))
    }
    pub(super) async fn excluded_between(
        &self,
        from: &str,
        to: &str,
    ) -> Result<BTreeSet<String>, String> {
        let mut paths = BTreeSet::new();
        paths.extend(self.recorded_exclusions(from).await?.unwrap_or_default());
        paths.extend(self.recorded_exclusions(to).await?.unwrap_or_default());
        Ok(paths)
    }
    pub(crate) async fn historical_exclusions(
        &self,
        ids: &[String],
    ) -> Result<Vec<String>, String> {
        let mut out = BTreeSet::new();
        for id in ids {
            match self.recorded_exclusions(id).await? {
                Some(paths) => {
                    out.extend(paths.into_iter().map(|p| format!("historical {id}: {p}")))
                }
                None => {
                    out.insert(format!(
                        "historical {id}: exclusions unavailable for legacy snapshot"
                    ));
                }
            }
        }
        Ok(out.into_iter().collect())
    }
}
