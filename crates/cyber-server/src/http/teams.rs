//! Feature, team definition and team routes (`configuration` → Feature flags registry,
//! `agent-teams`; design section 14). Team routes answer `404 FeatureDisabledError`
//! while `teams` is off for the team's directory (the request's Location for the list and
//! team definitions, the lead's for a new team).

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::{StatusCode, request::Parts};
use axum::response::Response;
use axum::{Json, Router, routing::get, routing::post};
use cyber_core::budget::Budget;
use cyber_core::features::{FeatureStatus, TEAMS};
use cyber_core::teams::Definitions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use super::events::{EventEnvelope, envelope, spawn_stream, sse_event, stream_response};
use super::{
    ApiError, AppState,
    envelope::{Data, Located, LocationInfo, location, query_value},
};
use crate::runtime::teams::{
    SpawnedMember, Task, TaskFilter, TaskStatus, Team, TeamListing, TeamSpawn, TeamStart, TeamView,
};
use crate::runtime::{LiveEvent, Runtime};

/// `POST /api/v1/teams`: a team led by `lead_session_id`, from a definition of the
/// lead's Location, from teammates, or both.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeam {
    pub lead_session_id: String,
    /// A team definition name (`cyber teams definitions`).
    #[serde(default)]
    pub definition: Option<String>,
    /// Teammates to spawn, after the definition's.
    #[serde(default)]
    pub teammates: Vec<TeamSpawn>,
    /// Over the definition's budget and `budgets.team`.
    #[serde(default)]
    pub budget: Option<Budget>,
}

/// `POST /api/v1/teams/:id/resume`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeTeam {
    /// The new lead after `lead_deleted`: a top-level Session in no team.
    #[serde(default)]
    pub lead: Option<String>,
}

/// A started team and its spawned teammates.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreatedTeam {
    pub team: TeamView,
    pub spawned: Vec<SpawnedMember>,
}

type Result<T> = std::result::Result<T, ApiError>;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/features", get(features))
        .route("/team-definitions", get(definitions))
        .route("/teams", get(list).post(create))
        .route("/teams/{team_id}", get(show))
        .route("/teams/{team_id}/tasks", get(tasks))
        .route("/teams/{team_id}/stop", post(stop))
        .route("/teams/{team_id}/resume", post(resume))
        .route("/teams/{team_id}/events", get(events))
}

async fn features(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<FeatureStatus>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    let features = state.services.features(&directory)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: features.features,
    }))
}

async fn definitions(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Definitions>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    require_teams(&state, &directory)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: state.services.team_definitions(&directory)?,
    }))
}

/// `404 FeatureDisabledError` unless `teams` is on for `directory`.
pub(crate) fn require_teams(state: &AppState, directory: &std::path::Path) -> Result<()> {
    if state.services.features(directory)?.enabled(TEAMS) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        "FeatureDisabledError",
        cyber_core::teams::DISABLED,
    ))
}

/// A known team whose directory has `teams` on.
fn known(state: &AppState, team_id: &str) -> Result<Team> {
    let team = state
        .runtime
        .team(team_id)?
        .ok_or_else(|| crate::runtime::RuntimeError::TeamNotFound(team_id.to_owned()))?;
    require_teams(state, std::path::Path::new(&team.directory))?;
    Ok(team)
}

fn query(parts: &Parts, name: &str) -> Option<String> {
    parts.uri.query().and_then(|q| query_value(q, name))
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<TeamListing>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    require_teams(&state, &directory)?;
    let status = query(&parts, "status");
    if let Some(status) = status.as_deref()
        && !["active", "stopping", "stopped"].contains(&status)
    {
        return Err(ApiError::invalid(format!(
            "status must be active, stopping or stopped, not {status:?}"
        )));
    }
    let teams = state
        .runtime
        .teams(&directory.display().to_string(), status.as_deref())?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: teams,
    }))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateTeam>,
) -> Result<(StatusCode, Json<Data<CreatedTeam>>)> {
    let lead = state.runtime.state(&body.lead_session_id).await?.info;
    let directory = std::path::PathBuf::from(&lead.directory);
    require_teams(&state, &directory)?;
    let definition = match body.definition.as_deref() {
        Some(name) => Some(definition(&state, &directory, name)?),
        None => None,
    };
    let start = TeamStart {
        definition,
        teammates: body.teammates,
        budget: body.budget,
    };
    let (team, spawned) = state.runtime.start_team(&lead.id, start).await?;
    let team = state.runtime.team_view(&team.id)?;
    Ok((
        StatusCode::CREATED,
        Json(Data {
            data: CreatedTeam { team, spawned },
        }),
    ))
}

/// The definition `name` of `directory`; one that failed to load reports why.
fn definition(
    state: &AppState,
    directory: &std::path::Path,
    name: &str,
) -> Result<cyber_core::teams::TeamDefinition> {
    let definitions = state.services.team_definitions(directory)?;
    if let Some(definition) = definitions.get(name) {
        return Ok(definition.clone());
    }
    let issue = definitions
        .issues
        .iter()
        .find(|issue| issue.path.file_stem().and_then(|stem| stem.to_str()) == Some(name));
    Err(ApiError::invalid(match issue {
        Some(issue) => format!("Team definition {name} is not valid: {}", issue.message),
        None => format!("No team definition {name}"),
    }))
}

async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<TeamView>>> {
    known(&state, &id)?;
    Ok(Json(Data {
        data: state.runtime.team_view(&id)?,
    }))
}

async fn tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<Data<Vec<Task>>>> {
    known(&state, &id)?;
    let status = query(&parts, "status")
        .map(|status| {
            serde_json::from_value::<TaskStatus>(json!(status)).map_err(|_| {
                ApiError::invalid(format!(
                    "status must be todo, in_progress, blocked, done or cancelled, not {status:?}"
                ))
            })
        })
        .transpose()?;
    let filter = TaskFilter {
        status,
        owner: query(&parts, "owner"),
    };
    let tasks = state
        .runtime
        .board(&id)?
        .into_iter()
        .filter(|task| filter.status.is_none_or(|status| task.status == status))
        .filter(|task| {
            filter
                .owner
                .as_deref()
                .is_none_or(|owner| task.owner.as_deref() == Some(owner))
        })
        .collect();
    Ok(Json(Data { data: tasks }))
}

async fn stop(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Data<Team>>> {
    known(&state, &id)?;
    Ok(Json(Data {
        data: state.runtime.team_stop(&id).await?,
    }))
}

async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ResumeTeam>>,
) -> Result<Json<Data<Team>>> {
    known(&state, &id)?;
    let body = body.map(|Json(body)| body).unwrap_or_default();
    Ok(Json(Data {
        data: state.runtime.team_resume(&id, body.lead.as_deref()).await?,
    }))
}

/// Live `team.*` events of the team (on its lead's stream) and the permission and
/// question events of its members, for the team view.
async fn events(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    known(&state, &id)?;
    // Subscribe before reading the members, so a spawn in between is not missed.
    let live = state.runtime.subscribe();
    let members = member_sessions(&state.runtime, &id)?;
    let (tx, rx) = mpsc::channel(256);
    let runtime = state.runtime.clone();
    spawn_stream(
        runtime.clone(),
        tx.clone(),
        follow_team(runtime.clone(), id, members, live, tx),
    );
    Ok(stream_response(rx, runtime))
}

fn member_sessions(runtime: &Runtime, team_id: &str) -> Result<HashSet<String>> {
    Ok(runtime
        .team_members(team_id)?
        .into_iter()
        .map(|member| member.session_id)
        .collect())
}

async fn follow_team(
    runtime: Runtime,
    team_id: String,
    mut members: HashSet<String>,
    mut live: broadcast::Receiver<LiveEvent>,
    tx: mpsc::Sender<axum::response::sse::Event>,
) {
    let hello = EventEnvelope {
        id: "live:0".into(),
        kind: "server.connected".into(),
        data: json!({}),
        location: None,
        durable: None,
    };
    if tx.send(sse_event(&hello)).await.is_err() {
        return;
    }
    let mut counter = 0u64;
    loop {
        let event = match live.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Some(membership_changed) = team_event(&event, &team_id, &members) else {
            continue;
        };
        if membership_changed && let Ok(current) = member_sessions(&runtime, &team_id) {
            members = current;
        }
        counter += 1;
        if tx
            .send(sse_event(&envelope(&event, counter)))
            .await
            .is_err()
        {
            return;
        }
    }
}

/// Whether the team stream carries `event` (`Some`), and whether it changes who the
/// members are (`Some(true)`): a `team.*` record of this team on any stream, so a new
/// lead's records count, or a member's permission or question event.
fn team_event(event: &LiveEvent, team_id: &str, members: &HashSet<String>) -> Option<bool> {
    match event {
        LiveEvent::Durable {
            session_id,
            kind,
            data,
            ..
        } => {
            if kind.starts_with("team.") {
                return (data["team_id"] == team_id).then(|| {
                    kind == crate::runtime::teams::records::MEMBER_SPAWNED
                        || kind == crate::runtime::teams::records::RESUMED
                });
            }
            let request = kind.starts_with("permission.") || kind.starts_with("question.");
            (request && members.contains(session_id)).then_some(false)
        }
        LiveEvent::RequestRouted { session_id, .. } => {
            members.contains(session_id).then_some(false)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn durable(session: &str, kind: &str, team: &str) -> LiveEvent {
        LiveEvent::Durable {
            session_id: session.into(),
            seq: 1,
            kind: kind.into(),
            data: json!({"team_id": team}),
        }
    }

    #[test]
    fn the_team_stream_carries_team_records_and_member_requests_only() {
        let members: HashSet<String> = ["ses_lead".to_owned(), "ses_api".to_owned()].into();
        let of = |event: LiveEvent| team_event(&event, "team_1", &members);
        assert_eq!(
            of(durable("ses_lead", "team.task.updated.1", "team_1")),
            Some(false)
        );
        assert_eq!(
            of(durable("ses_new", "team.resumed.1", "team_1")),
            Some(true)
        );
        assert_eq!(
            of(durable("ses_lead", "team.member.spawned.1", "team_1")),
            Some(true)
        );
        assert_eq!(
            of(durable("ses_lead", "team.task.updated.1", "team_2")),
            None
        );
        assert_eq!(
            of(durable("ses_api", "permission.asked.1", "")),
            Some(false)
        );
        assert_eq!(of(durable("ses_other", "permission.asked.1", "")), None);
        assert_eq!(
            of(durable("ses_api", "session.prompt.admitted.1", "")),
            None
        );
    }
}
