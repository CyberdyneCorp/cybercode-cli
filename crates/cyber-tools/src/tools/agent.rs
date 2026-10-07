//! Permission-gated foreground child Sessions using the shared runtime.

use cyber_core::config::AgentProfile;
use cyber_server::runtime::{
    Admission, CallStatus, CreateSession, Delivery, Entry, RetrySafety, Runtime, SessionInfo,
    SessionState, StructuredSchema, ToolDef,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::permissions::{Decision, Request};
use crate::subagents::ChildGuard;

pub(crate) struct Agent;

impl Tool for Agent {
    fn def(&self) -> ToolDef {
        def(
            "agent",
            "Delegate a focused task to a foreground subagent and return its final answer and Session ID.",
            json!({"type":"object", "required":["prompt"], "additionalProperties":false, "properties":{
                "prompt":{"type":"string"}, "agent":{"type":"string"}, "description":{"type":"string"},
                "model":{"type":"string"}, "output_schema":{}, "isolation":{"type":"string", "enum":["none","worktree"]},
                "background":{"type":"boolean"}, "fork":{"type":"boolean"}, "resume":{"type":"string"}, "name":{"type":"string"}
            }}),
            RetrySafety::Never,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(run(ctx))
    }
}

struct Spawn {
    output_schema: Option<StructuredSchema>,
    profile: AgentProfile,
    description: String,
    result_bytes: usize,
    max_depth: u64,
    max_concurrent: usize,
}

fn configured(ctx: &Ctx<'_>) -> Result<Spawn, ToolError> {
    let input = &ctx.inv.input;
    if text(input, "prompt").trim().is_empty() {
        return Err(failed("prompt is required"));
    }
    let name = input
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or("general");
    let (config, _) = (ctx.host.opts.config)(&ctx.location).map_err(failed)?;
    let profiles = cyber_core::config::resolve_agents(&config).map_err(failed)?;
    let available = profiles
        .values()
        .filter(|p| p.subagent_capable())
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let profile = profiles
        .get(name)
        .filter(|p| !p.hidden)
        .cloned()
        .ok_or_else(|| failed(format!("Unknown agent {name:?}. Available: {available}")))?;
    if !profile.subagent_capable() {
        return Err(failed(format!("Agent {name:?} cannot run a subagent")));
    }
    check_features(input, &profile)?;
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("Run focused subagent task");
    if !(3..=8).contains(&description.split_whitespace().count()) {
        return Err(failed("description must contain 3–8 words"));
    }
    let output_schema = input
        .get("output_schema")
        .cloned()
        .map(StructuredSchema::new)
        .transpose()
        .map_err(failed)?;
    Ok(Spawn {
        output_schema,
        profile,
        description: description.into(),
        result_bytes: setting(&config, "result_max_bytes", 16384)? as usize,
        max_depth: setting(&config, "max_depth", 2)?,
        max_concurrent: setting(&config, "max_concurrent", 8)? as usize,
    })
}

fn setting(config: &Value, key: &str, fallback: u64) -> Result<u64, ToolError> {
    let value = config
        .get("agents")
        .and_then(|v| v.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(fallback);
    if value > usize::MAX as u64
        || (key == "max_concurrent" && value > tokio::sync::Semaphore::MAX_PERMITS as u64)
    {
        return Err(failed(format!("agents.{key} exceeds the platform limit")));
    }
    Ok(value)
}

fn check_features(input: &Value, profile: &AgentProfile) -> Result<(), ToolError> {
    let isolation = input
        .get("isolation")
        .and_then(Value::as_str)
        .or(profile.isolation.as_deref())
        .unwrap_or("none");
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(profile.background);
    if isolation != "none"
        || background
        || input.get("fork").and_then(Value::as_bool).unwrap_or(false)
        || input.get("resume").is_some()
        || input.get("name").is_some()
    {
        return Err(failed(
            "This agent execution path currently supports fresh foreground Sessions with isolation none; requested orchestration is not implemented yet",
        ));
    }
    Ok(())
}

async fn run(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let mut spawn = configured(ctx)?;
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Agent tool requires the runtime"))?;
    let parent = runtime
        .state(&ctx.inv.session_id)
        .await
        .map_err(|e| failed(e.to_string()))?
        .info;
    let ancestors = runtime
        .ancestors(&parent)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let root = ancestors.last().unwrap_or(&parent);
    let (root_config, _) =
        (ctx.host.opts.config)(std::path::Path::new(&root.directory)).map_err(failed)?;
    cyber_core::config::resolve_agents(&root_config).map_err(failed)?;
    spawn.max_depth = spawn.max_depth.min(setting(&root_config, "max_depth", 2)?);
    spawn.max_concurrent = spawn
        .max_concurrent
        .min(setting(&root_config, "max_concurrent", 8)? as usize);
    if ancestors.len() as u64 >= spawn.max_depth {
        return Err(failed(format!(
            "Subagent depth limit reached ({}). Increase agents.max_depth to allow deeper nesting.",
            spawn.max_depth
        )));
    }
    ctx.authorize(
        Request {
            action: "agent".into(),
            resources: vec![spawn.profile.name.clone()],
            ..Request::default()
        },
        vec![spawn.profile.name.clone()],
        json!({"agent":spawn.profile.name, "description":spawn.description}),
    )
    .await?;
    let pool = ctx
        .host
        .subagents
        .pool(&root.id, spawn.max_concurrent)
        .map_err(failed)?;
    let permit = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
        permit = pool.acquire_owned() => permit.map_err(|e| failed(e.to_string()))?,
    };
    // Queued or approved calls may outlive a profile/configuration reload.
    spawn.profile = configured(ctx)?.profile;
    let policy = ctx.host.policy(ctx.inv).await.map_err(failed)?;
    let request = Request {
        action: "agent".into(),
        resources: vec![spawn.profile.name.clone()],
        ..Request::default()
    };
    if let Decision::Deny(reason) = policy.decide(&request) {
        return Err(failed(format!("Permission denied: {reason}")));
    }
    execute_child(ctx, runtime, parent, spawn, permit).await
}

fn child_mode(parent: &str, requested: Option<&str>) -> String {
    let mode = requested.unwrap_or(parent);
    match (parent, mode) {
        (_, "plan") | ("bypass", _) => mode,
        ("plan" | "dont-ask", _) | (_, "bypass") => parent,
        (a, b) if a == b => parent,
        _ => "default",
    }
    .into()
}

async fn execute_child(
    ctx: &Ctx<'_>,
    runtime: Runtime,
    parent: SessionInfo,
    spawn: Spawn,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<String, ToolError> {
    let id = cyber_core::ids::new_id("ses");
    let guard = ChildGuard::new(runtime.clone(), id.clone(), permit);
    let result = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err(ToolError::Aborted),
        result = create_and_wait(ctx, &runtime, &parent, &spawn, &id) => result,
    };
    guard.settle(result.is_err()).await;
    result
}

async fn create_and_wait(
    ctx: &Ctx<'_>,
    runtime: &Runtime,
    parent: &SessionInfo,
    spawn: &Spawn,
    id: &str,
) -> Result<String, ToolError> {
    let model = ctx.inv.input.get("model").and_then(Value::as_str);
    runtime
        .create_session(CreateSession {
            output_schema: spawn.output_schema.clone(),
            id: Some(id.into()),
            directory: parent.directory.clone(),
            worktree_id: parent.worktree_id.clone(),
            parent_id: Some(parent.id.clone()),
            agent: Some(spawn.profile.name.clone()),
            title: Some(format!("{} (@{})", spawn.description, spawn.profile.name)),
            model: model.unwrap_or(&parent.model).into(),
            model_is_default: model.is_none(),
            mode: Some(child_mode(
                &ctx.inv.mode,
                spawn.profile.permission_mode.as_deref(),
            )),
            ..Default::default()
        })
        .await
        .map_err(|e| failed(e.to_string()))?;
    let mut admission = Admission::text(text(&ctx.inv.input, "prompt"), Delivery::Queue);
    admission.source = "session".into();
    runtime
        .admit(id, admission)
        .await
        .map_err(|e| failed(e.to_string()))?;
    runtime.wait_idle(id).await;
    if spawn.output_schema.is_some() {
        return structured_result(runtime, id).await;
    }
    let state = runtime.state(id).await.map_err(|e| failed(e.to_string()))?;
    let answer = state
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            Entry::Assistant(answer) => Some(answer),
            _ => None,
        })
        .ok_or_else(|| failed("Subagent stopped without a completed answer"))?;
    if let Some(error) = &answer.error {
        return Err(failed(format!("Subagent failed: {error}")));
    }
    if !answer.finished || !answer.calls.is_empty() {
        return Err(failed("Subagent stopped without a completed answer"));
    }
    render_result(ctx, spawn, id, &answer.text)
}

fn render_result(
    ctx: &Ctx<'_>,
    spawn: &Spawn,
    id: &str,
    answer: &str,
) -> Result<String, ToolError> {
    let budget = crate::Budget {
        max_bytes: spawn.result_bytes,
        max_lines: usize::MAX,
        dir: ctx.host.opts.tool_output_dir.clone(),
    };
    let mut result = json!({"id":id,"text":answer});
    if answer.len() > spawn.result_bytes {
        let path = budget.store(answer).map_err(failed)?;
        let mut end = spawn.result_bytes;
        while !answer.is_char_boundary(end) {
            end -= 1;
        }
        result["text"] = json!(&answer[..end]);
        result["output_file"] = json!(path);
    }
    Ok(result.to_string())
}

async fn structured_result(runtime: &Runtime, id: &str) -> Result<String, ToolError> {
    let state = runtime.state(id).await.map_err(|e| failed(e.to_string()))?;
    if let Some(value) = state.structured_result() {
        return Ok(json!({"id":id,"result":value}).to_string());
    }
    ensure_child_finished(&state)?;
    let errors = state.structured_result_error();
    let mut admission = Admission::text(
        format!(
            "Your structured result is missing or invalid: {errors}. Call return_result with a value matching its schema. This is your single result retry."
        ),
        Delivery::Queue,
    );
    admission.source = "session".into();
    runtime
        .admit(id, admission)
        .await
        .map_err(|e| failed(e.to_string()))?;
    runtime.wait_idle(id).await;
    let state = runtime.state(id).await.map_err(|e| failed(e.to_string()))?;
    match state.structured_result() {
        Some(value) => Ok(json!({"id":id,"result":value}).to_string()),
        None => {
            ensure_child_finished(&state)?;
            Err(failed(format!(
                "SchemaMismatch: {}",
                state.structured_result_error()
            )))
        }
    }
}

fn ensure_child_finished(state: &SessionState) -> Result<(), ToolError> {
    let answer = state
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            Entry::Assistant(answer) => Some(answer),
            _ => None,
        })
        .ok_or_else(|| failed("Subagent stopped before completing a result attempt"))?;
    if !answer.finished || answer.error.is_some() || state.structured_attempt_rejected() {
        return Err(failed(
            "Subagent stopped before completing a result attempt",
        ));
    }
    let stopped = answer
        .calls
        .iter()
        .filter_map(|id| state.calls.get(id))
        .any(|call| {
            matches!(
                call.status,
                CallStatus::Interrupted | CallStatus::OutcomeUnknown
            )
        });
    if stopped {
        return Err(failed(
            "Subagent stopped before completing a result attempt",
        ));
    }
    Ok(())
}
