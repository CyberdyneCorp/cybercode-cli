//! Session routes (`server-api` → Session and prompt routes).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cyber_llm::Content;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AppState;
use super::envelope::{Cursor, Data, Located, LocationInfo, Page, location};
use super::error::ApiError;
use crate::runtime::{
    Admission, CallState, CreateSession, Delivery, Entry, FileDiff, InboxRow, ListFilter,
    PermissionReply, QuestionReply, Receipt, RevertState, RevertTarget, RuntimeError, SessionInfo,
    SessionRow, SessionState, TaskState, Totals,
};

type Result<T> = std::result::Result<T, ApiError>;

/// A Session with its live status.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Session {
    #[serde(flatten)]
    pub info: SessionInfo,
    /// `idle` or `running`.
    pub status: String,
    /// Mode of the running Turn, or the selected mode while idle.
    #[serde(default)]
    pub effective_mode: String,
    /// Selected mode waiting for the next Turn.
    pub pending_mode: Option<String>,
    /// Sequence of the last durable event; stream from here to follow new activity.
    pub seq: i64,
    pub totals: Totals,
    #[serde(flatten)]
    pub children_usage: crate::runtime::ChildrenUsage,
    pub revert: Option<RevertState>,
}

/// A history entry with the tool calls it made.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Message {
    #[serde(flatten)]
    pub entry: Entry,
    pub tools: Vec<CallState>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionContext {
    pub epoch: Option<u32>,
    pub baseline: Option<String>,
    pub summary: Option<String>,
    pub task: TaskState,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ShellResult {
    pub output: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub budget: Option<cyber_core::budget::Budget>,
    pub id: Option<String>,
    /// Explicit `provider/model[#variant]`; omitted model uses agent then Location defaults.
    pub model: Option<String>,
    pub agent: Option<String>,
    pub mode: Option<String>,
    pub title: Option<String>,
    pub parent_id: Option<String>,
    /// Session ruleset in the `permissions` config shape.
    pub rules: Option<Value>,
    pub max_steps: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReopenSubtreeBody {
    pub scope_id: String,
    pub stop_receipt_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub archived: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ForkBody {
    /// Copy history before this message; all of it when absent.
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubtaskBody {
    /// Explicit user request for a background child.
    pub prompt: String,
    /// A visible subagent-capable profile. Omit to fork the current agent/context.
    pub agent: Option<String>,
    /// Additional model-visible Content, retained without text conversion.
    #[serde(default)]
    pub attachments: Vec<Content>,
    /// Positive ceiling capped by the selected profile's subagent step limit.
    pub max_steps: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PromptBody {
    pub id: Option<String>,
    pub parts: Vec<Content>,
    #[serde(default = "steer")]
    pub delivery: Delivery,
    #[serde(default = "yes")]
    pub resume: bool,
    /// `user` unless a client relays another source.
    pub source: Option<String>,
}

fn steer() -> Delivery {
    Delivery::Steer
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommandBody {
    /// A skill or custom command name, without the slash.
    pub name: String,
    #[serde(default)]
    pub arguments: String,
    pub id: Option<String>,
    #[serde(default = "steer")]
    pub delivery: Delivery,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AgentBody {
    pub agent: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ModelBody {
    pub model: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ModeBody {
    pub mode: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CompactBody {
    pub instructions: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RevertBody {
    pub message_id: String,
    pub target: RevertTarget,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShellBody {
    pub command: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InboxEditBody {
    pub parts: Option<Vec<Content>>,
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ReleaseBody {
    /// `steer` (default) or `queue`.
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyKind {
    Once,
    Always,
    Reject,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PermissionReplyBody {
    pub reply: ReplyKind,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuestionReplyBody {
    /// Selected labels (or custom text) per question; omit to dismiss.
    pub answers: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub search: Option<String>,
    /// Include child Sessions (default false).
    pub children: Option<bool>,
    pub archived: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MessagesQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DiffQuery {
    pub message_id: String,
}

pub fn routes() -> Router<AppState> {
    let s = "/sessions/{session_id}";
    Router::new()
        .route("/sessions", get(list).post(create))
        .route(s, get(show).patch(update).delete(remove))
        .route(&format!("{s}/fork"), post(fork))
        .route(&format!("{s}/subtask"), post(subtask))
        .route(
            &format!("{s}/delegations/{{request_id}}"),
            get(delegation).post(start_delegation),
        )
        .route(
            &format!("{s}/delegations/{{request_id}}/stop"),
            post(stop_delegation),
        )
        .route(&format!("{s}/prompt"), post(prompt))
        .route(&format!("{s}/interrupt"), post(interrupt))
        .route(&format!("{s}/stop-subtree"), post(stop_subtree))
        .route(&format!("{s}/reopen-subtree"), post(reopen_subtree))
        .route(&format!("{s}/wake"), post(wake))
        .route(&format!("{s}/command"), post(command))
        .route(&format!("{s}/approve"), post(approve_auto))
        .route(&format!("{s}/agent"), post(agent))
        .route(&format!("{s}/model"), post(model))
        .route(&format!("{s}/mode"), post(mode))
        .route(&format!("{s}/compact"), post(compact))
        .route(&format!("{s}/rewind"), post(rewind))
        .route(&format!("{s}/revert/stage"), post(stage))
        .route(&format!("{s}/revert/clear"), post(clear))
        .route(&format!("{s}/revert/commit"), post(commit))
        .route(&format!("{s}/shell"), post(shell))
        .route(&format!("{s}/messages"), get(messages))
        .route(&format!("{s}/messages/{{message_id}}"), get(message))
        .route(&format!("{s}/inbox"), get(inbox))
        .route(
            &format!("{s}/inbox/{{message_id}}"),
            axum::routing::patch(edit_input).delete(remove_input),
        )
        .route(&format!("{s}/inbox/{{message_id}}/release"), post(release))
        .route(
            &format!("{s}/inbox/{{message_id}}/drop"),
            post(remove_input),
        )
        .route(&format!("{s}/context"), get(context))
        .route(&format!("{s}/diff"), get(diff))
        .route(
            &format!("{s}/permissions/{{request_id}}/reply"),
            post(reply_permission),
        )
        .route(
            &format!("{s}/questions/{{request_id}}/reply"),
            post(answer_question),
        )
}

pub(super) fn session(state: &AppState, s: SessionState) -> Session {
    let status = if state.runtime.is_running(&s.info.id) {
        "running"
    } else {
        "idle"
    };
    let running = status == "running";
    let effective_mode = s.effective_mode(running).to_string();
    let pending_mode = s.pending_mode(running).map(str::to_string);
    Session {
        effective_mode,
        pending_mode,
        status: status.into(),
        seq: s.last_seq,
        totals: s.totals,
        children_usage: s.children_usage,
        revert: s.revert,
        info: s.info,
    }
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<ListQuery>,
) -> Result<Json<Located<Page<SessionRow>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    let filter = ListFilter {
        directory: Some(directory.display().to_string()),
        roots_only: !q.children.unwrap_or(false),
        include_archived: q.archived.unwrap_or(false),
        search: q.search,
        limit: Some(q.limit.unwrap_or(50).clamp(1, 200)),
        cursor: q.cursor,
        ..ListFilter::default()
    };
    let page = state.runtime.list(&filter).map_err(|e| match e {
        RuntimeError::Invalid(m) => ApiError::new(StatusCode::BAD_REQUEST, "InvalidCursorError", m),
        other => other.into(),
    })?;
    let data = Page {
        data: page.sessions,
        cursor: Cursor {
            previous: None,
            next: page.next,
        },
    };
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn create(
    State(state): State<AppState>,
    parts: Parts,
    body: Option<Json<CreateBody>>,
) -> Result<Response> {
    let directory = location(&parts, &state.options.default_directory)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let model_is_default = body.model.is_none();
    let model = body
        .model
        .or_else(|| state.services.default_model(&directory))
        .unwrap_or_default();
    let req = CreateSession {
        admission_authority: None,
        budget: body.budget,
        child_worktree_setup_pending: false,
        child_worktree: None,
        subagent_name: None,
        fork_from: None,
        output_schema: None,
        id: body.id,
        directory: directory.display().to_string(),
        model,
        model_is_default,
        agent: body.agent,
        mode: body.mode,
        parent_id: body.parent_id,
        title: body.title,
        rules: body.rules,
        max_steps: body.max_steps,
        worktree_id: None,
    };
    let info = state.runtime.create_session(req).await?;
    let data = session(&state, state.runtime.state(&info.id).await?);
    Ok((
        StatusCode::CREATED,
        Json(Located {
            location: LocationInfo::of(&directory),
            data,
        }),
    )
        .into_response())
}

async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Session>>> {
    Ok(Json(Data {
        data: session(&state, state.runtime.state(&id).await?),
    }))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<Data<Session>>> {
    if let Some(title) = body.title {
        state.runtime.rename(&id, &title).await?;
    }
    if let Some(archived) = body.archived {
        state.runtime.archive(&id, archived).await?;
    }
    show(State(state), Path(id)).await
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.runtime.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn fork(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ForkBody>>,
) -> Result<Response> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let info = state.runtime.fork(&id, body.message_id.as_deref()).await?;
    let data = session(&state, state.runtime.state(&info.id).await?);
    Ok((StatusCode::CREATED, Json(Data { data })).into_response())
}

impl SubtaskBody {
    fn request(self) -> crate::runtime::UserSubtask {
        crate::runtime::UserSubtask {
            skill_command: None,
            admission_id: None,
            prompt: self.prompt,
            agent: self.agent,
            attachments: self.attachments,
            max_steps: self.max_steps,
        }
    }
}
async fn subtask(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SubtaskBody>,
) -> Result<Response> {
    let job = state.runtime.subtask_request(&id, body.request()).await?;
    Ok((StatusCode::ACCEPTED, Json(Data { data: job })).into_response())
}
async fn start_delegation(
    State(state): State<AppState>,
    Path((id, request)): Path<(String, String)>,
    Json(body): Json<SubtaskBody>,
) -> Result<Response> {
    let data = state
        .runtime
        .start_delegation(&id, &request, body.request())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(Data { data })).into_response())
}
async fn delegation(
    State(state): State<AppState>,
    Path((id, request)): Path<(String, String)>,
) -> Result<Json<Data<crate::runtime::Delegation>>> {
    state.runtime.state(&id).await?;
    let data = state
        .runtime
        .delegation(&id, &request)?
        .ok_or_else(|| ApiError::not_found("RequestNotFoundError", "No delegation request"))?;
    Ok(Json(Data { data }))
}
async fn stop_delegation(
    State(state): State<AppState>,
    Path((id, request)): Path<(String, String)>,
) -> Result<Json<Data<crate::runtime::Delegation>>> {
    Ok(Json(Data {
        data: state.runtime.cancel_delegation(&id, &request).await?,
    }))
}

async fn prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Response> {
    if body.parts.is_empty() {
        return Err(ApiError::invalid("parts must not be empty"));
    }
    let admission = Admission {
        skill_command: None,
        message_id: body.id,
        parts: body.parts,
        delivery: body.delivery,
        source: body.source.unwrap_or_else(|| "user".into()),
        resume: body.resume,
    };
    let receipt: Receipt = state.runtime.admit_user(&id, admission).await?;
    Ok((StatusCode::ACCEPTED, Json(Data { data: receipt })).into_response())
}

async fn wake(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.runtime.wake(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Run a skill or custom command: its expanded template is admitted as the prompt.
async fn command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<CommandBody>,
) -> Result<Response> {
    let info = state.runtime.state(&id).await?.info;
    let turn = crate::runtime::TurnContext {
        session_id: info.id,
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = state
        .services
        .command_plan(&turn, &b.name, &b.arguments)
        .await
        .map_err(ApiError::forbidden)?
        .ok_or_else(|| {
            ApiError::not_found(
                "CommandNotFoundError",
                format!("no command or skill named {}", b.name),
            )
        })?;
    let admission = plan.admission(b.id, b.delivery);
    let receipt = state.runtime.admit_user(&id, admission).await?;
    Ok((StatusCode::ACCEPTED, Json(Data { data: receipt })).into_response())
}

async fn approve_auto(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    let receipt = state.runtime.approve_auto(&id).await?;
    Ok((StatusCode::ACCEPTED, Json(Data { data: receipt })).into_response())
}

async fn stop_subtree(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<crate::runtime::SubtreeStopReport>>> {
    let report = state.runtime.stop_subtree(&id).await?;
    Ok(Json(Data { data: report }))
}

async fn reopen_subtree(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(review): Json<ReopenSubtreeBody>,
) -> Result<Json<Data<crate::runtime::SubtreeReopenReport>>> {
    let report = state
        .runtime
        .reopen_subtree(&id, &review.scope_id, &review.stop_receipt_id)
        .await?;
    Ok(Json(Data { data: report }))
}

async fn interrupt(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.runtime.interrupt(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<AgentBody>,
) -> Result<Json<Data<Session>>> {
    state.runtime.switch_agent(&id, &b.agent).await?;
    show(State(state), Path(id)).await
}

async fn model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ModelBody>,
) -> Result<Json<Data<Session>>> {
    state.runtime.switch_model(&id, &b.model).await?;
    show(State(state), Path(id)).await
}

async fn mode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ModeBody>,
) -> Result<Json<Data<Session>>> {
    const MODES: [&str; 6] = [
        "default",
        "accept-edits",
        "plan",
        "auto",
        "dont-ask",
        "bypass",
    ];
    if !MODES.contains(&b.mode.as_str()) {
        return Err(ApiError::invalid(format!(
            "unknown mode {}; use one of {}",
            b.mode,
            MODES.join(", ")
        )));
    }
    state.runtime.switch_mode(&id, &b.mode).await?;
    show(State(state), Path(id)).await
}

async fn compact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CompactBody>>,
) -> Result<StatusCode> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    state.runtime.compact(&id, body.instructions).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn rewind(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<RevertBody>,
) -> Result<Json<Data<RevertState>>> {
    let staged = state
        .runtime
        .revert_stage(&id, &b.message_id, b.target)
        .await?;
    state.runtime.revert_commit(&id).await?;
    Ok(Json(Data { data: staged }))
}

async fn stage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<RevertBody>,
) -> Result<Json<Data<RevertState>>> {
    Ok(Json(Data {
        data: state
            .runtime
            .revert_stage(&id, &b.message_id, b.target)
            .await?,
    }))
}

async fn clear(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.runtime.revert_clear(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn commit(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.runtime.revert_commit(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn shell(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ShellBody>,
) -> Result<Json<Data<ShellResult>>> {
    let output = state.runtime.shell(&id, &b.command).await?;
    Ok(Json(Data {
        data: ShellResult { output },
    }))
}

fn message_of(state: &SessionState, entry: &Entry) -> Message {
    let tools = match entry {
        Entry::Assistant(a) => a
            .calls
            .iter()
            .filter_map(|c| state.calls.get(c).cloned())
            .collect(),
        _ => Vec::new(),
    };
    Message {
        entry: entry.clone(),
        tools,
    }
}

async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<Page<Message>>> {
    let s = state.runtime.state(&id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    let start = match q.cursor.as_deref() {
        Some(c) => c.parse::<usize>().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "InvalidCursorError",
                "malformed cursor",
            )
        })?,
        None => 0,
    };
    let mut ordered: Vec<&Entry> = s.entries.iter().collect();
    if q.order.as_deref() != Some("asc") {
        ordered.reverse();
    }
    let data: Vec<Message> = ordered
        .iter()
        .skip(start)
        .take(limit)
        .map(|e| message_of(&s, e))
        .collect();
    let next = (start + limit < ordered.len()).then(|| (start + limit).to_string());
    let previous = (start > 0).then(|| start.saturating_sub(limit).to_string());
    Ok(Json(Page {
        data,
        cursor: Cursor { previous, next },
    }))
}

async fn message(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<Data<Message>>> {
    let s = state.runtime.state(&id).await?;
    let entry = s
        .entries
        .iter()
        .find(|e| e.id() == message_id)
        .ok_or_else(|| {
            ApiError::not_found("MessageNotFoundError", format!("no message {message_id}"))
        })?;
    Ok(Json(Data {
        data: message_of(&s, entry),
    }))
}

async fn inbox(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<InboxRow>>>> {
    Ok(Json(Data {
        data: state.runtime.state(&id).await?.inbox,
    }))
}

async fn edit_input(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(String, String)>,
    Json(b): Json<InboxEditBody>,
) -> Result<StatusCode> {
    state
        .runtime
        .edit_input(&id, &message_id, b.parts, b.delivery)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_input(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    state.runtime.remove_input(&id, &message_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn release(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(String, String)>,
    body: Option<Json<ReleaseBody>>,
) -> Result<StatusCode> {
    let delivery = body
        .and_then(|Json(b)| b.delivery)
        .unwrap_or(Delivery::Steer);
    state.runtime.release(&id, &message_id, delivery).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn context(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<SessionContext>>> {
    let s = state.runtime.state(&id).await?;
    let data = SessionContext {
        epoch: s.epoch.as_ref().map(|e| e.number),
        baseline: s.epoch.map(|e| e.baseline),
        summary: s.compacted.map(|c| c.summary),
        task: s.task,
    };
    Ok(Json(Data { data }))
}

async fn diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<Data<Vec<FileDiff>>>> {
    Ok(Json(Data {
        data: state.runtime.diff(&id, &q.message_id).await?,
    }))
}

fn owned_request(state: &AppState, session_id: &str, request_id: &str) -> Result<()> {
    let known = state
        .runtime
        .pending_requests(Some(session_id))
        .iter()
        .any(|r| r.id == request_id);
    if known {
        Ok(())
    } else {
        Err(ApiError::not_found(
            "RequestNotFoundError",
            format!("no pending request {request_id} in session {session_id}"),
        ))
    }
}

async fn reply_permission(
    State(state): State<AppState>,
    Path((id, request_id)): Path<(String, String)>,
    Json(b): Json<PermissionReplyBody>,
) -> Result<StatusCode> {
    owned_request(&state, &id, &request_id)?;
    let reply = match b.reply {
        ReplyKind::Once => PermissionReply::Once,
        ReplyKind::Always => PermissionReply::Always,
        ReplyKind::Reject => PermissionReply::Reject { message: b.message },
    };
    state.runtime.reply_permission(&request_id, reply).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn answer_question(
    State(state): State<AppState>,
    Path((id, request_id)): Path<(String, String)>,
    Json(b): Json<QuestionReplyBody>,
) -> Result<StatusCode> {
    owned_request(&state, &id, &request_id)?;
    let reply = match b.answers {
        Some(answers) => QuestionReply::Answers { answers },
        None => QuestionReply::Dismissed,
    };
    state.runtime.answer_question(&request_id, reply).await?;
    Ok(StatusCode::NO_CONTENT)
}
