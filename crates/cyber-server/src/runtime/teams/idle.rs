//! Idle notifications and member status (`agent-teams` → Idle notifications; design
//! sections 4 and 7).
//!
//! Every teammate of an active team carries a system watch: when its Drain ends with
//! nothing promotable, the lead's stream records `team.member.updated.1` (`idle`, with
//! the last status line) and `team.member.idle.1`, and the lead is admitted the notice
//! `msgx_team_idle_<team>_<member>_<seq>` (`source: "team"`) in the same commit, then
//! `TeammateIdle` hooks run with their decisions only logged. While the team is stopping
//! or stopped no notice is sent; the status alone is recorded. A Drain cut off by a
//! restart leaves its member `working`, which startup recovery treats as an ended Drain.
//!
//! The other statuses follow the Session: `working` when a Drain starts, `waiting_input`
//! while a permission or question request of it (or routed to it) is open, `stopped`
//! once the team stops, which only a resume clears.

use serde_json::json;

use super::records::{self, MemberIdle, MemberStatus, MemberUpdated, Role, Task, TaskStatus};
use super::{Member, Team};
use crate::runtime::events::event;
use crate::runtime::messaging::{render, watch};
use crate::runtime::{Admission, Delivery, Inner, Runtime, RuntimeError};

/// The `source` of the notices admitted to a lead.
pub const NOTICE_SOURCE: &str = "team";
/// The hook event that runs after a recorded idle notice.
pub const IDLE_HOOK: &str = "TeammateIdle";

/// The inbox message ID of a teammate's idle notice: one per position of its stream, so
/// a recovered notice is never admitted twice.
pub fn notice_id(team_id: &str, member: &str, seq: i64) -> String {
    format!("msgx_team_idle_{team_id}_{member}_{seq}")
}

/// The notice text (design section 7): with an `in_progress` task owned, `<name> is idle
/// with task_<id> in progress: <status>`; else `<name> is idle: all assigned tasks done`
/// when it owns `done` tasks and nothing open, else `<name> is idle: <status>`.
pub fn notice_line(name: &str, board: &[Task], last_status: &str) -> String {
    let owned: Vec<&Task> = board
        .iter()
        .filter(|task| task.owner.as_deref() == Some(name))
        .collect();
    if let Some(task) = owned.iter().find(|t| t.status == TaskStatus::InProgress) {
        return format!("{name} is idle with {} in progress: {last_status}", task.id);
    }
    let open = owned
        .iter()
        .any(|t| matches!(t.status, TaskStatus::Todo | TaskStatus::Blocked));
    let done = owned.iter().any(|t| t.status == TaskStatus::Done);
    if done && !open {
        format!("{name} is idle: all assigned tasks done")
    } else {
        format!("{name} is idle: {last_status}")
    }
}

/// A teammate Session found idle: no Drain, nothing promotable, not archived.
struct IdleTeammate {
    last_status: String,
    seq: i64,
}

fn warn(what: &str, session_id: &str, error: &RuntimeError) {
    cyber_core::log::warn(
        "teams",
        what,
        json!({ "session_id": session_id, "error": error.to_string() }),
    );
}

impl Runtime {
    /// A teammate's Drain ended (`drain::run`): record it idle and notify the lead. A
    /// Drain stopped by shutdown records nothing; startup recovery does.
    pub(crate) async fn team_drain_ended(&self, session_id: &str) {
        if self.is_shutting_down() {
            return;
        }
        if let Err(error) = self.teammate_went_idle(session_id).await {
            warn("teammate idle failed", session_id, &error);
        }
        // `agent-teams` → Shutdown and resume: the last teammate to stop ends the stop.
        if let Err(error) = self.teammate_drain_stopped(session_id).await {
            warn("team stop failed", session_id, &error);
        }
    }

    /// Startup recovery: teammates of active or stopping teams still recorded `working`
    /// or `waiting_input` lost their Drain to a restart; each idle one is recorded idle
    /// and its lead notified now. Must run after `recover_watches`. Returns how many
    /// members it checked.
    pub async fn recover_team_idle(&self) -> Result<usize, RuntimeError> {
        let sessions = self.inner.store.read(|db| {
            let mut statement = db.prepare(
                "SELECT m.session_id FROM team_member m JOIN team t ON t.id = m.team_id
                 WHERE m.role = 'teammate' AND m.status IN ('working', 'waiting_input')
                   AND t.status IN ('active', 'stopping')
                 ORDER BY m.rowid",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })?;
        for session_id in &sessions {
            if let Err(error) = self.teammate_went_idle(session_id).await {
                warn("teammate idle recovery failed", session_id, &error);
            }
        }
        Ok(sessions.len())
    }

    /// The teammate `session_id` as its idle watch sees it; `None` for any other Session
    /// or a teammate that is not idle.
    async fn idle_teammate(&self, session_id: &str) -> Result<Option<IdleTeammate>, RuntimeError> {
        let handle = match self.inner.handle(session_id).await {
            Ok(handle) => handle,
            Err(RuntimeError::SessionNotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        if self.is_running(session_id) {
            return Ok(None);
        }
        let state = handle.state.lock().await;
        let teammate = state
            .info
            .team
            .as_ref()
            .is_some_and(|tag| tag.role == Role::Teammate);
        if !teammate || state.info.archived || watch::promotable(&state) {
            return Ok(None);
        }
        let (last_status, _) = watch::last_run(&state);
        Ok(Some(IdleTeammate {
            last_status,
            seq: state.last_seq,
        }))
    }

    /// Record an idle teammate and, while its team is active, admit the notice to the
    /// lead with `team.member.idle.1` in one commit, then run `TeammateIdle`.
    async fn teammate_went_idle(&self, session_id: &str) -> Result<(), RuntimeError> {
        let Some(idle) = self.idle_teammate(session_id).await? else {
            return Ok(());
        };
        // Read the member and record it under the team status lock: a stop that ends
        // between the read and the commit would otherwise be overwritten by `idle`
        // (only a resume clears `stopped`), and a stopped team's lead would be notified.
        let recorded = {
            let _status = self.inner.team_status.lock().await;
            let Some((team, member)) = self.recorded_member(session_id)? else {
                return Ok(());
            };
            let notified = self.record_idle(&team, &member, &idle).await?;
            (team, member, notified)
        };
        let (team, member, Some(tasks_in_progress)) = recorded else {
            return Ok(());
        };
        // The envelope's `session_id` is the teammate's: the hook runs on its Session.
        let fields = json!({
            "team_id": team.id,
            "member": member.name,
            "tasks_in_progress": tasks_in_progress,
            "last_status": idle.last_status,
        });
        self.inner
            .teammate_idle_hook(session_id, &member.name, fields)
            .await;
        Ok(())
    }

    /// Record `member` idle; while `team` is active, admit the idle notice to its lead in
    /// the same commit. The member's tasks in progress when a notice was sent.
    async fn record_idle(
        &self,
        team: &Team,
        member: &Member,
        idle: &IdleTeammate,
    ) -> Result<Option<Vec<String>>, RuntimeError> {
        let updated = event(
            records::MEMBER_UPDATED,
            &MemberUpdated {
                team_id: team.id.clone(),
                name: member.name.clone(),
                status: MemberStatus::Idle,
                last_status: Some(idle.last_status.clone()),
            },
        );
        if team.status != "active" {
            self.inner.commit_to_lead(team, vec![updated]).await?;
            return Ok(None);
        }
        let board = self.board(&team.id)?;
        let tasks_in_progress: Vec<String> = board
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .filter(|t| t.owner.as_deref() == Some(member.name.as_str()))
            .map(|t| t.id.clone())
            .collect();
        let line = notice_line(&member.name, &board, &idle.last_status);
        let recorded = MemberIdle {
            team_id: team.id.clone(),
            name: member.name.clone(),
            tasks_in_progress: tasks_in_progress.clone(),
            last_status: idle.last_status.clone(),
        };
        let admission = Admission {
            message_id: Some(notice_id(&team.id, &member.name, idle.seq)),
            source: NOTICE_SOURCE.into(),
            resume: true,
            ..Admission::text(
                render::team_idle_notice(&team.id, &member.name, &line),
                Delivery::Steer,
            )
        };
        self.admit_message(
            &team.lead_id,
            admission,
            vec![updated, event(records::MEMBER_IDLE, &recorded)],
        )
        .await?;
        Ok(Some(tasks_in_progress))
    }

    /// The team and member row of a teammate whose status may change: not `stopped`
    /// (only a resume clears it), in a team that exists and has `teams` on in its
    /// directory (unreadable settings record nothing).
    fn recorded_member(&self, session_id: &str) -> Result<Option<(Team, Member)>, RuntimeError> {
        let Some((team, member)) = self.team_membership(session_id)? else {
            return Ok(None);
        };
        let enabled = matches!(self.inner.tools.team_settings(&team.directory), Ok(Some(_)));
        if member.role != Role::Teammate || member.status == MemberStatus::Stopped || !enabled {
            return Ok(None);
        }
        Ok(Some((team, member)))
    }

    /// Record a teammate's live status when it changed: `waiting_input` while a request
    /// of it is open, else `working` while its Drain runs. Idle is recorded by the
    /// Drain's end, with its notice.
    async fn refresh_member_status(&self, session_id: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let teammate = handle
            .state
            .lock()
            .await
            .info
            .team
            .as_ref()
            .is_some_and(|tag| tag.role == Role::Teammate);
        if !teammate {
            return Ok(());
        }
        let status = if !self.inner.pending(Some(session_id)).is_empty() {
            MemberStatus::WaitingInput
        } else if self.is_running(session_id) {
            MemberStatus::Working
        } else {
            return Ok(());
        };
        // As for idle: no status recorded over a stop that ends meanwhile.
        let _status = self.inner.team_status.lock().await;
        let Some((team, member)) = self.recorded_member(session_id)? else {
            return Ok(());
        };
        if member.status == status {
            return Ok(());
        }
        let updated = MemberUpdated {
            team_id: team.id.clone(),
            name: member.name,
            status,
            last_status: None,
        };
        self.inner
            .commit_to_lead(&team, vec![event(records::MEMBER_UPDATED, &updated)])
            .await
    }
}

impl Inner {
    /// A teammate's Drain started, or one of its requests opened or was answered:
    /// record its status when it changed. Failures are logged; status never blocks work.
    pub(crate) async fn member_status_changed(&self, session_id: &str) {
        let Some(inner) = self.me.upgrade() else {
            return;
        };
        if let Err(error) = (Runtime { inner }).refresh_member_status(session_id).await {
            warn("teammate status failed", session_id, &error);
        }
    }

    /// Append team records to the lead's stream (the team's single writer).
    pub(super) async fn commit_to_lead(
        &self,
        team: &Team,
        events: Vec<cyber_store::NewEvent>,
    ) -> Result<(), RuntimeError> {
        let handle = self.handle(&team.lead_id).await?;
        self.commit(&handle, events).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, owner: &str) -> Task {
        Task {
            id: id.into(),
            seq: 0,
            title: id.into(),
            description: String::new(),
            status,
            owner: Some(owner.into()),
            depends_on: Vec::new(),
            created_by: "lead".into(),
            created_ms: 0,
            updated_ms: 0,
            version: 0,
        }
    }

    #[test]
    fn the_three_notice_texts() {
        let board = [
            task("task_1", TaskStatus::Done, "tests"),
            task("task_2", TaskStatus::Cancelled, "tests"),
            task("task_3", TaskStatus::InProgress, "api"),
            task("task_4", TaskStatus::Todo, "ui"),
        ];
        assert_eq!(
            notice_line("tests", &board, "ok"),
            "tests is idle: all assigned tasks done"
        );
        assert_eq!(
            notice_line("api", &board, "wired the route"),
            "api is idle with task_3 in progress: wired the route"
        );
        assert_eq!(
            notice_line("ui", &board, "waiting on api"),
            "ui is idle: waiting on api"
        );
        // Owning nothing is not "all done".
        assert_eq!(notice_line("docs", &board, "idle"), "docs is idle: idle");
        let id = notice_id("team_01J", "tests", 42);
        assert_eq!(id, "msgx_team_idle_team_01J_tests_42");
        assert!(crate::runtime::messaging::send::valid_id(&id));
    }
}
