//! Pull requests a Session is linked to (`vcs-integration` → PR checkout and linked
//! sessions): recorded when the Session creates or pushes to a PR branch, and listed by
//! `GET /sessions?pr=<number>` for `cyber pr <number>` and `cyber --from-pr`.

use serde::{Deserialize, Serialize};

use super::events::event;
use super::{Runtime, RuntimeError};

pub const PR_LINKED: &str = "session.pr.linked.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PrLink {
    pub number: u64,
    /// The PR's web URL as `gh` reports it.
    pub url: String,
}

impl Runtime {
    /// Link `session_id` to a pull request; linking the same PR again records nothing.
    pub async fn link_pr(&self, session_id: &str, link: PrLink) -> Result<(), RuntimeError> {
        if link.number == 0 || !link.url.starts_with("https://") {
            return Err(RuntimeError::Invalid(
                "a PR link needs a positive number and an https URL".into(),
            ));
        }
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        if state.pull_requests.contains(&link.number) {
            return Ok(());
        }
        self.inner
            .commit_locked(&mut state, vec![event(PR_LINKED, &link)])?;
        Ok(())
    }
}
