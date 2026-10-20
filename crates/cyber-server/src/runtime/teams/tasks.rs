//! The shared task board of a team (`agent-teams` → Shared task list, Task claims; design
//! section 5). Every write runs on the lead's stream, the team's single writer: under the
//! lead's Session lock, one store transaction reads the board, checks the change and
//! appends `team.task.updated.1`, which projects the row. Two concurrent claims therefore
//! serialize, and the second sees the first's owner (`Task already claimed by <name>`).
//!
//! Rules: a task moves to `in_progress` (or `done`) only once every dependency is `done`;
//! dependencies are other tasks of the same team and a cycle is refused when written;
//! `expected_version` refuses an edit of a task that changed since; only the lead
//! reassigns a task another member owns, assigns tasks to others, or cancels.

use std::collections::{HashMap, HashSet};

use cyber_store::Expected;
use rusqlite::{Row, Transaction, params};
use serde::Deserialize;

use super::records::{self, Role, Task, TaskStatus, TaskUpdated};
use super::{Member, Team, named};
use crate::runtime::events::event;
use crate::runtime::{Runtime, RuntimeError};

/// The hook matcher subject of team task events (Jobs use their kind).
pub const TASK_SUBJECT: &str = "team_task";

/// `team_task_create`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskCreate {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Tasks of the same team this one waits for.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// A member name.
    #[serde(default)]
    pub owner: Option<String>,
}

/// `team_task_update`; a claim is `status: in_progress` (the owner defaults to the caller).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskUpdate {
    pub id: String,
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    /// Refuse the change unless the task is still at this version.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `team_task_list`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskFilter {
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default)]
    pub owner: Option<String>,
}

const TASK_COLUMNS: &str = "id, seq, title, description, status, owner, depends_on, created_by, \
     created_ms, updated_ms, version";

fn task_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    let depends_on: String = row.get(6)?;
    Ok(Task {
        id: row.get(0)?,
        seq: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        status: named(row, 4)?,
        owner: row.get(5)?,
        depends_on: serde_json::from_str(&depends_on).unwrap_or_default(),
        created_by: row.get(7)?,
        created_ms: row.get(8)?,
        updated_ms: row.get(9)?,
        version: row.get(10)?,
    })
}

fn tasks_of(db: &rusqlite::Connection, team_id: &str) -> rusqlite::Result<Vec<Task>> {
    let mut statement = db.prepare(&format!(
        "SELECT {TASK_COLUMNS} FROM team_task WHERE team_id = ?1 ORDER BY seq"
    ))?;
    statement
        .query_map(params![team_id], task_row)?
        .collect::<rusqlite::Result<Vec<_>>>()
}

/// The member making a change.
#[derive(Debug, Clone)]
struct Caller {
    name: String,
    lead: bool,
}

/// The board as one transaction reads it.
struct Board {
    tasks: Vec<Task>,
    members: Vec<String>,
}

impl Board {
    fn load(tx: &Transaction<'_>, team_id: &str) -> Result<Self, RuntimeError> {
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM team WHERE id = ?1",
                params![team_id],
                |row| row.get(0),
            )
            .map_err(store)?;
        match status.as_deref() {
            Some("active") => {}
            Some(other) => {
                return Err(RuntimeError::Conflict(format!("Team {team_id} is {other}")));
            }
            None => return Err(RuntimeError::Invalid(format!("No team {team_id}"))),
        }
        let mut statement = tx
            .prepare("SELECT name FROM team_member WHERE team_id = ?1")
            .map_err(store)?;
        let members = statement
            .query_map(params![team_id], |row| row.get(0))
            .and_then(|rows| rows.collect::<rusqlite::Result<Vec<String>>>())
            .map_err(store)?;
        Ok(Self {
            tasks: tasks_of(tx, team_id).map_err(store)?,
            members,
        })
    }

    fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    fn check_owner(&self, owner: Option<&str>) -> Result<(), RuntimeError> {
        match owner {
            Some(name) if !self.members.iter().any(|member| member == name) => Err(
                RuntimeError::Invalid(format!("No member {name} in the team")),
            ),
            _ => Ok(()),
        }
    }

    /// Dependencies that are distinct tasks of this board other than `id`.
    fn check_dependencies(&self, id: &str, depends_on: &[String]) -> Result<(), RuntimeError> {
        let mut seen = HashSet::new();
        for dependency in depends_on {
            if dependency == id {
                return Err(RuntimeError::TaskDependency(format!(
                    "Task {id} cannot depend on itself"
                )));
            }
            if self.task(dependency).is_none() {
                return Err(RuntimeError::TaskDependency(format!(
                    "Task dependency {dependency} is not a task of this team"
                )));
            }
            if !seen.insert(dependency) {
                return Err(RuntimeError::Invalid(format!(
                    "Task dependency {dependency} is listed twice"
                )));
            }
        }
        Ok(())
    }

    /// `Task depends on unfinished task_<id>`: the first dependency not `done`.
    fn check_finished(&self, depends_on: &[String]) -> Result<(), RuntimeError> {
        match depends_on
            .iter()
            .find(|d| self.task(d).is_none_or(|t| t.status != TaskStatus::Done))
        {
            Some(dependency) => Err(RuntimeError::TaskDependency(format!(
                "Task depends on unfinished {dependency}"
            ))),
            None => Ok(()),
        }
    }

    /// `Task dependency cycle: task_a → task_b → task_a` when `id` depending on
    /// `depends_on` would close a cycle.
    fn check_cycle(&self, id: &str, depends_on: &[String]) -> Result<(), RuntimeError> {
        let mut graph: HashMap<&str, &[String]> = self
            .tasks
            .iter()
            .map(|task| (task.id.as_str(), task.depends_on.as_slice()))
            .collect();
        graph.insert(id, depends_on);
        let mut path = vec![id.to_owned()];
        if reaches(&graph, id, id, &mut path, &mut HashSet::new()) {
            return Err(RuntimeError::TaskDependency(format!(
                "Task dependency cycle: {}",
                path.join(" → ")
            )));
        }
        Ok(())
    }
}

/// Whether `target` is reachable from `from`; `path` ends with the route when it is.
fn reaches<'a>(
    graph: &HashMap<&'a str, &'a [String]>,
    from: &str,
    target: &str,
    path: &mut Vec<String>,
    visited: &mut HashSet<&'a str>,
) -> bool {
    for next in graph.get(from).copied().unwrap_or_default() {
        path.push(next.clone());
        if next == target {
            return true;
        }
        if visited.insert(next.as_str()) && reaches(graph, next, target, path, visited) {
            return true;
        }
        path.pop();
    }
    false
}

fn store(error: rusqlite::Error) -> RuntimeError {
    RuntimeError::Store(error.into())
}

fn check_title(title: &str) -> Result<(), RuntimeError> {
    if title.trim().is_empty() {
        return Err(RuntimeError::Invalid("A task has a title".into()));
    }
    Ok(())
}

/// The task `input` creates on `board`.
fn created(board: &Board, input: TaskCreate, by: &str, now: i64) -> Result<Task, RuntimeError> {
    check_title(&input.title)?;
    board.check_owner(input.owner.as_deref())?;
    let id = cyber_core::ids::new_id("task");
    board.check_dependencies(&id, &input.depends_on)?;
    Ok(Task {
        id,
        seq: board.tasks.iter().map(|task| task.seq).max().unwrap_or(0) + 1,
        title: input.title.trim().to_owned(),
        description: input.description,
        status: TaskStatus::Todo,
        owner: input.owner,
        depends_on: input.depends_on,
        created_by: by.to_owned(),
        created_ms: now,
        updated_ms: now,
        version: 0,
    })
}

/// The owner `input` asks for: its `owner`, else the caller for a claim.
fn requested_owner(input: &TaskUpdate, caller: &Caller) -> Option<String> {
    input
        .owner
        .clone()
        .or_else(|| (input.status == Some(TaskStatus::InProgress)).then(|| caller.name.clone()))
}

/// `Task already claimed by <name>`: a claim of a task another member is working on.
fn check_claim(task: &Task, input: &TaskUpdate, owner: Option<&str>) -> Result<(), RuntimeError> {
    match task.owner.as_deref() {
        Some(current)
            if input.status == Some(TaskStatus::InProgress)
                && task.status == TaskStatus::InProgress
                && owner != Some(current) =>
        {
            Err(RuntimeError::TaskClaimed(current.to_owned()))
        }
        _ => Ok(()),
    }
}

/// What a teammate may not do: cancel, change a task another member owns, or assign a
/// task to someone else. The lead may do all of it.
fn check_teammate(task: &Task, input: &TaskUpdate, caller: &Caller) -> Result<(), RuntimeError> {
    if caller.lead {
        return Ok(());
    }
    let refuse = |message: String| RuntimeError::Forbidden {
        reason: "team_lead_only",
        message,
    };
    if input.status == Some(TaskStatus::Cancelled) {
        return Err(refuse("Only the lead can cancel a task".into()));
    }
    if let Some(owner) = task.owner.as_deref().filter(|owner| *owner != caller.name) {
        return Err(refuse(format!(
            "Only the lead can change {} owned by {owner}",
            task.id
        )));
    }
    match input.owner.as_deref() {
        Some(owner) if owner != caller.name => Err(refuse(
            "Only the lead can assign a task to another member".into(),
        )),
        _ => Ok(()),
    }
}

/// The task as `input` leaves it on `board`, with its previous status.
fn updated(
    board: &Board,
    input: TaskUpdate,
    caller: &Caller,
    now: i64,
) -> Result<(Task, TaskStatus), RuntimeError> {
    let task = board
        .task(&input.id)
        .ok_or_else(|| RuntimeError::Invalid(format!("No task {} in the team", input.id)))?;
    if let Some(version) = input.expected_version.filter(|v| *v != task.version) {
        return Err(RuntimeError::Conflict(format!(
            "Task changed since version {version}"
        )));
    }
    let owner = requested_owner(&input, caller);
    check_claim(task, &input, owner.as_deref())?;
    check_teammate(task, &input, caller)?;
    board.check_owner(owner.as_deref())?;
    if let Some(depends_on) = &input.depends_on {
        board.check_dependencies(&task.id, depends_on)?;
        board.check_cycle(&task.id, depends_on)?;
    }
    let status = input.status.unwrap_or(task.status);
    let depends_on = input.depends_on.unwrap_or_else(|| task.depends_on.clone());
    let starts = status != task.status || depends_on != task.depends_on;
    if starts && matches!(status, TaskStatus::InProgress | TaskStatus::Done) {
        board.check_finished(&depends_on)?;
    }
    if let Some(title) = &input.title {
        check_title(title)?;
    }
    let next = Task {
        title: input
            .title
            .map_or_else(|| task.title.clone(), |t| t.trim().to_owned()),
        description: input
            .description
            .unwrap_or_else(|| task.description.clone()),
        status,
        owner: owner.or_else(|| task.owner.clone()),
        depends_on,
        updated_ms: now,
        version: task.version + 1,
        ..task.clone()
    };
    Ok((next, task.status))
}

impl Runtime {
    /// `team_task_create` by the member Session `session_id`.
    pub async fn team_task_create(
        &self,
        session_id: &str,
        input: TaskCreate,
    ) -> Result<Task, RuntimeError> {
        let (team, member) = self.board_member(session_id)?;
        let (team_id, by) = (team.id.clone(), member.name.clone());
        let change = self
            .write_task(&team, move |tx| {
                let board = Board::load(tx, &team_id)?;
                let task = created(&board, input, &by, now())?;
                Ok(TaskUpdated {
                    team_id,
                    task,
                    previous_status: None,
                    by,
                })
            })
            .await?;
        self.inner
            .team_task_hook(session_id, "TaskCreated", &change)
            .await;
        Ok(change.task)
    }

    /// `team_task_update` by the member Session `session_id`; a claim is a move to
    /// `in_progress`. A move to `done` runs `TaskCompleted`.
    pub async fn team_task_update(
        &self,
        session_id: &str,
        input: TaskUpdate,
    ) -> Result<Task, RuntimeError> {
        let (team, member) = self.board_member(session_id)?;
        let team_id = team.id.clone();
        let caller = Caller {
            name: member.name.clone(),
            lead: member.role == Role::Lead,
        };
        let change = self
            .write_task(&team, move |tx| {
                let board = Board::load(tx, &team_id)?;
                let (task, previous) = updated(&board, input, &caller, now())?;
                Ok(TaskUpdated {
                    team_id,
                    task,
                    previous_status: Some(previous),
                    by: caller.name,
                })
            })
            .await?;
        if change.task.status == TaskStatus::Done
            && change.previous_status != Some(TaskStatus::Done)
        {
            self.inner
                .team_task_hook(session_id, "TaskCompleted", &change)
                .await;
        }
        Ok(change.task)
    }

    /// The board of the team `session_id` is a member of, in creation order.
    pub fn team_tasks(
        &self,
        session_id: &str,
        filter: &TaskFilter,
    ) -> Result<Vec<Task>, RuntimeError> {
        let (team, _) = self.board_member(session_id)?;
        Ok(self
            .board(&team.id)?
            .into_iter()
            .filter(|task| filter.status.is_none_or(|status| task.status == status))
            .filter(|task| {
                filter
                    .owner
                    .as_deref()
                    .is_none_or(|owner| task.owner.as_deref() == Some(owner))
            })
            .collect())
    }

    /// One task of the team `session_id` is a member of.
    pub fn team_task(&self, session_id: &str, id: &str) -> Result<Task, RuntimeError> {
        let (team, _) = self.board_member(session_id)?;
        self.board(&team.id)?
            .into_iter()
            .find(|task| task.id == id)
            .ok_or_else(|| RuntimeError::Invalid(format!("No task {id} in the team")))
    }

    /// Every task of a team, in creation order.
    pub fn board(&self, team_id: &str) -> Result<Vec<Task>, RuntimeError> {
        let team_id = team_id.to_owned();
        Ok(self
            .inner
            .store
            .read(move |db| Ok(tasks_of(db, &team_id)?))?)
    }

    /// The team and member row of a board caller, refused outside a team or while
    /// `teams` is off in the team's directory.
    fn board_member(&self, session_id: &str) -> Result<(Team, Member), RuntimeError> {
        let (team, member) = self.team_membership(session_id)?.ok_or_else(|| {
            RuntimeError::Invalid(format!("Session {session_id} is not a member of a team"))
        })?;
        self.inner
            .tools
            .team_settings(&team.directory)
            .map_err(RuntimeError::Invalid)?
            .ok_or(RuntimeError::FeatureDisabled(cyber_core::teams::DISABLED))?;
        Ok((team, member))
    }

    /// Decide a task change and append it to the lead's stream in one transaction,
    /// under the lead's Session lock.
    async fn write_task<F>(&self, team: &Team, decide: F) -> Result<TaskUpdated, RuntimeError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<TaskUpdated, RuntimeError> + Send + 'static,
    {
        let handle = self.inner.handle(&team.lead_id).await?;
        let mut state = handle.state.lock().await;
        let lead = state.info.id.clone();
        let (stored, decision) =
            self.inner
                .store
                .append_checked(&lead, Expected::Seq(state.last_seq), move |tx| {
                    Ok(match decide(tx) {
                        Ok(change) => (vec![event(records::TASK_UPDATED, &change)], Ok(change)),
                        Err(refusal) => (Vec::new(), Err(refusal)),
                    })
                })?;
        for e in &stored {
            state.apply(e).map_err(RuntimeError::Corrupt)?;
        }
        drop(state);
        self.inner.publish(&stored);
        decision
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, owner: Option<&str>, depends_on: &[&str]) -> Task {
        Task {
            id: id.into(),
            seq: 1,
            title: id.into(),
            description: String::new(),
            status,
            owner: owner.map(str::to_owned),
            depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
            created_by: "lead".into(),
            created_ms: 1,
            updated_ms: 1,
            version: 3,
        }
    }

    fn board(tasks: Vec<Task>) -> Board {
        Board {
            tasks,
            members: ["lead", "api", "tests"].map(str::to_owned).to_vec(),
        }
    }

    fn caller(name: &str) -> Caller {
        Caller {
            name: name.into(),
            lead: name == "lead",
        }
    }

    fn update(id: &str) -> TaskUpdate {
        TaskUpdate {
            id: id.into(),
            ..TaskUpdate::default()
        }
    }

    fn refusal(result: Result<(Task, TaskStatus), RuntimeError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn a_task_starts_only_once_its_dependencies_are_done() {
        let b = board(vec![
            task("task_a", TaskStatus::Todo, None, &[]),
            task("task_b", TaskStatus::Todo, None, &["task_a"]),
        ]);
        let start = TaskUpdate {
            status: Some(TaskStatus::InProgress),
            ..update("task_b")
        };
        assert_eq!(
            refusal(updated(&b, start.clone(), &caller("api"), 9)),
            "TaskDependencyError: Task depends on unfinished task_a"
        );
        let done = TaskUpdate {
            status: Some(TaskStatus::Done),
            ..update("task_b")
        };
        assert!(updated(&b, done, &caller("lead"), 9).is_err());
        let b = board(vec![
            task("task_a", TaskStatus::Done, None, &[]),
            task("task_b", TaskStatus::Todo, None, &["task_a"]),
        ]);
        let (started, previous) = updated(&b, start, &caller("api"), 9).unwrap();
        assert_eq!(previous, TaskStatus::Todo);
        assert_eq!(started.status, TaskStatus::InProgress);
        assert_eq!(
            started.owner.as_deref(),
            Some("api"),
            "a claim owns the task"
        );
        assert_eq!((started.version, started.updated_ms), (4, 9));
    }

    #[test]
    fn cycles_and_foreign_dependencies_are_refused_when_written() {
        let b = board(vec![
            task("task_a", TaskStatus::Todo, None, &[]),
            task("task_b", TaskStatus::Todo, None, &["task_a"]),
            task("task_c", TaskStatus::Todo, None, &["task_b"]),
        ]);
        let set = |id: &str, deps: &[&str]| TaskUpdate {
            depends_on: Some(deps.iter().map(|d| (*d).to_owned()).collect()),
            ..update(id)
        };
        assert_eq!(
            refusal(updated(&b, set("task_a", &["task_b"]), &caller("lead"), 9)),
            "TaskDependencyError: Task dependency cycle: task_a → task_b → task_a"
        );
        assert_eq!(
            refusal(updated(&b, set("task_a", &["task_c"]), &caller("lead"), 9)),
            "TaskDependencyError: Task dependency cycle: task_a → task_c → task_b → task_a"
        );
        assert!(
            refusal(updated(&b, set("task_a", &["task_a"]), &caller("lead"), 9))
                .contains("cannot depend on itself")
        );
        assert!(
            refusal(updated(&b, set("task_a", &["task_zz"]), &caller("lead"), 9))
                .contains("not a task of this team")
        );
        assert!(updated(&b, set("task_c", &["task_a"]), &caller("lead"), 9).is_ok());
        let new = TaskCreate {
            title: "d".into(),
            depends_on: vec!["task_x".into()],
            ..TaskCreate::default()
        };
        assert!(created(&b, new, "api", 9).is_err());
    }

    #[test]
    fn claims_versions_and_lead_only_changes() {
        let b = board(vec![
            task("task_a", TaskStatus::InProgress, Some("api"), &[]),
            task("task_b", TaskStatus::Todo, Some("api"), &[]),
            task("task_c", TaskStatus::Todo, None, &[]),
        ]);
        let claim = |id: &str| TaskUpdate {
            status: Some(TaskStatus::InProgress),
            ..update(id)
        };
        assert_eq!(
            refusal(updated(&b, claim("task_a"), &caller("tests"), 9)),
            "TaskClaimedError: Task already claimed by api"
        );
        assert_eq!(
            refusal(updated(&b, claim("task_a"), &caller("lead"), 9)),
            "TaskClaimedError: Task already claimed by api",
            "a claim never takes a task someone works on"
        );
        assert!(updated(&b, claim("task_a"), &caller("api"), 9).is_ok());
        assert!(
            refusal(updated(&b, claim("task_b"), &caller("tests"), 9))
                .contains("Only the lead can change task_b owned by api")
        );
        let cancel = TaskUpdate {
            status: Some(TaskStatus::Cancelled),
            ..update("task_c")
        };
        assert!(
            refusal(updated(&b, cancel.clone(), &caller("api"), 9))
                .contains("Only the lead can cancel")
        );
        assert!(updated(&b, cancel, &caller("lead"), 9).is_ok());
        let assign = |owner: &str| TaskUpdate {
            owner: Some(owner.into()),
            ..update("task_c")
        };
        assert!(
            refusal(updated(&b, assign("tests"), &caller("api"), 9))
                .contains("Only the lead can assign")
        );
        let (reassigned, _) = updated(
            &b,
            TaskUpdate {
                owner: Some("tests".into()),
                ..update("task_a")
            },
            &caller("lead"),
            9,
        )
        .unwrap();
        assert_eq!(reassigned.owner.as_deref(), Some("tests"));
        assert!(
            refusal(updated(&b, assign("ghost"), &caller("lead"), 9)).contains("No member ghost")
        );
        let stale = TaskUpdate {
            title: Some("renamed".into()),
            expected_version: Some(2),
            ..update("task_c")
        };
        assert_eq!(
            refusal(updated(&b, stale, &caller("lead"), 9)),
            "ConflictError: Task changed since version 2"
        );
        let current = TaskUpdate {
            title: Some("renamed".into()),
            expected_version: Some(3),
            ..update("task_c")
        };
        assert_eq!(
            updated(&b, current, &caller("api"), 9).unwrap().0.title,
            "renamed"
        );
        assert!(
            refusal(updated(&b, update("task_zz"), &caller("lead"), 9)).contains("No task task_zz")
        );
    }

    #[test]
    fn created_tasks_follow_the_board() {
        let b = board(vec![task("task_a", TaskStatus::Done, None, &[])]);
        let new = created(
            &b,
            TaskCreate {
                title: " schema ".into(),
                depends_on: vec!["task_a".into()],
                owner: Some("tests".into()),
                ..TaskCreate::default()
            },
            "api",
            7,
        )
        .unwrap();
        assert!(new.id.starts_with("task_"));
        assert_eq!(
            (new.seq, new.title.as_str(), new.status, new.version),
            (2, "schema", TaskStatus::Todo, 0)
        );
        assert_eq!(new.created_by, "api");
        assert!(created(&b, TaskCreate::default(), "api", 7).is_err());
    }
}
