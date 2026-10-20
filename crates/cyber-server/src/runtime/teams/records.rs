//! Durable team records (`agent-teams` → Team events): the `team.*` events of the lead's
//! stream (the team's single writer), validated on append, and their projector into
//! `team`, `team_member` and `team_task`. A rebuild clears the tables and re-projects
//! the events, so the rows are always a function of the stream.

use cyber_core::budget::Budget;
use cyber_core::teams::Isolation;
use rusqlite::{Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use cyber_store::{EventRegistry, StoredEvent};

pub const CREATED: &str = "team.created.1";
pub const MEMBER_SPAWNED: &str = "team.member.spawned.1";
pub const MEMBER_UPDATED: &str = "team.member.updated.1";
pub const TASK_UPDATED: &str = "team.task.updated.1";
pub const MEMBER_IDLE: &str = "team.member.idle.1";
pub const BUDGET_EXCEEDED: &str = "team.budget.exceeded.1";
pub const STOPPED: &str = "team.stopped.1";
pub const RESUMED: &str = "team.resumed.1";

/// The projection tables, children first, in the order a rebuild clears them.
pub const TABLES: [&str; 3] = ["team_task", "team_member", "team"];
/// The event type prefix every team record shares.
pub const PREFIX: &str = "team.";
/// The longest last status line kept for a member.
pub const MAX_STATUS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Lead,
    Teammate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    /// A Drain runs.
    Working,
    Idle,
    /// A permission or question request is open.
    WaitingInput,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    User,
    Budget,
    LeadDeleted,
}

/// The team label of a member Session, set at creation and never changed, so every
/// surface can label a teammate without a join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamTag {
    pub team_id: String,
    pub role: Role,
    /// The member name: `lead`, or the teammate's name.
    pub name: String,
}

impl TeamTag {
    pub fn validate(&self) -> Result<(), String> {
        team_id(&self.team_id)?;
        member_name(&self.name)?;
        if (self.name == "lead") != (self.role == Role::Lead) {
            return Err("the member named lead, and only it, has the lead role".into());
        }
        Ok(())
    }
}

/// `team.created.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Created {
    pub team_id: String,
    /// The definition name, or `adhoc`.
    pub name: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// The definition the team was started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// The definition body, admitted to the lead as `team_instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// `team.member.spawned.1`; the lead is the member named `lead`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberSpawned {
    pub team_id: String,
    pub name: String,
    pub session_id: String,
    pub role: Role,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub isolation: Isolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub owns: Vec<String>,
}

/// `team.member.updated.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberUpdated {
    pub team_id: String,
    pub name: String,
    pub status: MemberStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}

/// A task as it is after a change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub seq: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub created_by: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub version: i64,
}

/// `team.task.updated.1`: the full task after the change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskUpdated {
    pub team_id: String,
    pub task: Task,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<TaskStatus>,
    /// The member that made the change.
    pub by: String,
}

/// `team.member.idle.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberIdle {
    pub team_id: String,
    pub name: String,
    #[serde(default)]
    pub tasks_in_progress: Vec<String>,
    pub last_status: String,
}

/// One member's share of a team's spend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberSpend {
    pub name: String,
    pub turns: u64,
    pub tokens: u64,
    pub cost_usd: f64,
}

/// `team.budget.exceeded.1`: recorded once; the team moves to `stopping`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetExceeded {
    pub team_id: String,
    pub budget: Budget,
    pub spent: Value,
    pub members: Vec<MemberSpend>,
}

/// `team.stopped.1`. With `stopping` the team only begins to stop (teammates finish
/// their current Turn); without it the team is stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stopped {
    pub team_id: String,
    pub reason: StopReason,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

/// `team.resumed.1`: `lead_id` is the lead from now on (a new one after `lead_deleted`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resumed {
    pub team_id: String,
    pub lead_id: String,
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CREATED, validate_created)
        .and_then(|r| r.register_with(MEMBER_SPAWNED, validate_spawned))
        .and_then(|r| r.register_with(MEMBER_UPDATED, validate_member_updated))
        .and_then(|r| r.register_with(TASK_UPDATED, validate_task_updated))
        .and_then(|r| {
            r.register_with(MEMBER_IDLE, |v| {
                let idle: MemberIdle = decode(v)?;
                team_id(&idle.team_id)?;
                member_name(&idle.name)
            })
        })
        .and_then(|r| {
            r.register_with(BUDGET_EXCEEDED, |v| {
                let exceeded: BudgetExceeded = decode(v)?;
                team_id(&exceeded.team_id)?;
                exceeded.budget.validate()
            })
        })
        .and_then(|r| r.register_with(STOPPED, |v| team_id(&decode::<Stopped>(v)?.team_id)))
        .and_then(|r| {
            r.register_with(RESUMED, |v| {
                let resumed: Resumed = decode(v)?;
                team_id(&resumed.team_id)?;
                prefixed(&resumed.lead_id, "ses", "lead_id")
            })
        })
        .expect("valid team events");
}

fn validate_created(data: &Value) -> Result<(), String> {
    let created: Created = decode(data)?;
    team_id(&created.team_id)?;
    if created.name != "adhoc" && !cyber_core::teams::valid_name(&created.name) {
        return Err(format!("team name {:?} is not valid", created.name));
    }
    if created.directory.is_empty() {
        return Err("a team has a directory".into());
    }
    created.budget.as_ref().map_or(Ok(()), Budget::validate)
}

fn validate_spawned(data: &Value) -> Result<(), String> {
    let spawned: MemberSpawned = decode(data)?;
    prefixed(&spawned.session_id, "ses", "session_id")?;
    TeamTag {
        team_id: spawned.team_id.clone(),
        role: spawned.role,
        name: spawned.name.clone(),
    }
    .validate()?;
    if spawned.worktree_id.is_some() != (spawned.isolation == Isolation::Worktree) {
        return Err("a worktree member, and only one, has a worktree".into());
    }
    Ok(())
}

fn validate_member_updated(data: &Value) -> Result<(), String> {
    let updated: MemberUpdated = decode(data)?;
    team_id(&updated.team_id)?;
    member_name(&updated.name)?;
    match &updated.last_status {
        Some(line) if line.chars().count() > MAX_STATUS => Err(format!(
            "a last status line is at most {MAX_STATUS} characters"
        )),
        _ => Ok(()),
    }
}

fn validate_task_updated(data: &Value) -> Result<(), String> {
    let updated: TaskUpdated = decode(data)?;
    team_id(&updated.team_id)?;
    member_name(&updated.by)?;
    let task = &updated.task;
    prefixed(&task.id, "task", "task.id")?;
    if task.title.trim().is_empty() {
        return Err("a task has a title".into());
    }
    if let Some(dependency) = task
        .depends_on
        .iter()
        .find(|d| !cyber_core::ids::has_prefix(d, "task") || **d == task.id)
    {
        return Err(format!(
            "task dependency {dependency:?} is not another task"
        ));
    }
    task.owner.as_deref().map_or(Ok(()), member_name)
}

fn team_id(id: &str) -> Result<(), String> {
    prefixed(id, "team", "team_id")
}

fn prefixed(id: &str, prefix: &str, field: &str) -> Result<(), String> {
    if cyber_core::ids::has_prefix(id, prefix) {
        Ok(())
    } else {
        Err(format!("{field} {id:?} is not a {prefix}_ ID"))
    }
}

/// `lead`, or a valid teammate name.
fn member_name(name: &str) -> Result<(), String> {
    if name == "lead" || (cyber_core::teams::valid_name(name) && name != "all") {
        Ok(())
    } else {
        Err(format!("member name {name:?} is not valid"))
    }
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|error| error.to_string())
}

fn text<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

/// The serialized name of a unit enum value.
fn name<T: Serialize>(value: T) -> Result<String, String> {
    match serde_json::to_value(value).map_err(|error| error.to_string())? {
        Value::String(name) => Ok(name),
        other => Err(format!("{other} is not a name")),
    }
}

/// Write the team tables in the event transaction.
pub(crate) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    let result = match e.kind.as_str() {
        CREATED => project_created(tx, e, decode(&e.data)?)?,
        MEMBER_SPAWNED => project_spawned(tx, decode(&e.data)?)?,
        MEMBER_UPDATED => {
            let u: MemberUpdated = decode(&e.data)?;
            tx.execute(
                "UPDATE team_member SET status = ?3, last_status = COALESCE(?4, last_status)
                 WHERE team_id = ?1 AND name = ?2",
                params![u.team_id, u.name, name(u.status)?, u.last_status],
            )
        }
        TASK_UPDATED => project_task(tx, decode(&e.data)?)?,
        MEMBER_IDLE => {
            let idle: MemberIdle = decode(&e.data)?;
            tx.execute(
                "UPDATE team_member SET status = 'idle', last_status = ?3
                 WHERE team_id = ?1 AND name = ?2",
                params![idle.team_id, idle.name, idle.last_status],
            )
        }
        BUDGET_EXCEEDED => {
            let exceeded: BudgetExceeded = decode(&e.data)?;
            set_status(tx, &exceeded.team_id, "stopping", Some("budget"), e.time_ms)
        }
        STOPPED => project_stopped(tx, decode(&e.data)?, e.time_ms)?,
        RESUMED => project_resumed(tx, decode(&e.data)?, e.time_ms)?,
        _ => return Ok(()),
    };
    result.map(drop).map_err(|error| error.to_string())
}

fn project_created(
    tx: &Transaction<'_>,
    e: &StoredEvent,
    c: Created,
) -> Result<rusqlite::Result<usize>, String> {
    let budget = c.budget.as_ref().map(text).transpose()?;
    Ok(tx.execute(
        "INSERT INTO team (id, name, lead_id, directory, status, budget, instructions,
             created_ms, updated_ms)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?7)",
        params![
            c.team_id,
            c.name,
            e.aggregate_id,
            c.directory,
            budget,
            c.instructions,
            e.time_ms
        ],
    ))
}

fn project_spawned(
    tx: &Transaction<'_>,
    s: MemberSpawned,
) -> Result<rusqlite::Result<usize>, String> {
    Ok(tx.execute(
        "INSERT INTO team_member (team_id, name, session_id, role, agent, model, isolation,
             worktree_id, branch, owns, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'idle')",
        params![
            s.team_id,
            s.name,
            s.session_id,
            name(s.role)?,
            s.agent,
            s.model,
            name(s.isolation)?,
            s.worktree_id,
            s.branch,
            text(&s.owns)?
        ],
    ))
}

fn project_task(tx: &Transaction<'_>, u: TaskUpdated) -> Result<rusqlite::Result<usize>, String> {
    let t = u.task;
    Ok(tx.execute(
        "INSERT INTO team_task (id, team_id, seq, title, description, status, owner,
             depends_on, created_by, created_ms, updated_ms, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, title = excluded.title,
             description = excluded.description, status = excluded.status,
             owner = excluded.owner, depends_on = excluded.depends_on,
             updated_ms = excluded.updated_ms, version = excluded.version
         WHERE team_task.team_id = excluded.team_id",
        params![
            t.id,
            u.team_id,
            t.seq,
            t.title,
            t.description,
            name(t.status)?,
            t.owner,
            text(&t.depends_on)?,
            t.created_by,
            t.created_ms,
            t.updated_ms,
            t.version
        ],
    ))
}

fn project_stopped(
    tx: &Transaction<'_>,
    s: Stopped,
    time_ms: i64,
) -> Result<rusqlite::Result<usize>, String> {
    let reason = name(s.reason)?;
    if s.stopping {
        return Ok(set_status(
            tx,
            &s.team_id,
            "stopping",
            Some(&reason),
            time_ms,
        ));
    }
    if let Err(error) = tx.execute(
        "UPDATE team_member SET status = 'stopped' WHERE team_id = ?1",
        params![s.team_id],
    ) {
        return Ok(Err(error));
    }
    Ok(set_status(
        tx,
        &s.team_id,
        "stopped",
        Some(&reason),
        time_ms,
    ))
}

fn project_resumed(
    tx: &Transaction<'_>,
    r: Resumed,
    time_ms: i64,
) -> Result<rusqlite::Result<usize>, String> {
    // A new lead (after `lead_deleted`) takes the lead's member row.
    if let Err(error) = tx.execute(
        "UPDATE team_member SET status = 'idle',
             session_id = CASE WHEN role = 'lead' THEN ?2 ELSE session_id END
         WHERE team_id = ?1",
        params![r.team_id, r.lead_id],
    ) {
        return Ok(Err(error));
    }
    Ok(tx.execute(
        "UPDATE team SET status = 'active', stop_reason = NULL,
             former_leads = CASE WHEN lead_id = ?2 THEN former_leads
                 ELSE json_insert(former_leads, '$[#]', lead_id) END,
             lead_id = ?2, updated_ms = ?3
         WHERE id = ?1",
        params![r.team_id, r.lead_id, time_ms],
    ))
}

fn set_status(
    tx: &Transaction<'_>,
    team_id: &str,
    status: &str,
    reason: Option<&str>,
    time_ms: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE team SET status = ?2, stop_reason = ?3, updated_ms = ?4 WHERE id = ?1",
        params![team_id, status, reason, time_ms],
    )
}
