//! Saved binary references in the shared Session diff. The immutable source
//! catalog is independent of Git and of notification/hook delivery. No private
//! command journal or byte image is opened to build this summary.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::FileDecision;
use super::{FileDiff, Runtime, RuntimeError, TurnContext};

impl Runtime {
    pub(super) async fn merge_binary_snapshot_diffs(
        &self,
        turn: &TurnContext,
        files: &mut Vec<FileDiff>,
    ) -> Result<(), RuntimeError> {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (turn, files);
            Ok(())
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let mut grouped = std::collections::BTreeMap::<
                String,
                (
                    std::path::PathBuf,
                    Vec<cyber_core::binary::receipts::Version>,
                ),
            >::new();
            for record in self.binary_snapshot_records(turn).await? {
                let path = record.resource.path().to_path_buf();
                if self
                    .file_decisions(turn, "read", std::slice::from_ref(&path))
                    .await
                    .first()
                    != Some(&FileDecision::Allow)
                {
                    files.retain(|file| file.file != record.version.path);
                    continue;
                }
                let Some(version) = self.binary_snapshot_reference(turn, record).await? else {
                    continue;
                };
                grouped
                    .entry(version.path.clone())
                    .or_insert_with(|| (path, Vec::new()))
                    .1
                    .push(version);
            }
            if self
                .file_turn(
                    Some(&turn.session_id),
                    std::path::Path::new(&turn.directory),
                )
                .await?
                != *turn
            {
                return Err(RuntimeError::Conflict(
                    "Binary snapshot scope changed".into(),
                ));
            }
            for (path, (resource, versions)) in grouped {
                if self.file_decisions(turn, "read", &[resource]).await.first()
                    != Some(&FileDecision::Allow)
                {
                    files.retain(|file| file.file != path);
                    continue;
                }
                if let Some(existing) = files.iter_mut().find(|file| file.file == path) {
                    existing.binary = Some(versions);
                } else {
                    files.push(FileDiff {
                        file: path,
                        status: "modified".into(),
                        additions: 0,
                        deletions: 0,
                        patch: String::new(),
                        binary: Some(versions),
                    });
                }
            }
            Ok(())
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn binary_snapshot_records(
        &self,
        turn: &TurnContext,
    ) -> Result<Vec<cyber_core::binary::editor::SavedReference>, RuntimeError> {
        let Ok(storage) = self.binary_documents().storage() else {
            return Ok(Vec::new());
        };
        let permit = self
            .binary_documents()
            .worker()
            .map_err(|e| RuntimeError::Conflict(e.to_string()))?;
        let (session, directory) = (turn.session_id.clone(), turn.directory.clone());
        let cancel = self.inner.closed.child_token();
        let _cancel = cancel.clone().drop_guard();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            storage.session_saved_references(&session, std::path::Path::new(&directory), || {
                cancel.is_cancelled()
            })
        })
        .await
        .map_err(|_| RuntimeError::Conflict("Binary snapshot worker failed".into()))?
        .map_err(|e| {
            RuntimeError::Conflict(format!(
                "Binary snapshot references are unavailable: {e}; use paged source history"
            ))
        })
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn binary_snapshot_reference(
        &self,
        turn: &TurnContext,
        record: cyber_core::binary::editor::SavedReference,
    ) -> Result<Option<cyber_core::binary::receipts::Version>, RuntimeError> {
        let Ok(storage) = self.binary_documents().storage() else {
            return Ok(None);
        };
        let permit = self
            .binary_documents()
            .worker()
            .map_err(|e| RuntimeError::Conflict(e.to_string()))?;
        let directory = turn.directory.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let root = cyber_core::config::project_root(std::path::Path::new(&directory))
                .canonicalize()
                .ok()?;
            if root != record.root {
                return None;
            }
            let source = cyber_core::binary::Document::open(&root, &record.version.path).ok()?;
            if source.path() != record.resource.path() {
                return None;
            }
            let version = storage
                .version_info(
                    &source,
                    &record.version.document_id,
                    &record.version.version_id,
                )
                .ok()?;
            (version == record.version).then_some(version)
        })
        .await
        .map_err(|_| RuntimeError::Conflict("Binary snapshot worker failed".into()))
    }
}
