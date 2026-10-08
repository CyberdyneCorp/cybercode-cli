//! Permission-gated foreground and background child Sessions using the shared runtime.

use cyber_core::config::AgentProfile;
use cyber_server::runtime::{
    Admission, CallStatus, CreateSession, Delivery, Entry, RetrySafety, Runtime, SessionInfo,
    SessionState, StructuredSchema, ToolDef, WeakRuntime,
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
            "Delegate a focused task to a subagent; wait for its final result or start a background task with completion handback. Resume an existing child by name or Session ID. Set fork true to inherit the caller history, agent and model unless overridden.",
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
        Box::pin(run(ctx, false))
    }
}

struct Spawn {
    authority: Option<cyber_server::runtime::AdmissionAuthority>,
    attachments: Vec<cyber_llm::Content>,
    max_steps: Option<u32>,
    isolation: bool,
    user_requested: bool,
    fork: bool,
    resume: Option<SessionInfo>,
    usage: cyber_server::runtime::JobUsage,
    background: bool,
    name: Option<String>,
    output_schema: Option<StructuredSchema>,
    profile: AgentProfile,
    description: String,
    result_bytes: usize,
    max_depth: u64,
    max_concurrent: usize,
}

fn configured(ctx: &Ctx<'_>, resume: Option<&SessionInfo>) -> Result<Spawn, ToolError> {
    let input = &ctx.inv.input;
    if text(input, "prompt").trim().is_empty() {
        return Err(failed("prompt is required"));
    }
    let fork = input.get("fork").and_then(Value::as_bool).unwrap_or(false);
    if fork && resume.is_some() {
        return Err(failed("fork and resume cannot be combined"));
    }
    let name = input
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            resume.map_or(if fork { &ctx.inv.agent } else { "general" }, |info| {
                info.agent.as_str()
            })
        });
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
    if !profile.subagent_capable()
        && !(resume.is_some() && profile.primary_capable())
        && !(fork && profile.name == ctx.inv.agent && profile.primary_capable())
    {
        return Err(failed(format!("Agent {name:?} cannot run a subagent")));
    }
    if let Some(existing) = resume {
        if profile.name != existing.agent {
            return Err(failed("Resume cannot change the child agent"));
        }
        if input
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| model != existing.model)
        {
            return Err(failed(
                "Resume cannot change the child model; select it on the child Session first",
            ));
        }
    }
    let isolation = check_features(input, &profile)?;
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
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(profile.background);
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    if name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 128)
    {
        return Err(failed("name must contain 1–128 bytes"));
    }
    Ok(Spawn {
        authority: None,
        attachments: Vec::new(),
        max_steps: None,
        isolation,
        user_requested: false,
        fork,
        resume: resume.cloned(),
        usage: Default::default(),
        background,
        name,
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

fn check_features(input: &Value, profile: &AgentProfile) -> Result<bool, ToolError> {
    let isolation = input
        .get("isolation")
        .and_then(Value::as_str)
        .or(profile.isolation.as_deref())
        .unwrap_or("none");
    match isolation {
        "none" => Ok(false),
        "worktree" => Ok(true),
        _ => Err(failed("Unsupported subagent isolation")),
    }
}

fn user_options(spawn: &mut Spawn, input: &Value) -> Result<(), ToolError> {
    if spawn.user_requested {
        spawn.attachments = input
            .get("attachments")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| failed(error.to_string()))?
            .unwrap_or_default();
        spawn.max_steps = input
            .get("max_steps")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|limit| u32::try_from(limit).ok())
                    .filter(|limit| *limit > 0)
                    .ok_or_else(|| failed("Subtask max_steps must be a positive integer"))
            })
            .transpose()?;
    }
    Ok(())
}

pub(crate) async fn run(ctx: &Ctx<'_>, user_requested: bool) -> Result<String, ToolError> {
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Agent tool requires the runtime"))?;
    let authority = runtime
        .capture_child_admission(&ctx.inv.session_id)
        .map_err(|e| failed(e.to_string()))?;
    let resume = match ctx.inv.input.get("resume").and_then(Value::as_str) {
        Some(reference) => Some(
            runtime
                .resolve_subagent(&ctx.inv.session_id, reference)
                .await
                .map_err(|e| failed(e.to_string()))?,
        ),
        None => None,
    };
    let mut spawn = configured(ctx, resume.as_ref())?;
    spawn.authority = Some(authority);
    spawn.user_requested = user_requested;
    user_options(&mut spawn, &ctx.inv.input)?;
    if let Some(existing) = &resume {
        let state = runtime
            .state(&existing.id)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let isolated = state.child_worktree().is_some();
        if ctx
            .inv
            .input
            .get("isolation")
            .and_then(Value::as_str)
            .is_some_and(|choice| (choice == "worktree") != isolated)
        {
            return Err(failed("Resume cannot change child isolation"));
        }
        spawn.isolation = isolated;
    }
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
    let request = Request {
        action: "agent".into(),
        resources: vec![spawn.profile.name.clone()],
        ..Request::default()
    };
    if user_requested {
        if let Decision::Deny(reason) = ctx.policy.user_delegation(&request) {
            return Err(failed(format!("Permission denied: {reason}")));
        }
    } else {
        ctx.authorize(
            request,
            vec![spawn.profile.name.clone()],
            json!({"agent":spawn.profile.name, "description":spawn.description}),
        )
        .await?;
    }
    let execution = if let Some(child) = &spawn.resume {
        let owner = runtime
            .claim_child_execution(&parent.id, &child.id)
            .map_err(|e| failed(e.to_string()))?;
        if runtime.is_running(&child.id)
            || runtime
                .jobs(Some(&parent.id))
                .map_err(|e| failed(e.to_string()))?
                .iter()
                .any(|job| {
                    job.child_id == child.id
                        && job.status == cyber_server::runtime::JobStatus::Running
                })
        {
            return Err(failed("Subagent busy"));
        }
        Some(owner)
    } else {
        None
    };
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
    prepare_child_admission(ctx, &runtime, &mut spawn).await?;
    let authority = spawn
        .authority
        .clone()
        .expect("captured admission authority");
    authority
        .run(
            ctx.cancel.clone(),
            execute_child(ctx, runtime, parent, spawn, permit, execution),
        )
        .await
}

async fn prepare_child_admission(
    ctx: &Ctx<'_>,
    runtime: &Runtime,
    spawn: &mut Spawn,
) -> Result<(), ToolError> {
    spawn
        .authority
        .as_ref()
        .expect("captured admission authority")
        .verify(runtime, &ctx.inv.session_id)
        .map_err(|e| failed(e.to_string()))?;
    revalidate_spawn(ctx, spawn).await?;
    if spawn.user_requested {
        runtime
            .mark_delegation_launching(&ctx.inv.session_id, &ctx.inv.operation_key)
            .await
            .map_err(|e| failed(e.to_string()))?;
    }
    Ok(())
}

async fn revalidate_spawn(ctx: &Ctx<'_>, spawn: &mut Spawn) -> Result<(), ToolError> {
    // Queued or approved calls may outlive a profile/configuration reload.
    ctx.host.check_agent_tool(ctx.inv).map_err(failed)?;
    let latest = configured(ctx, spawn.resume.as_ref())?;
    spawn.profile = latest.profile;
    if spawn.resume.is_none() {
        spawn.isolation = latest.isolation;
    }
    spawn.background = latest.background;
    spawn.name = latest.name;
    let policy = ctx.host.policy(ctx.inv).await.map_err(failed)?;
    let request = Request {
        action: "agent".into(),
        resources: vec![spawn.profile.name.clone()],
        ..Request::default()
    };
    let decision = if spawn.user_requested {
        policy.user_delegation(&request)
    } else {
        policy.decide(&request)
    };
    if let Decision::Deny(reason) = decision {
        return Err(failed(format!("Permission denied: {reason}")));
    }
    Ok(())
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
    execution: Option<cyber_server::runtime::ChildExecution>,
) -> Result<String, ToolError> {
    let mut spawn = spawn;
    let reservation = reserve_child(ctx, &runtime, &parent, &mut spawn).await?;
    let id = spawn
        .resume
        .as_ref()
        .map(|info| info.id.clone())
        .unwrap_or_else(|| cyber_core::ids::new_id("ses"));
    let execution = match execution {
        Some(execution) => execution,
        None => runtime
            .claim_child_execution(&parent.id, &id)
            .map_err(|e| failed(e.to_string()))?,
    };
    let guard = ChildGuard::new(runtime.clone(), id.clone(), permit, execution);
    let result = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err(ToolError::Aborted),
        result = guard.execution().run(ctx.cancel.clone(), create_child(ctx, &runtime, &parent, &spawn, &id, guard.execution())) => result,
    };
    let (prompt, worktree) = match result {
        Ok(created) => created,
        Err(error) => {
            guard.settle(spawn.resume.is_none()).await;
            return Err(error);
        }
    };
    let weak = runtime.downgrade();
    let bytes = spawn.result_bytes;
    let dir = ctx.host.opts.tool_output_dir.clone();
    let structured = spawn.output_schema.is_some();
    if spawn.background {
        let run = ChildRun {
            worktree,
            weak,
            id,
            prompt,
            structured,
            bytes,
            dir,
        };
        return handoff_background(&runtime, &parent, spawn, reservation, guard, run).await;
    }
    drop(reservation);
    let result = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err(ToolError::Aborted),
        result = finish_child(&weak, &id, &prompt, structured, bytes, dir) => result,
    };
    let result = attach_worktree_result(result, worktree.as_ref(), ctx.cancel.clone()).await;
    guard
        .settle_foreground(&runtime, &id, result.is_err())
        .await;
    result
}

async fn attach_worktree_result(
    result: Result<String, ToolError>,
    worktree: Option<&crate::worktrees::ChildWorktree>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<String, ToolError> {
    let Some(worktree) = worktree else {
        return result;
    };
    let text = result?;
    let mut output: Value = serde_json::from_str(&text).map_err(|e| failed(e.to_string()))?;
    output["worktree"] = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ToolError::Aborted),
        report = worktree.report(cancel.clone()) => report,
    };
    Ok(output.to_string())
}

struct ChildRun {
    worktree: Option<crate::worktrees::ChildWorktree>,
    weak: WeakRuntime,
    id: String,
    prompt: String,
    structured: bool,
    bytes: usize,
    dir: std::path::PathBuf,
}

async fn reserve_child(
    ctx: &Ctx<'_>,
    runtime: &Runtime,
    parent: &SessionInfo,
    spawn: &mut Spawn,
) -> Result<Option<cyber_server::runtime::JobAdmission>, ToolError> {
    let reservation = tokio::select! { biased; _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted), result=runtime.reserve_child_job(&parent.id) => result.map_err(|e| failed(e.to_string()))? };
    revalidate_spawn(ctx, spawn).await?;
    if spawn.fork && spawn.output_schema.is_none() {
        spawn.output_schema = runtime
            .state(&parent.id)
            .await
            .map_err(|e| failed(e.to_string()))?
            .output_schema()
            .cloned();
    }
    if let Some(existing) = &spawn.resume {
        let state = runtime
            .state(&existing.id)
            .await
            .map_err(|e| failed(e.to_string()))?;
        if state.info.parent_id.as_deref() != Some(&parent.id)
            || state.info.agent != spawn.profile.name
        {
            return Err(failed("Resumed child identity changed; retry"));
        }
        spawn.usage = runtime
            .job_usage(&existing.id)
            .map_err(|e| failed(e.to_string()))?;
        if ctx
            .inv
            .input
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| model != state.info.model)
        {
            return Err(failed(
                "Resume cannot change the child model; select it on the child Session first",
            ));
        }
        let old_name = match state.info.subagent_name.clone() {
            Some(name) => Some(name),
            None => runtime
                .jobs(Some(&parent.id))
                .map_err(|e| failed(e.to_string()))?
                .into_iter()
                .find(|job| job.child_id == existing.id)
                .map(|job| job.name),
        };
        if let Some(name) = old_name {
            if spawn
                .name
                .as_ref()
                .is_some_and(|requested| requested != &name)
            {
                return Err(failed("Resume cannot change the child name"));
            }
            spawn.name = Some(name);
            if spawn.output_schema.is_none() {
                spawn.output_schema = state.output_schema().cloned();
            }
            return Ok(Some(reservation));
        }
        if spawn.output_schema.is_none() {
            spawn.output_schema = state.output_schema().cloned();
        }
    }
    let used = runtime
        .subagent_names(&parent.id)
        .map_err(|e| failed(e.to_string()))?;
    if let Some(name) = &spawn.name {
        let jobs = runtime
            .jobs(Some(&parent.id))
            .map_err(|e| failed(e.to_string()))?;
        if used.contains(name) && !jobs.iter().any(|job| job.name == *name) {
            return Err(failed(format!("Subagent name already used: {name}")));
        }
        if let Some(existing) = jobs.iter().find(|job| job.name == *name) {
            let status = if existing.status == cyber_server::runtime::JobStatus::Running {
                "active"
            } else {
                "used"
            };
            return Err(failed(format!("Background name already {status}: {name}")));
        }
    } else {
        let used = used.into_iter().collect::<std::collections::BTreeSet<_>>();
        let mut name = spawn.profile.name.clone();
        let mut suffix = 2;
        while used.contains(&name) {
            name = format!("{}-{suffix}", spawn.profile.name);
            suffix += 1;
        }
        spawn.name = Some(name);
    }
    Ok(Some(reservation))
}

async fn handoff_background(
    runtime: &Runtime,
    parent: &SessionInfo,
    spawn: Spawn,
    reservation: Option<cyber_server::runtime::JobAdmission>,
    guard: ChildGuard,
    run: ChildRun,
) -> Result<String, ToolError> {
    let id = run.id.clone();
    let work = Box::pin(async move {
        let result = finish_child(
            &run.weak,
            &run.id,
            &run.prompt,
            run.structured,
            run.bytes,
            run.dir,
        )
        .await;
        let result = attach_worktree_result(
            result,
            run.worktree.as_ref(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        guard.settle(result.is_err()).await;
        result
            .and_then(|text| serde_json::from_str(&text).map_err(|e| failed(e.to_string())))
            .map_err(|e| match e {
                ToolError::Failed(text) => text,
                ToolError::Aborted => "Subagent interrupted".into(),
            })
    });
    let name = spawn.name.unwrap_or(spawn.profile.name);
    let job = runtime
        .start_child_job_attempt(
            &parent.id,
            &id,
            cyber_server::runtime::JobAttempt {
                name,
                description: spawn.description,
                usage: spawn.usage,
            },
            reservation,
            work,
        )
        .await
        .map_err(|e| failed(e.to_string()))?;
    Ok(json!({"id":id,"job_id":job.id,"name":job.name,"state":"running"}).to_string())
}

async fn create_child(
    ctx: &Ctx<'_>,
    runtime: &Runtime,
    parent: &SessionInfo,
    spawn: &Spawn,
    id: &str,
    execution: &cyber_server::runtime::ChildExecution,
) -> Result<(String, Option<crate::worktrees::ChildWorktree>), ToolError> {
    spawn
        .authority
        .as_ref()
        .expect("captured admission authority")
        .verify(runtime, &parent.id)
        .map_err(|e| failed(e.to_string()))?;
    if let Some(existing) = &spawn.resume {
        let state = runtime
            .state(&existing.id)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let worktree = match state.child_worktree().cloned() {
            Some(managed) => Some(
                ctx.host
                    .resume_child_worktree(
                        ctx.inv,
                        ctx.cancel.clone(),
                        execution,
                        existing,
                        managed,
                    )
                    .await
                    .map_err(|e| failed(e.to_string()))?,
            ),
            None => None,
        };
        let mut admission = Admission::text(text(&ctx.inv.input, "prompt"), Delivery::Queue);
        admission.source = "session".into();
        admission.parts.extend(spawn.attachments.clone());
        let receipt = runtime
            .resume_child(
                execution,
                admission,
                spawn.name.clone().expect("reserved name"),
                spawn.output_schema.clone(),
            )
            .await
            .map_err(|e| failed(e.to_string()))?;
        return Ok((receipt.message_id, worktree));
    }
    let model = ctx.inv.input.get("model").and_then(Value::as_str);
    let request = CreateSession {
        admission_authority: spawn.authority.clone(),
        output_schema: spawn.output_schema.clone(),
        fork_from: spawn.fork.then(|| parent.id.clone()),
        id: Some(id.into()),
        directory: parent.directory.clone(),
        worktree_id: parent.worktree_id.clone(),
        parent_id: Some(parent.id.clone()),
        agent: Some(spawn.profile.name.clone()),
        max_steps: spawn.max_steps.map(|limit| {
            limit.min(u32::try_from(spawn.profile.steps.unwrap_or(50)).unwrap_or(u32::MAX))
        }),
        subagent_name: spawn.name.clone(),
        title: Some(format!("{} (@{})", spawn.description, spawn.profile.name)),
        model: model.unwrap_or(&parent.model).into(),
        model_is_default: model.is_none() && !spawn.fork,
        mode: Some(child_mode(
            &ctx.inv.mode,
            spawn.profile.permission_mode.as_deref(),
        )),
        ..Default::default()
    };
    let worktree = if spawn.isolation {
        Some(
            ctx.host
                .create_child_worktree(ctx.inv, ctx.cancel.clone(), request, spawn.user_requested)
                .await
                .map_err(|e| failed(e.to_string()))?,
        )
    } else {
        runtime
            .create_session(request)
            .await
            .map_err(|e| failed(e.to_string()))?;
        None
    };
    let mut admission = Admission::text(text(&ctx.inv.input, "prompt"), Delivery::Queue);
    admission.source = "session".into();
    admission.parts.extend(spawn.attachments.clone());
    let receipt = runtime
        .admit(id, admission)
        .await
        .map_err(|e| failed(e.to_string()))?;
    Ok((receipt.message_id, worktree))
}

async fn finish_child(
    weak: &WeakRuntime,
    id: &str,
    prompt: &str,
    structured: bool,
    bytes: usize,
    dir: std::path::PathBuf,
) -> Result<String, ToolError> {
    weak.wait_idle(id)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let state = weak
        .upgrade()
        .ok_or_else(|| failed("Server stopped"))?
        .state(id)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let start = state
        .entries
        .iter()
        .position(|entry| matches!(entry, Entry::User { id, .. } if id == prompt))
        .ok_or_else(|| failed("Subagent prompt was not promoted; preparation failed"))?;
    let answer = state
        .entries
        .iter()
        .skip(start + 1)
        .rev()
        .find_map(|entry| match entry {
            Entry::Assistant(answer) => Some(answer),
            _ => None,
        })
        .ok_or_else(|| failed("Subagent stopped without a completed answer"))?;
    if structured {
        return structured_result(weak, id).await;
    }
    if let Some(error) = &answer.error {
        return Err(failed(format!("Subagent failed: {error}")));
    }
    if !answer.finished || !answer.calls.is_empty() {
        return Err(failed("Subagent stopped without a completed answer"));
    }
    render_result(
        bytes,
        dir,
        id,
        state.info.subagent_name.as_deref(),
        &answer.text,
    )
}

fn render_result(
    bytes: usize,
    dir: std::path::PathBuf,
    id: &str,
    name: Option<&str>,
    answer: &str,
) -> Result<String, ToolError> {
    let budget = crate::Budget {
        max_bytes: bytes,
        max_lines: usize::MAX,
        dir,
    };
    let mut result = json!({"id":id,"text":answer});
    if let Some(name) = name {
        result["name"] = json!(name);
    }
    if answer.len() > bytes {
        let path = budget.store(answer).map_err(failed)?;
        let mut end = bytes;
        while !answer.is_char_boundary(end) {
            end -= 1;
        }
        result["text"] = json!(&answer[..end]);
        result["output_file"] = json!(path);
    }
    Ok(result.to_string())
}

async fn structured_result(weak: &WeakRuntime, id: &str) -> Result<String, ToolError> {
    let state = weak
        .upgrade()
        .ok_or_else(|| failed("Server stopped"))?
        .state(id)
        .await
        .map_err(|e| failed(e.to_string()))?;
    if let Some(value) = state.structured_result() {
        return Ok(json!({"id":id,"name":state.info.subagent_name,"result":value}).to_string());
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
    weak.upgrade()
        .ok_or_else(|| failed("Server stopped"))?
        .admit(id, admission)
        .await
        .map_err(|e| failed(e.to_string()))?;
    weak.wait_idle(id)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let state = weak
        .upgrade()
        .ok_or_else(|| failed("Server stopped"))?
        .state(id)
        .await
        .map_err(|e| failed(e.to_string()))?;
    match state.structured_result() {
        Some(value) => {
            Ok(json!({"id":id,"name":state.info.subagent_name,"result":value}).to_string())
        }
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
