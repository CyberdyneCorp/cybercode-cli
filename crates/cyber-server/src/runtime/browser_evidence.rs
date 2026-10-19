//! Browser verification evidence (`browser-verification` → Verification artifacts and
//! lifecycle): durable Session records of what a browser observed, or of a dev server
//! that failed before its readiness check, linked to the workspace content hash. A report
//! marks evidence stale once the workspace hash differs. Artifacts stay in managed local
//! storage (`<data>/browser/artifacts/<session_id>/`): deleting the Session or retention
//! removes them, and nothing uploads them.

use serde::{Deserialize, Serialize};

use super::events::event;
use super::{Runtime, RuntimeError};

/// One [`BrowserEvidence`].
pub const BROWSER_EVIDENCE: &str = "session.browser.evidence.1";

/// What an evidence record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    /// The browser observed the page; see `observed` and `not_run`.
    Observed,
    /// The dev server exited, or its readiness check timed out, before the page could be
    /// tested: nothing was observed.
    StartupFailed,
}

/// The browser viewport, when the server reported or was asked for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// A stored screenshot, console or network capture, or a dev server log tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserArtifact {
    /// `screenshot`, `console`, `network`, `snapshot` or `log`.
    pub kind: String,
    pub sha256: String,
    pub size: u64,
    pub media_type: String,
    /// The managed local copy.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserEvidence {
    pub id: String,
    pub session_id: String,
    /// The tool call that recorded it.
    pub call_id: String,
    pub status: EvidenceStatus,
    /// The workspace content hash when it was recorded (`sha256:…`), or `unavailable`.
    pub workspace_hash: String,
    /// The page URL (for a startup failure, the readiness URL).
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    pub time_ms: i64,
    /// What was done, oldest first (`navigate http://…`, `click Submit`, …).
    pub actions: Vec<String>,
    /// What the browser actually returned.
    pub observed: Vec<String>,
    /// Checks that did not run, so a report never presents them as passed.
    pub not_run: Vec<String>,
    pub artifacts: Vec<BrowserArtifact>,
}

/// One record in a report: stale once the workspace hash it was recorded under differs
/// from the current one (or either is unavailable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserEvidenceView {
    #[serde(flatten)]
    pub evidence: BrowserEvidence,
    pub stale: bool,
}

/// `GET /sessions/{id}/browser/evidence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserEvidenceReport {
    /// The workspace content hash now.
    pub workspace_hash: String,
    pub evidence: Vec<BrowserEvidenceView>,
}

impl BrowserEvidenceReport {
    pub fn new(workspace_hash: String, evidence: Vec<BrowserEvidence>) -> Self {
        let evidence = evidence
            .into_iter()
            .map(|evidence| BrowserEvidenceView {
                stale: evidence.workspace_hash != workspace_hash
                    || !workspace_hash.starts_with("sha256:"),
                evidence,
            })
            .collect();
        Self {
            workspace_hash,
            evidence,
        }
    }
}

/// The workspace hash of `directory`, or `unavailable` when it cannot be computed.
pub fn workspace_hash(directory: &std::path::Path) -> String {
    cyber_core::browser::workspace_hash(directory).unwrap_or_else(|error| {
        cyber_core::log::warn("browser", &error, serde_json::json!({}));
        "unavailable".into()
    })
}

impl Runtime {
    /// Record browser evidence for its Session.
    pub async fn record_browser_evidence(
        &self,
        evidence: BrowserEvidence,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(&evidence.session_id).await?;
        let mut state = handle.state.lock().await;
        self.inner
            .commit_locked(&mut state, vec![event(BROWSER_EVIDENCE, &evidence)])?;
        Ok(())
    }

    /// The Session's evidence, oldest first, with staleness against its directory now.
    pub async fn browser_evidence(
        &self,
        session_id: &str,
    ) -> Result<BrowserEvidenceReport, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let (directory, evidence) = {
            let state = handle.state.lock().await;
            (state.info.directory.clone(), state.browser_evidence.clone())
        };
        let hash = tokio::task::spawn_blocking(move || workspace_hash(directory.as_ref()))
            .await
            .unwrap_or_else(|_| "unavailable".into());
        Ok(BrowserEvidenceReport::new(hash, evidence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(hash: &str) -> BrowserEvidence {
        BrowserEvidence {
            id: "bev_1".into(),
            session_id: "ses_1".into(),
            call_id: "call_1".into(),
            status: EvidenceStatus::Observed,
            workspace_hash: hash.into(),
            url: "http://127.0.0.1:3000/".into(),
            viewport: None,
            time_ms: 1,
            actions: Vec::new(),
            observed: Vec::new(),
            not_run: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn evidence_is_stale_when_the_hash_differs_or_is_unavailable() {
        let report = BrowserEvidenceReport::new(
            "sha256:a".into(),
            vec![evidence("sha256:a"), evidence("sha256:b")],
        );
        assert!(!report.evidence[0].stale);
        assert!(report.evidence[1].stale);
        let unknown =
            BrowserEvidenceReport::new("unavailable".into(), vec![evidence("unavailable")]);
        assert!(unknown.evidence[0].stale);
    }
}
