//! What one Session's browser did since its last evidence record, the managed artifact
//! store, and the evidence records themselves (`browser-verification` → Verification
//! artifacts and lifecycle).

use std::path::{Path, PathBuf};

use cyber_server::runtime::{BrowserArtifact, BrowserEvidence, EvidenceStatus, Runtime, Viewport};
use sha2::{Digest as _, Sha256};

/// The checks a report lists as not run until the visit performed them.
const CHECKS: &[(Check, &str)] = &[
    (Check::Screenshot, "screenshot not captured"),
    (Check::Console, "console errors not inspected"),
    (Check::Network, "failed network requests not inspected"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    Screenshot,
    Console,
    Network,
}

/// The current page and what happened since the last record.
#[derive(Debug, Default)]
pub(crate) struct Visit {
    pub url: String,
    pub viewport: Option<Viewport>,
    pub actions: Vec<String>,
    pub observed: Vec<String>,
    pub artifacts: Vec<BrowserArtifact>,
    /// Checks done on the current page.
    pub done: Vec<Check>,
}

impl Visit {
    /// A navigation starts a new page: its checks start over.
    pub(crate) fn navigated(&mut self, url: &str) {
        self.url = url.to_string();
        self.done.clear();
    }

    pub(crate) fn checked(&mut self, check: Check) {
        if !self.done.contains(&check) {
            self.done.push(check);
        }
    }

    /// The record of what happened since the last one, which it then forgets.
    pub(crate) fn take(&mut self) -> Taken {
        Taken {
            url: self.url.clone(),
            viewport: self.viewport,
            actions: std::mem::take(&mut self.actions),
            observed: std::mem::take(&mut self.observed),
            artifacts: std::mem::take(&mut self.artifacts),
            not_run: CHECKS
                .iter()
                .filter(|(check, _)| !self.done.contains(check))
                .map(|(_, text)| (*text).to_string())
                .collect(),
        }
    }
}

pub(crate) struct Taken {
    pub url: String,
    pub viewport: Option<Viewport>,
    pub actions: Vec<String>,
    pub observed: Vec<String>,
    pub artifacts: Vec<BrowserArtifact>,
    pub not_run: Vec<String>,
}

/// `<data>/browser/artifacts/<session_id>/<sha256>.<extension>`: content addressed, so
/// a repeated capture is stored once.
pub(crate) fn store_artifact(
    root: &Path,
    session_id: &str,
    kind: &str,
    media_type: &str,
    bytes: &[u8],
) -> Result<BrowserArtifact, String> {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let dir = root.join("artifacts").join(session_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("browser artifact: {e}"))?;
    let path: PathBuf = dir.join(format!("{sha256}.{}", extension(media_type)));
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|e| format!("browser artifact: {e}"))?;
    }
    Ok(BrowserArtifact {
        kind: kind.into(),
        sha256,
        size: bytes.len() as u64,
        media_type: media_type.into(),
        path: path.display().to_string(),
    })
}

fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "txt",
    }
}

/// Record the evidence `taken` describes, under the workspace hash of `directory` now.
pub(crate) async fn record(
    runtime: &Runtime,
    session_id: &str,
    call_id: &str,
    directory: &Path,
    status: EvidenceStatus,
    taken: Taken,
) -> Result<BrowserEvidence, String> {
    let directory = directory.to_path_buf();
    let workspace_hash = tokio::task::spawn_blocking(move || {
        cyber_server::runtime::browser_evidence::workspace_hash(&directory)
    })
    .await
    .unwrap_or_else(|_| "unavailable".into());
    let evidence = BrowserEvidence {
        id: cyber_core::ids::new_id("bev"),
        session_id: session_id.into(),
        call_id: call_id.into(),
        status,
        workspace_hash,
        url: taken.url,
        viewport: taken.viewport,
        time_ms: chrono_now_ms(),
        actions: taken.actions,
        observed: taken.observed,
        not_run: taken.not_run,
        artifacts: taken.artifacts,
    };
    runtime
        .record_browser_evidence(evidence.clone())
        .await
        .map_err(|error| error.to_string())?;
    Ok(evidence)
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_not_done_on_the_current_page_are_listed_as_not_run() {
        let mut visit = Visit::default();
        visit.navigated("http://127.0.0.1:1/");
        visit.checked(Check::Screenshot);
        let taken = visit.take();
        assert_eq!(
            taken.not_run,
            vec![
                "console errors not inspected",
                "failed network requests not inspected"
            ]
        );
        visit.checked(Check::Console);
        visit.checked(Check::Network);
        assert!(visit.take().not_run.is_empty());
        visit.navigated("http://127.0.0.1:1/other");
        assert_eq!(visit.take().not_run.len(), 3);
    }

    #[test]
    fn artifacts_are_content_addressed() {
        let root = tempfile::tempdir().unwrap();
        let first =
            store_artifact(root.path(), "ses_1", "screenshot", "image/png", b"png").unwrap();
        let again =
            store_artifact(root.path(), "ses_1", "screenshot", "image/png", b"png").unwrap();
        assert_eq!(first, again);
        assert!(first.path.ends_with(&format!("{}.png", first.sha256)));
        assert_eq!(std::fs::read(&first.path).unwrap(), b"png");
    }
}
