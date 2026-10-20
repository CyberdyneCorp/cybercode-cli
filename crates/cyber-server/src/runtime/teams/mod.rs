//! Agent teams (`agent-teams`), behind the `teams` feature: a lead Session and named
//! teammate Sessions that share a task board. All team state is the `team.*` events of
//! the lead's stream ([`records`]), projected into `team`, `team_member` and `team_task`.
//!
//! Permissions in teams: a teammate's effective Mode is the stricter of its own and the
//! lead's, at each Turn start and again at each tool dispatch (with the lead's deny
//! rules, the way subagents inherit their ancestors'), and a teammate cannot switch to a
//! Mode above the lead's.
//!
//! Files: in a team with a `shared` teammate, a shared member cannot change a path
//! another member owns ([`ownership`]); the lead merges worktree teammates' branches with
//! `team_merge` ([`merge`]), which reports conflicts without touching its checkout.
//!
//! Budgets, stop and resume: a team budget is checked at each teammate's Turn start
//! ([`budget`]); a stop lets teammates finish their current Turn, and a resume continues
//! with every Session, task and message intact ([`stop`]); startup finishes what a
//! restart cut off ([`recovery`]).

mod budget;
mod idle;
pub mod merge;
mod ownership;
pub mod records;
mod recovery;
mod spawn;
mod stop;
mod tasks;
mod view;

pub use budget::exceeded_line;
pub use idle::{IDLE_HOOK, NOTICE_SOURCE, notice_id, notice_line};
pub use merge::{MergeOutcome, MergeTarget, TeamMerge};
pub use ownership::{OWNED_PREFIX, Ownership};
pub use records::{
    MemberSpend, MemberStatus, Role, StopReason, Task, TaskStatus, TaskUpdated, TeamTag,
};
pub use recovery::TeamRecovery;
pub use spawn::{SpawnedMember, TeamSpawn, TeamStart};
pub use stop::resume_line;
pub use tasks::{TASK_SUBJECT, TaskCreate, TaskFilter, TaskUpdate};
pub use view::{CurrentTask, LastMessage, MemberView, TeamListing, TeamSpent, TeamView};

use cyber_core::teams::Isolation;
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ancestry::AncestorAuthority;
use super::{Handle, Inner, Runtime, RuntimeError, SessionInfo};

/// The `system` message reason of the definition body admitted to the lead.
pub const INSTRUCTIONS_REASON: &str = "team_instructions";

/// A team, as its projection holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Team {
    pub id: String,
    /// The definition name, or `adhoc`.
    pub name: String,
    pub lead_id: String,
    pub directory: String,
    /// `active`, `stopping` or `stopped`.
    pub status: String,
    /// Why a stopping or stopped team stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

/// A member of a team (the lead is the member named `lead`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Member {
    pub team_id: String,
    pub name: String,
    pub session_id: String,
    pub role: Role,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub isolation: Isolation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub owns: Vec<String>,
    pub status: MemberStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}

const TEAM_COLUMNS: &str =
    "id, name, lead_id, directory, status, stop_reason, created_ms, updated_ms";
const MEMBER_COLUMNS: &str = "team_id, name, session_id, role, agent, model, isolation, \
     worktree_id, branch, owns, status, last_status";

/// A stored enum name as its type.
fn named<T: DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_value(serde_json::Value::String(text)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error.into())
    })
}

fn team_row(row: &Row<'_>) -> rusqlite::Result<Team> {
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        lead_id: row.get(2)?,
        directory: row.get(3)?,
        status: row.get(4)?,
        stop_reason: row
            .get::<_, Option<String>>(5)?
            .map(|_| named(row, 5))
            .transpose()?,
        created_ms: row.get(6)?,
        updated_ms: row.get(7)?,
    })
}

fn member_row(row: &Row<'_>) -> rusqlite::Result<Member> {
    let owns: Option<String> = row.get(9)?;
    Ok(Member {
        team_id: row.get(0)?,
        name: row.get(1)?,
        session_id: row.get(2)?,
        role: named(row, 3)?,
        agent: row.get(4)?,
        model: row.get(5)?,
        isolation: named(row, 6)?,
        worktree_id: row.get(7)?,
        branch: row.get(8)?,
        owns: owns
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
        status: named(row, 10)?,
        last_status: row.get(11)?,
    })
}

impl Runtime {
    /// Rebuild the team tables from the `team.*` events; the number of events
    /// re-projected.
    pub fn rebuild_teams(&self) -> Result<usize, RuntimeError> {
        Ok(self.inner.store.reproject(
            &records::TABLES,
            records::PREFIX,
            std::sync::Arc::new(records::project),
        )?)
    }

    /// A team by ID.
    pub fn team(&self, team_id: &str) -> Result<Option<Team>, RuntimeError> {
        self.inner.team(team_id)
    }

    /// The members of a team, the lead first, then teammates in spawn order.
    pub fn team_members(&self, team_id: &str) -> Result<Vec<Member>, RuntimeError> {
        let team_id = team_id.to_owned();
        Ok(self.inner.store.read(move |db| {
            let mut statement = db.prepare(&format!(
                "SELECT {MEMBER_COLUMNS} FROM team_member WHERE team_id = ?1
                 ORDER BY role = 'teammate', rowid"
            ))?;
            let members = statement
                .query_map(params![team_id], member_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(members)
        })?)
    }

    /// The team `session_id` is a member of (as lead or teammate), with its member row.
    pub fn team_membership(
        &self,
        session_id: &str,
    ) -> Result<Option<(Team, Member)>, RuntimeError> {
        let session_id = session_id.to_owned();
        let member = self.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    &format!("SELECT {MEMBER_COLUMNS} FROM team_member WHERE session_id = ?1"),
                    params![session_id],
                    member_row,
                )
                .optional()?)
        })?;
        let Some(member) = member else {
            return Ok(None);
        };
        let team = self.team(&member.team_id)?.ok_or_else(|| {
            RuntimeError::Corrupt(format!(
                "team {} of a member is not recorded",
                member.team_id
            ))
        })?;
        Ok(Some((team, member)))
    }

    /// The lead of a teammate's team as an ancestor authority: its Mode is the
    /// teammate's ceiling and its deny rules apply to the teammate. `None` for a Session
    /// that is not a teammate. A teammate whose team or lead cannot be read fails closed.
    pub async fn team_lead_authority(
        &self,
        info: &SessionInfo,
    ) -> Result<Option<AncestorAuthority>, RuntimeError> {
        self.inner.team_lead_authority(info).await
    }

    /// The lead a request of a teammate (or of one of its subagents) is also routed to,
    /// with the teammate's name its clients label it with (`agent-teams` → Labelled
    /// teammate requests). `None` outside a team, for the lead's own requests (its
    /// subagents reach it as their ancestor) and once the lead Session is deleted.
    /// `ancestors` run from the parent to the root.
    pub(crate) async fn request_team_lead(
        &self,
        info: &SessionInfo,
        ancestors: &[SessionInfo],
    ) -> Result<Option<(SessionInfo, String)>, RuntimeError> {
        let root = ancestors.last().unwrap_or(info);
        let Some(tag) = root.team.as_ref().filter(|tag| tag.role == Role::Teammate) else {
            return Ok(None);
        };
        let Some(team) = self.team(&tag.team_id)? else {
            return Ok(None);
        };
        let handle = match self.inner.handle(&team.lead_id).await {
            Ok(handle) => handle,
            Err(RuntimeError::SessionNotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let lead = handle.state.lock().await.info.clone();
        Ok(Some((lead, tag.name.clone())))
    }
}

impl Inner {
    fn team(&self, team_id: &str) -> Result<Option<Team>, RuntimeError> {
        let team_id = team_id.to_owned();
        Ok(self.store.read(move |db| {
            Ok(db
                .query_row(
                    &format!("SELECT {TEAM_COLUMNS} FROM team WHERE id = ?1"),
                    params![team_id],
                    team_row,
                )
                .optional()?)
        })?)
    }

    pub(crate) async fn team_lead_authority(
        &self,
        info: &SessionInfo,
    ) -> Result<Option<AncestorAuthority>, RuntimeError> {
        let Some(tag) = info.team.as_ref().filter(|tag| tag.role == Role::Teammate) else {
            return Ok(None);
        };
        let team = self.team(&tag.team_id)?.ok_or_else(|| {
            RuntimeError::Invalid(format!(
                "Team {} of Session {} is not recorded",
                tag.team_id, info.id
            ))
        })?;
        // A deleted lead leaves no authority to bound the teammate by: fail closed.
        let handle = self
            .handle(&team.lead_id)
            .await
            .map_err(|error| match error {
                RuntimeError::SessionNotFound(_) => RuntimeError::Invalid(format!(
                    "The lead of team {} was deleted; resume the team with a new lead",
                    team.id
                )),
                error => error,
            })?;
        let running = self
            .drains
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&team.lead_id);
        let state = handle.state.lock().await;
        Ok(Some(AncestorAuthority {
            info: state.info.clone(),
            effective_mode: state.effective_mode(running).into(),
            effective_agent: state.effective_agent(running).into(),
        }))
    }

    /// The Mode ceiling of a teammate Session: its lead's effective Mode (`plan` for a
    /// lead running the read-only `reviewer` agent); `None` for any other Session. Reads
    /// the Session and then the lead one at a time, never holding both locks.
    pub(crate) async fn team_ceiling(
        &self,
        handle: &Handle,
    ) -> Result<Option<String>, RuntimeError> {
        let info = handle.state.lock().await.info.clone();
        Ok(self.team_lead_authority(&info).await?.map(|lead| {
            if lead.effective_agent == "reviewer" {
                "plan".into()
            } else {
                lead.effective_mode
            }
        }))
    }
}

/// `mode` bounded by a team ceiling.
pub(crate) fn bounded_mode(mode: &str, ceiling: Option<&str>) -> String {
    match ceiling {
        Some(ceiling) => super::workflows::stricter_mode(mode, ceiling),
        None => mode.to_owned(),
    }
}

/// `Mode limited by the team lead (<mode>)` when `mode` is above the ceiling.
pub(crate) fn check_mode(mode: &str, ceiling: Option<&str>) -> Result<(), RuntimeError> {
    match ceiling {
        Some(ceiling) if bounded_mode(mode, Some(ceiling)) != mode => Err(RuntimeError::Invalid(
            format!("Mode limited by the team lead ({ceiling})"),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_teammate_mode_is_bounded_by_the_lead() {
        assert_eq!(bounded_mode("default", Some("plan")), "plan");
        assert_eq!(bounded_mode("plan", Some("bypass")), "plan");
        assert_eq!(bounded_mode("auto", None), "auto");
        // An unknown ceiling fails closed to `plan`.
        assert_eq!(bounded_mode("default", Some("telepathy")), "plan");
        assert!(check_mode("plan", Some("default")).is_ok());
        assert!(check_mode("default", Some("default")).is_ok());
        assert_eq!(
            check_mode("bypass", Some("default"))
                .unwrap_err()
                .to_string(),
            "InvalidRequestError: Mode limited by the team lead (default)"
        );
        assert!(check_mode("bypass", None).is_ok());
    }
}
