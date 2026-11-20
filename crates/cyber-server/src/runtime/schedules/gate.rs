//! The unattended gate (`loops-scheduling` → Unattended Mode for schedules): checked when
//! a schedule is added and again before every run, because configuration, trust and
//! checkouts change between runs. Every failure refuses; nothing is assumed.

use std::path::Path;

use super::SchedulePolicy;
use crate::runtime::{Runtime, RuntimeError};

/// What a schedule would run: where, as which agent, in which Mode, in which Session.
pub(super) struct Draft<'a> {
    pub directory: &'a str,
    pub agent: &'a str,
    pub mode: &'a str,
    pub session_id: Option<&'a str>,
}

impl Runtime {
    /// Admit `draft` or say why not; the directory's policy when admitted.
    pub(super) async fn schedule_gate(&self, draft: &Draft<'_>) -> Result<SchedulePolicy, String> {
        let path = Path::new(draft.directory);
        if !path.is_absolute() || !path.is_dir() {
            return Err(format!(
                "Refused: {} is not an existing directory",
                draft.directory
            ));
        }
        let policy = self.inner.tools.schedule_policy(draft.directory)?;
        if !policy.trusted {
            return Err(format!(
                "Refused: {} is not trusted; run `cyber trust inspect` there",
                draft.directory
            ));
        }
        if draft.mode == "bypass" && !policy.allow_bypass {
            return Err(
                "Refused: a schedule runs in bypass only when loops.allow_bypass is true in \
                 user or managed config"
                    .into(),
            );
        }
        self.admit_mode(draft.directory, draft.agent, draft.mode)
            .map_err(|error| format!("Refused: {}", plain(&error)))?;
        if let Some(session_id) = draft.session_id {
            self.schedule_session_usable(session_id, draft.directory)
                .await?;
        }
        Ok(policy)
    }

    /// A Session a schedule resumes: a root Session of the schedule's directory that is
    /// not archived.
    async fn schedule_session_usable(
        &self,
        session_id: &str,
        directory: &str,
    ) -> Result<(), String> {
        let state = self
            .state(session_id)
            .await
            .map_err(|_| format!("Refused: Session {session_id} does not exist"))?;
        let info = &state.info;
        if info.parent_id.is_some() || info.workflow_run_id.is_some() {
            return Err(format!(
                "Refused: Session {session_id} is not a root Session"
            ));
        }
        if info.archived {
            return Err(format!("Refused: Session {session_id} is archived"));
        }
        if !same_directory(&info.directory, directory) {
            return Err(format!(
                "Refused: Session {session_id} belongs to {}, not {directory}",
                info.directory
            ));
        }
        Ok(())
    }
}

fn same_directory(a: &str, b: &str) -> bool {
    let canonical = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
    canonical(a) == canonical(b)
}

/// A runtime error's message without its error-type prefix.
pub(super) fn plain(error: &RuntimeError) -> String {
    let text = error.to_string();
    match text.split_once(": ") {
        Some((tag, rest)) if tag.ends_with("Error") => rest.to_owned(),
        _ => text,
    }
}
