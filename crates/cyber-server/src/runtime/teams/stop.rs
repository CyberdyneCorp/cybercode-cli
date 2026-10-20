//! Stop, resume and lead deletion (`agent-teams` → Shutdown and resume; design section
//! 12).
//!
//! A stop moves the team to `stopping` (`team.stopped.1` with `stopping`, reason `user`;
//! a budget stop gets there through `team.budget.exceeded.1`, reason `budget`). That is
//! the cooperative stop: each teammate's current Turn finishes, and the Turn-start gate
//! starts no other, nor one of a teammate's descendants, so queued input stays queued.
//! Once no Drain of a teammate or of its descendants runs the team is `stopped` with the
//! same reason; worktrees are kept.
//!
//! A resume (`team.resumed.1`) is allowed from `stopped` only, and after `lead_deleted`
//! only with a new lead in the team's Location. Members go back to `idle` (idle watches are their Drain ends,
//! armed again while the team is active); a teammate owning an `in_progress` task is
//! told to continue it, and one with queued input has its Drain restarted. Sessions,
//! tasks and message history are untouched, so teammates continue with their context.
//!
//! Deleting the lead records `team.stopped.1` with reason `lead_deleted` (stopped at
//! once: no further record can reach the deleted stream) before the Session goes.

use rusqlite::OptionalExtension;

use super::records::{self, MemberStatus, Resumed, Role, StopReason, Stopped, TaskStatus};
use super::{NOTICE_SOURCE, Team};
use crate::runtime::events::event;
use crate::runtime::messaging::watch;
use crate::runtime::workflow_recovery::location_key;
use crate::runtime::{Admission, Delivery, Runtime, RuntimeError};

/// The text a teammate owning an `in_progress` task is admitted on resume.
pub fn resume_line(task_id: &str, title: &str) -> String {
    format!("The team resumed. Continue {task_id}: {title}")
}

impl Runtime {
    /// Stop a team (reason `user`): `stopping` now, `stopped` once no teammate's Drain
    /// runs. Stopping a team that is already stopping or stopped changes nothing.
    pub async fn team_stop(&self, team_id: &str) -> Result<Team, RuntimeError> {
        {
            let _status = self.inner.team_status.lock().await;
            let team = self.known_team(team_id)?;
            if team.status == "active" {
                let stopping = Stopped {
                    team_id: team.id.clone(),
                    reason: StopReason::User,
                    stopping: true,
                };
                self.inner
                    .commit_to_lead(&team, vec![event(records::STOPPED, &stopping)])
                    .await?;
            }
        }
        self.finish_team_stop(team_id).await?;
        self.known_team(team_id)
    }

    /// Resume a stopped team. After `lead_deleted`, `lead` names the new lead: a
    /// top-level Session in the team's Location that is neither a teammate nor a member
    /// of another team.
    pub async fn team_resume(
        &self,
        team_id: &str,
        lead: Option<&str>,
    ) -> Result<Team, RuntimeError> {
        let team = self.known_team(team_id)?;
        self.inner
            .tools
            .team_settings(&team.directory)
            .map_err(RuntimeError::Invalid)?
            .ok_or(RuntimeError::FeatureDisabled(cyber_core::teams::DISABLED))?;
        {
            let _status = self.inner.team_status.lock().await;
            let team = self.known_team(team_id)?;
            if team.status != "stopped" {
                return Err(RuntimeError::Conflict(format!(
                    "Team {} is {}; only a stopped team resumes",
                    team.id, team.status
                )));
            }
            let lead_id = self.resume_lead(&team, lead).await?;
            let resumed = Resumed {
                team_id: team.id.clone(),
                lead_id: lead_id.clone(),
            };
            let handle = self.inner.handle(&lead_id).await?;
            self.inner
                .commit(&handle, vec![event(records::RESUMED, &resumed)])
                .await?;
        }
        self.restart_members(team_id).await?;
        self.known_team(team_id)
    }

    /// The lead a resume records: the current one, or after `lead_deleted` the new one
    /// `lead` names.
    async fn resume_lead(&self, team: &Team, lead: Option<&str>) -> Result<String, RuntimeError> {
        let deleted = self.stop_reason(&team.id)? == Some(StopReason::LeadDeleted);
        match (deleted, lead) {
            (false, None) => Ok(team.lead_id.clone()),
            (false, Some(id)) if id == team.lead_id => Ok(team.lead_id.clone()),
            (false, Some(_)) => Err(RuntimeError::Invalid(
                "A new lead can only be named after the lead was deleted".into(),
            )),
            (true, None) => Err(RuntimeError::Invalid(format!(
                "The lead of team {} was deleted; resume it with a new lead (--lead <session>)",
                team.id
            ))),
            (true, Some(id)) => self.new_lead(team, id).await,
        }
    }

    /// A new lead after `lead_deleted`: a top-level Session in the team's Location (its
    /// checkout receives merges, its configuration bounds the teammates, and the team is
    /// listed there) that is neither a teammate nor a member of another team.
    async fn new_lead(&self, team: &Team, id: &str) -> Result<String, RuntimeError> {
        let info = self.state(id).await?.info;
        if location_key(&info.directory) != location_key(&team.directory) {
            return Err(RuntimeError::Invalid(format!(
                "The new lead must work in the team's Location ({})",
                team.directory
            )));
        }
        if info.team.is_some() {
            return Err(RuntimeError::Invalid("Teammates cannot lead a team".into()));
        }
        if info.parent_id.is_some() {
            return Err(RuntimeError::Invalid(
                "Only a top-level Session can lead a team".into(),
            ));
        }
        match self.team_membership(id)? {
            Some((other, member)) if other.id != team.id || member.role != Role::Lead => {
                Err(RuntimeError::Conflict(format!(
                    "Session {id} is already a member of team {}",
                    other.id
                )))
            }
            _ => Ok(info.id),
        }
    }

    /// After a resume: a teammate owning an `in_progress` task with nothing queued is
    /// told to continue it; one with queued input has its Drain restarted.
    async fn restart_members(&self, team_id: &str) -> Result<(), RuntimeError> {
        let board = self.board(team_id)?;
        for member in self.team_members(team_id)? {
            if member.role != Role::Teammate {
                continue;
            }
            let handle = match self.inner.handle(&member.session_id).await {
                Ok(handle) => handle,
                Err(RuntimeError::SessionNotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            let queued = watch::promotable(&*handle.state.lock().await);
            let task = board.iter().find(|task| {
                task.status == TaskStatus::InProgress
                    && task.owner.as_deref() == Some(member.name.as_str())
            });
            match task {
                Some(task) if !queued => {
                    let admission = Admission {
                        source: NOTICE_SOURCE.into(),
                        resume: true,
                        ..Admission::text(resume_line(&task.id, &task.title), Delivery::Queue)
                    };
                    self.admit_message(&member.session_id, admission, Vec::new())
                        .await?;
                }
                _ if queued => self.inner.start_drain(&member.session_id, false),
                _ => {}
            }
        }
        Ok(())
    }

    /// End a stop once no teammate's Drain runs: `team.stopped.1` with the stop's
    /// reason. Whether the team is now stopped by this call.
    pub(crate) async fn finish_team_stop(&self, team_id: &str) -> Result<bool, RuntimeError> {
        let _status = self.inner.team_status.lock().await;
        let team = self.known_team(team_id)?;
        if team.status != "stopping" {
            return Ok(false);
        }
        if self.team_working(team_id)? {
            return Ok(false);
        }
        let stopped = Stopped {
            team_id: team.id.clone(),
            reason: self.stop_reason(team_id)?.unwrap_or(StopReason::User),
            stopping: false,
        };
        self.inner
            .commit_to_lead(&team, vec![event(records::STOPPED, &stopped)])
            .await?;
        Ok(true)
    }

    /// Whether a Drain of a teammate that has not stopped, or of any of its descendants
    /// (a background subagent among them), still runs.
    fn team_working(&self, team_id: &str) -> Result<bool, RuntimeError> {
        for member in self.team_members(team_id)? {
            if member.role != Role::Teammate || member.status == MemberStatus::Stopped {
                continue;
            }
            let tree = self.inner.descendants(&member.session_id)?;
            if tree.iter().any(|id| self.is_running(id)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A Drain of a teammate or of one of its descendants ended: a stopping team whose
    /// last running Drain this was is now stopped.
    pub(crate) async fn teammate_drain_stopped(
        &self,
        session_id: &str,
    ) -> Result<(), RuntimeError> {
        let info = match self.inner.handle(session_id).await {
            Ok(handle) => handle.state.lock().await.info.clone(),
            Err(RuntimeError::SessionNotFound(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        if info.team.is_none() && info.parent_id.is_none() {
            return Ok(());
        }
        let Some(tag) = self.binding_teammate(&info).await? else {
            return Ok(());
        };
        match self.team(&tag.team_id)? {
            Some(team) if team.status == "stopping" => {
                self.finish_team_stop(&team.id).await.map(drop)
            }
            _ => Ok(()),
        }
    }

    /// Before the lead `session_id` is deleted: stop its team with reason
    /// `lead_deleted`. Any other Session changes nothing.
    pub(crate) async fn stop_for_lead_deletion(
        &self,
        session_id: &str,
    ) -> Result<(), RuntimeError> {
        let _status = self.inner.team_status.lock().await;
        let Some((team, member)) = self.team_membership(session_id)? else {
            return Ok(());
        };
        if member.role != Role::Lead
            || (team.status == "stopped"
                && self.stop_reason(&team.id)? == Some(StopReason::LeadDeleted))
        {
            return Ok(());
        }
        let stopped = Stopped {
            team_id: team.id.clone(),
            reason: StopReason::LeadDeleted,
            stopping: false,
        };
        self.inner
            .commit_to_lead(&team, vec![event(records::STOPPED, &stopped)])
            .await
    }

    fn known_team(&self, team_id: &str) -> Result<Team, RuntimeError> {
        self.team(team_id)?
            .ok_or_else(|| RuntimeError::TeamNotFound(team_id.to_owned()))
    }

    /// The recorded stop reason of a team that is stopping or stopped.
    pub(crate) fn stop_reason(&self, team_id: &str) -> Result<Option<StopReason>, RuntimeError> {
        let team_id = team_id.to_owned();
        let reason: Option<String> = self.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT stop_reason FROM team WHERE id = ?1",
                    [team_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })?;
        reason
            .map(|text| serde_json::from_value(serde_json::Value::String(text)))
            .transpose()
            .map_err(|error| RuntimeError::Corrupt(format!("team stop reason: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resumed_teammate_is_told_which_task_to_continue() {
        assert_eq!(
            resume_line("task_01J", "Wire the route"),
            "The team resumed. Continue task_01J: Wire the route"
        );
    }
}
