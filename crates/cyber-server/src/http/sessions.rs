//! Session routes (`server-api` → Session and prompt routes).

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AppState;
use super::envelope::{Cursor, Data, Located, LocationInfo, Page, location};
use super::error::ApiError;
use super::prompt_parts;
pub use super::prompt_parts::PromptPart;
use crate::runtime::{
    Admission, CallState, CreateSession, Delivery, Entry, FileDiff, InboxRow, ListFilter,
    PermissionReply, QuestionReply, Receipt, RevertState, RevertTarget, RuntimeError, SessionInfo,
    SessionRow, SessionState, TaskState, Totals,
};

use crate::runtime::messaging::{InboundOverride, OverrideReason};
use cyber_core::config::InboundPolicy;

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
    /// The context window meter after the last finished step; null before the first
    /// step and after a compaction.
    pub context: Option<crate::runtime::ContextMeter>,
}

/// A history entry with the tool calls it made.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Message {
    #[serde(flatten)]
    pub entry: Entry,
    pub tools: Vec<CallState>,
    /// The previous message on this message's branch; null for the first message.
    pub parent_message_id: Option<String>,
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
    /// The background Job a `background: true` command started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<crate::runtime::Job>,
}

/// The foreground calls asked to move to the background (Ctrl+B).
#[derive(Debug, Serialize, JsonSchema)]
pub struct BackgroundResult {
    pub call_ids: Vec<String>,
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
    /// Session-level `messaging.inbound` override (`cyber exec` refuses messages with
    /// `{ "inbound": "refuse", "reason": "non_interactive" }`).
    pub messaging: Option<InboundOverride>,
    /// The Session's own output style; refused when it is invalid, unknown or held at
    /// the Location, or for a child Session.
    pub output_style: Option<String>,
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
    /// Set the Session's `messaging.inbound` override; `{ "inbound": null }` clears it.
    pub messaging: Option<MessagingPatch>,
}

/// The `messaging` of `PATCH /api/v1/sessions/:sessionID`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessagingPatch {
    /// The policy replacing the Location's default; null clears the override.
    pub inbound: Option<InboundPolicy>,
    /// Why the Session refuses (`non_interactive`); only with an `inbound` policy.
    #[serde(default)]
    pub reason: Option<OverrideReason>,
}

impl MessagingPatch {
    fn into_override(self) -> Result<Option<InboundOverride>> {
        match (self.inbound, self.reason) {
            (None, Some(_)) => Err(ApiError::invalid(
                "messaging.reason needs an inbound policy",
            )),
            (inbound, reason) => Ok(inbound.map(|inbound| InboundOverride { inbound, reason })),
        }
    }
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
    /// Text and media parts for the child, stored as the parent's prompt media are;
    /// `file` parts are refused.
    #[serde(default)]
    pub attachments: Vec<PromptPart>,
    /// Positive ceiling capped by the selected profile's subagent step limit.
    pub max_steps: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PromptBody {
    pub id: Option<String>,
    /// Text and media parts; at most 20 media parts.
    pub parts: Vec<PromptPart>,
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
    /// Start the command as a background Job of the Session (`& cmd`) instead of
    /// recording its output.
    #[serde(default)]
    pub background: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InboxEditBody {
    /// Text and media parts, stored as a prompt's are; `file` parts are refused.
    pub parts: Option<Vec<PromptPart>>,
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
    /// Only Sessions linked to this pull request number.
    pub pr: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MessagesQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub order: Option<String>,
    /// `all`: the messages of every branch, oldest first by creation, instead of the
    /// active path.
    pub branches: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DiffQuery {
    /// One user message's diff; without it, every file the Session changed.
    pub message_id: Option<String>,
}

pub fn routes() -> Router<AppState> {
    let s = "/sessions/{session_id}";
    Router::new()
        .route("/sessions", get(list).post(create))
        .route(s, get(show).patch(update).delete(remove))
        .route(&format!("{s}/fork"), post(fork))
        .route(
            &format!("{s}/subtask"),
            post(subtask).layer(DefaultBodyLimit::max(prompt_parts::PROMPT_BODY_LIMIT)),
        )
        .route(
            &format!("{s}/delegations/{{request_id}}"),
            get(delegation)
                .post(start_delegation)
                .layer(DefaultBodyLimit::max(prompt_parts::PROMPT_BODY_LIMIT)),
        )
        .route(
            &format!("{s}/delegations/{{request_id}}/stop"),
            post(stop_delegation),
        )
        .route(
            &format!("{s}/delegations/{{request_id}}/reconcile"),
            post(reconcile_delegation),
        )
        .route(
            &format!("{s}/prompt"),
            post(prompt).layer(DefaultBodyLimit::max(prompt_parts::PROMPT_BODY_LIMIT)),
        )
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
        .route(&format!("{s}/recap"), get(recap).post(generate_recap))
        .route(&format!("{s}/rewind"), post(rewind))
        .route(&format!("{s}/revert/stage"), post(stage))
        .route(&format!("{s}/revert/clear"), post(clear))
        .route(&format!("{s}/revert/commit"), post(commit))
        .route(&format!("{s}/shell"), post(shell))
        .route(&format!("{s}/background"), post(background))
        .route(&format!("{s}/messages"), get(messages))
        .route(&format!("{s}/messages/{{message_id}}"), get(message))
        .route(&format!("{s}/inbox"), get(inbox))
        .route(
            &format!("{s}/inbox/{{message_id}}"),
            axum::routing::patch(edit_input)
                .delete(remove_input)
                .layer(DefaultBodyLimit::max(prompt_parts::PROMPT_BODY_LIMIT)),
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
    let context = state.runtime.context_meter(&s);
    Session {
        context,
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
        pr: q.pr,
        ..ListFilter::default()
    };
    let page = list_page(&state, &filter)?;
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

/// One page of `GET /sessions` for any filter; a bad cursor is an `InvalidCursorError`.
pub(super) fn list_page(
    state: &AppState,
    filter: &ListFilter,
) -> Result<crate::runtime::SessionPage> {
    state.runtime.list(filter).map_err(|e| match e {
        RuntimeError::Invalid(m) => ApiError::new(StatusCode::BAD_REQUEST, "InvalidCursorError", m),
        other => other.into(),
    })
}

async fn create(
    State(state): State<AppState>,
    parts: Parts,
    body: Option<Json<CreateBody>>,
) -> Result<Response> {
    let directory = location(&parts, &state.options.default_directory)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    if let Some(style) = &body.output_style
        && body.parent_id.is_none()
    {
        super::output_styles::check_defined(&state, &directory, style)?;
    }
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
        workflow_run_id: None,
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
        source_directory: None,
        messaging: body.messaging,
        // Only the team runtime makes team members; a client cannot claim membership.
        team: None,
        output_style: body.output_style,
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

pub(crate) async fn show(
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
    if let Some(messaging) = body.messaging {
        state
            .runtime
            .set_inbound(&id, messaging.into_override()?)
            .await?;
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
    /// The request, its media stored as the parent Session's blobs (the child adopts
    /// them at admission); the blobs written, which a refused request removes.
    async fn request(
        self,
        state: &AppState,
        parent: &str,
    ) -> Result<(crate::runtime::UserSubtask, Vec<String>)> {
        let stored = if self.attachments.is_empty() {
            Default::default()
        } else {
            prompt_parts::stored(&state.runtime, parent, self.attachments).await?
        };
        let request = crate::runtime::UserSubtask {
            skill_command: None,
            admission_id: None,
            prompt: self.prompt,
            agent: self.agent,
            attachments: stored.parts,
            max_steps: self.max_steps,
            hook_context: None,
        };
        Ok((request, stored.created))
    }
}

/// `result`, removing the blobs a refused request wrote for Session `id`.
fn kept<T>(
    state: &AppState,
    id: &str,
    created: &[String],
    result: std::result::Result<T, RuntimeError>,
) -> Result<T> {
    result.map_err(|error| {
        state.runtime.discard_attachments(id, created);
        error.into()
    })
}

async fn subtask(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: std::result::Result<Json<SubtaskBody>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body.map_err(prompt_parts::rejected)?;
    let (request, created) = body.request(&state, &id).await?;
    let job = state.runtime.subtask_request(&id, request).await;
    let job = kept(&state, &id, &created, job)?;
    Ok((StatusCode::ACCEPTED, Json(Data { data: job })).into_response())
}
async fn start_delegation(
    State(state): State<AppState>,
    Path((id, request_id)): Path<(String, String)>,
    body: std::result::Result<Json<SubtaskBody>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body.map_err(prompt_parts::rejected)?;
    let (request, created) = body.request(&state, &id).await?;
    let data = state
        .runtime
        .start_delegation(&id, &request_id, request)
        .await;
    let data = kept(&state, &id, &created, data)?;
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
async fn reconcile_delegation(
    State(state): State<AppState>,
    Path((id, request)): Path<(String, String)>,
) -> Result<Json<Data<crate::runtime::Delegation>>> {
    state
        .runtime
        .delegation(&id, &request)?
        .ok_or_else(|| ApiError::not_found("RequestNotFoundError", "No delegation request"))?;
    Ok(Json(Data {
        data: state.runtime.reconcile_delegation(&id, &request).await?,
    }))
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
    body: std::result::Result<Json<PromptBody>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body.map_err(prompt_parts::rejected)?;
    submit_prompt(&state, &id, body).await
}

/// Store the prompt's media as references, then admit it; a refused admission removes
/// the blobs it wrote.
pub(crate) async fn submit_prompt(
    state: &AppState,
    id: &str,
    body: PromptBody,
) -> Result<Response> {
    let inputs = prompt_parts::inputs(body.parts)?;
    let stored = state.runtime.store_prompt_parts(id, inputs).await?;
    let admission = Admission {
        skill_command: None,
        template: None,
        message_id: body.id,
        parts: stored.parts,
        delivery: body.delivery,
        source: body.source.unwrap_or_else(|| "user".into()),
        resume: body.resume,
    };
    let receipt: Receipt = match state
        .runtime
        .admit_user_with_files(id, admission, stored.files)
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            state.runtime.discard_attachments(id, &stored.created);
            return Err(error.into());
        }
    };
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
        worktree: Default::default(),
    };
    if let Some(message) = &b.id {
        let request = crate::runtime::CommandRequest {
            name: b.name.clone(),
            arguments: b.arguments.clone(),
            delivery: b.delivery,
        };
        if let Some(receipt) = state
            .runtime
            .command_receipt(&id, message, &request)
            .await?
        {
            return Ok((StatusCode::ACCEPTED, Json(Data { data: receipt })).into_response());
        }
    }
    let plan = state
        .services
        .command_plan(&turn, &b.name, &b.arguments)
        .await?
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

pub(crate) async fn interrupt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
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

/// `GET .../recap`: the latest Session recap, or `null`.
async fn recap(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Option<crate::runtime::Recap>>>> {
    Ok(Json(Data {
        data: state.runtime.recap(&id).await?,
    }))
}

/// `POST .../recap`: the stored recap when it covers the latest finished Turn, else a new
/// one from the `small` model. A model failure is `503` with `service: "model"`.
async fn generate_recap(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<crate::runtime::Recap>>> {
    match state.runtime.generate_recap(&id).await {
        Ok(data) => Ok(Json(Data { data })),
        Err(crate::runtime::RuntimeError::Conflict(reason))
            if reason == crate::runtime::NOTHING_TO_RECAP =>
        {
            Err(
                ApiError::conflict("The Session has no finished Turn to recap")
                    .with_reason(crate::runtime::NOTHING_TO_RECAP),
            )
        }
        Err(crate::runtime::RuntimeError::Model(message)) => {
            let mut error = ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                format!("The recap model failed: {message}"),
            );
            error.body.service = Some("model".into());
            Err(error)
        }
        Err(error) => Err(error.into()),
    }
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
    if b.background {
        let job = state.runtime.shell_background(&id, &b.command).await?;
        let output = format!(
            "Started background job {}; its log is {}",
            job.id,
            job.output_path.as_deref().unwrap_or("")
        );
        return Ok(Json(Data {
            data: ShellResult {
                output,
                job: Some(job),
            },
        }));
    }
    let output = state.runtime.shell(&id, &b.command).await?;
    Ok(Json(Data {
        data: ShellResult { output, job: None },
    }))
}

/// Move the Session's running foreground command to the background (Ctrl+B); the call
/// settles with the new Job's ID.
async fn background(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<BackgroundResult>>> {
    state.runtime.state(&id).await?;
    let call_ids = state.runtime.request_handoff(&id);
    if call_ids.is_empty() {
        return Err(ApiError::conflict(
            "No foreground command of this Session is running",
        ));
    }
    Ok(Json(Data {
        data: BackgroundResult { call_ids },
    }))
}

pub(super) fn message_of(state: &SessionState, entry: &Entry) -> Message {
    let tree = state.branches();
    let tools = match entry {
        Entry::Assistant(a) => a
            .calls
            .iter()
            .filter_map(|c| {
                let stashed = || tree.stashed_call(&a.id, c);
                state.calls.get(c).or_else(stashed)
            })
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    Message {
        entry: entry.clone(),
        tools,
        parent_message_id: tree.parent(entry.id()).map(str::to_owned),
    }
}

/// Whether a `branches` query asks for every branch: absent or `all`.
pub(super) fn all_branches(branches: Option<&str>) -> Result<bool> {
    match branches {
        None => Ok(false),
        Some("all") => Ok(true),
        Some(other) => Err(ApiError::invalid(format!(
            "branches must be \"all\", not {other:?}"
        ))),
    }
}

/// The active path, or with `all` every message in creation order.
pub(super) fn listed(s: &SessionState, all: bool) -> Vec<&Entry> {
    if !all {
        return s.entries.iter().collect();
    }
    let messages = s.messages_by_id();
    s.branches()
        .ordered()
        .into_iter()
        .filter_map(|id| messages.get(id).copied())
        .collect()
}

pub(crate) async fn messages(
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
    let mut ordered = listed(&s, all_branches(q.branches.as_deref())?);
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
    body: std::result::Result<Json<InboxEditBody>, JsonRejection>,
) -> Result<StatusCode> {
    let Json(b) = body.map_err(prompt_parts::rejected)?;
    let (parts, created) = match b.parts {
        Some(parts) => {
            let stored = prompt_parts::stored(&state.runtime, &id, parts).await?;
            (Some(stored.parts), stored.created)
        }
        None => (None, Vec::new()),
    };
    let edited = state
        .runtime
        .edit_input(&id, &message_id, parts, b.delivery)
        .await;
    kept(&state, &id, &created, edited)?;
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
    let data = match &q.message_id {
        Some(message) => state.runtime.diff(&id, message).await?,
        None => state.runtime.session_diff(&id).await?,
    };
    Ok(Json(Data { data }))
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

pub(crate) async fn reply_permission(
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
