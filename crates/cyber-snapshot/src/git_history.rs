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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
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
        policy: Option<String>,
        raw: Option<String>,
    ) -> Result<String, String> {
        if !oid(tree) {
            return Err("Invalid snapshot tree identity".into());
        }
        if policy.as_ref().is_some_and(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err("Invalid Git policy fingerprint".into());
        }
        if raw
            .as_ref()
            .is_some_and(|raw| !oid(raw) || raw.len() != tree.len())
            || (raw.is_some() && policy.is_none())
        {
            return Err("Invalid Git raw capture identity".into());
        }
        let root = if let Some(raw) = &raw {
            self.envelope_tree(tree, raw).await?
        } else {
            tree.to_owned()
        };
        let record = Record {
            version: if raw.is_some() {
                3
            } else if policy.is_some() {
                2
            } else {
                1
            },
            location: self.worktree.join(&self.pathspec),
            tree: tree.into(),
            excluded,
            policy,
            raw,
        };
        let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        if bytes.len() > LIMIT {
            return Err("Snapshot exclusions exceed record limit".into());
        }
        let mut tag = format!("object {root}\ntype tree\ntag cyber-snapshot-v{}\ntagger Cyber Snapshot <snapshot@localhost> 0 +0000\n\n", record.version).into_bytes();
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
    async fn envelope_tree(&self, tree: &str, raw: &str) -> Result<String, String> {
        let records =
            format!("040000 tree {tree}\tcanonical\0") + &format!("040000 tree {raw}\traw\0");
        let id = String::from_utf8(self.input(&["mktree", "-z"], records.as_bytes()).await?)
            .map_err(|_| "Invalid Git envelope identity")?
            .trim()
            .to_owned();
        if !oid(&id) {
            return Err("Invalid Git envelope identity".into());
        }
        Ok(id)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) async fn native_image_catalog(
        &self,
        id: &str,
    ) -> Result<super::images::Catalog, String> {
        let record = self
            .read_record(id)
            .await?
            .ok_or("Git checkpoint lacks raw capture proof")?;
        let raw = record.raw.ok_or("Git checkpoint lacks raw capture proof")?;
        self.read_catalog(raw, &record.excluded).await
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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) async fn snapshot_policy(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.read_record(id).await?.and_then(|record| record.policy))
    }
    pub(super) async fn snapshot_metadata(
        &self,
        id: &str,
    ) -> Result<(String, Option<Vec<String>>), String> {
        Ok(match self.read_record(id).await? {
            Some(record) => (record.tree, Some(record.excluded)),
            None => (id.to_owned(), None),
        })
    }

    async fn read_record(&self, id: &str) -> Result<Option<Record>, String> {
        if !oid(id) {
            return Err("Invalid snapshot identity".into());
        }
        let kind = self.run(&["cat-file", "-t", id]).await?;
        if kind == b"tree\n" {
            return Ok(None);
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
        let root = header
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("object "))
            .filter(|root| oid(root))
            .ok_or("Invalid Git snapshot root")?;
        if !matches!(
            (
                record.version,
                record.policy.as_deref(),
                record.raw.as_deref()
            ),
            (1, None, None) | (2, Some(_), None) | (3, Some(_), Some(_))
        ) || record.policy.as_ref().is_some_and(|policy| {
            policy.len() != 64
                || !policy
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) || record.location != self.worktree.join(&self.pathspec)
            || !oid(&record.tree)
            || record.excluded.len() > PATH_LIMIT
            || record.excluded.iter().any(|p| !safe_path(p))
            || !header.starts_with(&format!(
                "object {}\ntype tree\ntag cyber-snapshot-v{}\n",
                root, record.version
            ))
        {
            return Err(
                "Snapshot exclusion record belongs to another Location or is corrupt".into(),
            );
        }
        if let Some(raw) = &record.raw {
            if !oid(raw) || raw.len() != record.tree.len() || root.len() != raw.len() {
                return Err("Invalid Git raw capture identity".into());
            }
            let bytes = self.run_bounded(&["cat-file", "tree", root], 1024).await?;
            super::images::verify_object(root, "tree", &bytes)?;
            if super::images::tree_entries(&bytes, root.len() / 2)?
                != vec![
                    ("canonical".into(), "040000".into(), record.tree.clone()),
                    ("raw".into(), "040000".into(), raw.clone()),
                ]
            {
                return Err("Invalid Git capture envelope".into());
            }
        } else if root != record.tree {
            return Err("Git snapshot root does not match its record".into());
        }
        Ok(Some(record))
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

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod policy_tests {
    use super::*;
    #[tokio::test]
    async fn versioned_tags_keep_legacy_metadata_readable_without_promoting_policy_proof() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("work");
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join("file"), b"source").unwrap();
        let shadow = Shadow::locate(&base.join("data/snapshot"), &root).unwrap();
        let checkpoint = shadow.track(&[], 2 * 1024 * 1024).await.unwrap();
        let record = shadow.read_record(&checkpoint.tree).await.unwrap().unwrap();
        assert_eq!(record.version, 3);
        assert!(record.raw.is_some());
        assert!(record.policy.is_some());
        let policy_only = shadow
            .record_snapshot(
                &record.tree,
                record.excluded.clone(),
                record.policy.clone(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            shadow
                .read_record(&policy_only)
                .await
                .unwrap()
                .unwrap()
                .version,
            2
        );
        assert!(shadow.native_image_catalog(&policy_only).await.is_err());
        // A valid tag hash cannot substitute its canonical tree for the envelope.
        let mut wrong_envelope = format!("object {}\ntype tree\ntag cyber-snapshot-v3\ntagger Cyber Snapshot <snapshot@localhost> 0 +0000\n\n", record.tree).into_bytes();
        wrong_envelope.extend(serde_json::to_vec(&record).unwrap());
        let wrong_envelope =
            String::from_utf8(shadow.input(&["mktag"], &wrong_envelope).await.unwrap()).unwrap();
        assert!(
            shadow
                .snapshot_metadata(wrong_envelope.trim())
                .await
                .is_err()
        );
        let legacy = shadow
            .record_snapshot(&record.tree, record.excluded.clone(), None, None)
            .await
            .unwrap();
        assert_eq!(
            shadow.read_record(&legacy).await.unwrap().unwrap().version,
            1
        );
        assert!(shadow.snapshot_policy(&legacy).await.unwrap().is_none());
        assert!(
            shadow
                .snapshot_policy(&record.tree)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            shadow.blob(&legacy, "file").await.unwrap().unwrap().1,
            b"source"
        );
        assert!(
            shadow
                .record_snapshot(&record.tree, vec![], Some("invalid".into()), None)
                .await
                .is_err()
        );
        let mut invalid = shadow.read_record(&legacy).await.unwrap().unwrap();
        invalid.policy = Some("a".repeat(64));
        let body = serde_json::to_vec(&invalid).unwrap();
        let mut tag = format!("object {}\ntype tree\ntag cyber-snapshot-v1\ntagger Cyber Snapshot <snapshot@localhost> 0 +0000\n\n", record.tree).into_bytes();
        tag.extend(body);
        let id = String::from_utf8(shadow.input(&["mktag"], &tag).await.unwrap()).unwrap();
        assert!(shadow.snapshot_metadata(id.trim()).await.is_err());
    }
}
