//! What clients show of a team (`agent-teams` → Team view; design sections 14 and 15):
//! each member with its status, current task, cost and last message, the task board,
//! and the team's budget and spend. `GET /teams`, `GET /teams/:id`, `cyber teams` and
//! the TUI team view all read it.

use cyber_core::budget::Budget;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::records::{MemberSpend, TaskStatus};
use super::{Member, Task, Team};
use crate::runtime::{Runtime, RuntimeError};

/// The longest last-message text a view carries, in characters.
const MESSAGE_PREVIEW: usize = 200;

/// A team with its members, board, budget and spend.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TeamView {
    pub team: Team,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    pub spent: TeamSpent,
    /// The lead first, then teammates in spawn order.
    pub members: Vec<MemberView>,
    /// Every task, in creation order.
    pub tasks: Vec<Task>,
}

/// A team's spend since it was created.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct TeamSpent {
    pub turns: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub wall_seconds: f64,
}

/// One member as the team view shows it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct MemberView {
    #[serde(flatten)]
    pub member: Member,
    /// The first `in_progress` task the member owns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<CurrentTask>,
    pub turns: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    /// The member's newest cross-session message with text, either direction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<LastMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct CurrentTask {
    pub id: String,
    pub title: String,
}

/// A member's newest message; `text` is cut to 200 characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct LastMessage {
    /// `in` (received) or `out` (sent).
    pub direction: String,
    /// The other side's address.
    pub peer: String,
    pub outcome: String,
    pub text: String,
    pub created_ms: i64,
}

/// A team as `GET /teams` lists it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TeamListing {
    #[serde(flatten)]
    pub team: Team,
    pub teammates: usize,
    pub tasks: usize,
    pub tasks_done: usize,
    pub cost_usd: f64,
}

impl TeamListing {
    fn of(view: TeamView) -> Self {
        Self {
            teammates: view.members.len().saturating_sub(1),
            tasks: view.tasks.len(),
            tasks_done: view
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Done)
                .count(),
            cost_usd: view.spent.cost_usd,
            team: view.team,
        }
    }
}

impl Runtime {
    /// The teams recorded for `directory` (their lead's Location), newest first,
    /// optionally only those with `status`.
    pub fn teams(
        &self,
        directory: &str,
        status: Option<&str>,
    ) -> Result<Vec<TeamListing>, RuntimeError> {
        let (directory, status) = (directory.to_owned(), status.map(str::to_owned));
        let ids: Vec<String> = self.inner.store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT id FROM team WHERE directory = ?1 AND (?2 IS NULL OR status = ?2)
                 ORDER BY created_ms DESC, id DESC",
            )?;
            let ids = statement
                .query_map(params![directory, status], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })?;
        ids.iter()
            .map(|id| self.team_view(id).map(TeamListing::of))
            .collect()
    }

    /// A team with its members, board, budget and spend.
    pub fn team_view(&self, team_id: &str) -> Result<TeamView, RuntimeError> {
        let team = self
            .team(team_id)?
            .ok_or_else(|| RuntimeError::TeamNotFound(team_id.to_owned()))?;
        let tasks = self.board(team_id)?;
        let spend = self.team_spend(team_id)?.unwrap_or_default();
        let members = self
            .team_members(team_id)?
            .into_iter()
            .map(|member| self.member_view(member, &tasks, &spend.members))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TeamView {
            budget: self.team_budget(team_id)?,
            spent: TeamSpent {
                turns: spend.total.turns,
                tokens: spend.total.tokens,
                cost_usd: spend.total.cost,
                wall_seconds: spend.total.elapsed_seconds,
            },
            team,
            members,
            tasks,
        })
    }

    fn member_view(
        &self,
        member: Member,
        tasks: &[Task],
        spend: &[MemberSpend],
    ) -> Result<MemberView, RuntimeError> {
        let current_task = tasks
            .iter()
            .find(|task| {
                task.status == TaskStatus::InProgress
                    && task.owner.as_deref() == Some(member.name.as_str())
            })
            .map(|task| CurrentTask {
                id: task.id.clone(),
                title: task.title.clone(),
            });
        let spent = spend.iter().find(|s| s.name == member.name);
        Ok(MemberView {
            current_task,
            turns: spent.map_or(0, |s| s.turns),
            tokens: spent.map_or(0, |s| s.tokens),
            cost_usd: spent.map_or(0.0, |s| s.cost_usd),
            last_message: self.last_message(&member.session_id)?,
            member,
        })
    }

    fn last_message(&self, session_id: &str) -> Result<Option<LastMessage>, RuntimeError> {
        let session_id = session_id.to_owned();
        Ok(self.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT direction, peer_address, outcome, text, created_ms
                     FROM xsession_message WHERE session_id = ?1 AND text IS NOT NULL
                     ORDER BY created_ms DESC, seq DESC LIMIT 1",
                    params![session_id],
                    |row| {
                        let text: String = row.get(3)?;
                        Ok(LastMessage {
                            direction: row.get(0)?,
                            peer: row.get(1)?,
                            outcome: row.get(2)?,
                            text: text.chars().take(MESSAGE_PREVIEW).collect(),
                            created_ms: row.get(4)?,
                        })
                    },
                )
                .optional()?)
        })?)
    }

    /// A team's recorded budget; one that no longer reads is reported as corrupt.
    fn team_budget(&self, team_id: &str) -> Result<Option<Budget>, RuntimeError> {
        let id = team_id.to_owned();
        let text: Option<String> = self.inner.store.read(move |db| {
            Ok(db
                .query_row("SELECT budget FROM team WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?
                .flatten())
        })?;
        text.map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|error| RuntimeError::Corrupt(format!("team {team_id} budget: {error}")))
    }
}
