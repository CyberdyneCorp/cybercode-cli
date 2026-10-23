//! The model-facing `workflow` tool (`workflows` → Model-authored workflows, Permissions for
//! runs): start a saved Workflow by name, or an inline script, as a run owned by the
//! calling Session. Starting needs the `workflow.run` permission on the module's
//! origin-qualified resource ([`workflow_resource`]): the name of a global saved
//! Workflow, `project:<name>` or `inline:<name>@<digest>`, so an approval or allow rule
//! for one never covers another script declaring the same name. A deny rule for the bare
//! name refuses every origin. The approval shows the name, phases and budgets before any
//! agent starts. The run executes in the server and its summary returns as a queued
//! message.

use std::path::Path;

use cyber_server::runtime::workflow_start::{self, StartError};
use cyber_server::runtime::{RetrySafety, RunStarter, StartWorkflowRun, ToolDef, TurnContext};
use cyber_workflow::{Origin, WorkflowModule};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed};
use crate::host::Ctx;
use crate::permissions::{Request, WORKFLOW_RUN, workflow_resource};

pub(crate) struct Workflow;

/// The context key of the Workflow library listing.
pub(crate) const LIBRARY_KEY: &str = "core/workflows";
/// Library entries listed at most.
const LIBRARY_LISTED: usize = 20;

/// The Workflows the `workflow` tool can start by name in `location`, bundled ones first,
/// for the model's context (`workflows` → Workflow library over HTTP): at most 20, each
/// with its origin, description and argument names. `None` when the library is empty.
pub(crate) fn library_listing(host: &crate::host::BuiltinHost, location: &Path) -> Option<String> {
    let (config, _) = (host.opts.config)(location).ok()?;
    let sources = workflow_start::LibrarySources::of(&config, host.opts.env.as_ref());
    let listing = workflow_start::discover(&host.opts.global_config_dir, location, sources);
    let mut entries: Vec<&WorkflowModule> = listing
        .workflows
        .iter()
        .map(|entry| &entry.module)
        .collect();
    entries.sort_by_key(|module| {
        (
            !matches!(module.origin, Origin::Bundled { .. }),
            module.meta.name.clone(),
        )
    });
    if entries.is_empty() {
        return None;
    }
    let mut text = String::from(
        "Workflow library: start one of these with the workflow tool by name, and prefer it over an inline script when it fits the task.\n",
    );
    for module in entries.iter().take(LIBRARY_LISTED) {
        text.push_str(&library_line(module));
    }
    if entries.len() > LIBRARY_LISTED {
        text.push_str(&format!(
            "- … and {} more (`cyber workflows list`)\n",
            entries.len() - LIBRARY_LISTED
        ));
    }
    Some(text)
}

fn library_line(module: &WorkflowModule) -> String {
    let origin = match &module.origin {
        Origin::Bundled { .. } => "bundled",
        Origin::Saved { scope, .. } => match scope {
            cyber_workflow::Scope::Global => "global",
            cyber_workflow::Scope::Project => "project",
        },
        Origin::Inline => "inline",
    };
    let args: Vec<&str> = module
        .meta
        .args
        .as_ref()
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let args = if args.is_empty() {
        String::new()
    } else {
        format!(" Args: {}.", args.join(", "))
    };
    format!(
        "- {} ({origin}): {}{args}\n",
        module.meta.name, module.meta.description
    )
}

impl Tool for Workflow {
    fn def(&self) -> ToolDef {
        def(
            "workflow",
            "Start a Workflow Run: a saved Workflow by name, or an inline JavaScript module (export const meta = {name, description, phases?}; export default async () => { ... } using agent(), parallel(), pipeline(), phase(), log(), checkpoint() and, when meta.tools or the tools option names them, tools.read({path, offset?, limit?}), tools.glob({pattern, path?, max?}) and tools.grep({pattern, path?, include?, max?})). The run executes in the background on the server; its summary arrives later as a queued message. Returns the run handle.",
            json!({"type":"object", "additionalProperties":false, "properties":{
                "name":{"type":"string", "description":"A saved Workflow's meta.name"},
                "script":{"type":"string", "description":"An inline JavaScript Workflow module"},
                "args":{"type":"object", "description":"The script's frozen args"},
                "budget":{"type":"object", "description":"Run budget fields: max_cost_usd, max_tokens, max_turns, max_wall_seconds, max_agents"},
                "max_concurrent":{"type":"integer", "minimum":1, "description":"At most this many of the run's agents at once (capped at the server-wide workflows.max_concurrent)"},
                "tools":{"type":"array", "items":{"enum":["read", "glob", "grep"]}, "description":"Read-only tools the script may call directly (tools.read, tools.glob, tools.grep) besides those its meta.tools names"}
            }}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(run(ctx))
    }
}

fn start_failed(error: StartError) -> ToolError {
    failed(error.to_string())
}

fn module(ctx: &Ctx<'_>, input: &Value) -> Result<WorkflowModule, ToolError> {
    match (input.get("name"), input.get("script")) {
        (Some(Value::String(name)), None) => {
            let (config, _) = (ctx.host.opts.config)(&ctx.location).map_err(failed)?;
            let sources = workflow_start::LibrarySources::of(&config, ctx.host.opts.env.as_ref());
            workflow_start::saved(
                &ctx.host.opts.global_config_dir,
                &ctx.location,
                name,
                sources,
            )
            .map_err(start_failed)
        }
        (None, Some(Value::String(script))) => {
            workflow_start::inline(script.clone()).map_err(start_failed)
        }
        _ => Err(failed(
            "Give exactly one of name (a saved Workflow) or script (an inline module)",
        )),
    }
}

/// What the approval shows: the workflow, its phases, the effective budgets it runs under
/// (the shared Run budgets resolution of `meta.budget`, the invocation and `budgets.run`),
/// its agent estimate and the tools it calls directly.
fn approval(module: &WorkflowModule, budget: &Value, tools: &[String]) -> Value {
    let origin = match &module.origin {
        Origin::Saved { path, .. } => json!(path),
        Origin::Bundled { name } => json!(format!("bundled:{name}")),
        Origin::Inline => json!("inline"),
    };
    json!({
        "name": module.meta.name,
        "description": module.meta.description,
        "phases": module.meta.phases.iter().map(|phase| phase.title.clone()).collect::<Vec<_>>(),
        "budget": budget,
        "estimate": module.meta.raw.get("estimate"),
        "tools": module.meta.tools.iter().chain(tools).collect::<Vec<_>>(),
        "origin": origin,
        "digest": module.digest,
    })
}

/// The owner Session as permission rules see it.
fn owner(ctx: &Ctx<'_>) -> TurnContext {
    TurnContext {
        session_id: ctx.inv.session_id.clone(),
        directory: ctx.inv.directory.clone(),
        agent: ctx.inv.agent.clone(),
        mode: ctx.inv.mode.clone(),
        prefers_apply_patch: false,
        rules: ctx.inv.rules.clone(),
        worktree: Default::default(),
    }
}

async fn run(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Workflow runs require the runtime"))?;
    // Fails closed: a Session whose ancestry cannot be read starts nothing. The runtime
    // checks again when the run starts.
    let scope = runtime
        .workflow_run_of(&ctx.inv.session_id)
        .await
        .map_err(|error| failed(error.to_string()))?;
    if scope.is_some() {
        return Err(failed(cyber_server::runtime::WORKFLOW_AGENT_STARTS_RUN));
    }
    let input = &ctx.inv.input;
    let module = module(ctx, input)?;
    let (config, _) = (ctx.host.opts.config)(&ctx.location).map_err(failed)?;
    let settings = workflow_start::location(&config).map_err(failed)?;
    let budget =
        workflow_start::budget(&settings, &module, input.get("budget")).map_err(start_failed)?;
    let share = workflow_start::share(&settings, &module, input.get("max_concurrent"))
        .map_err(start_failed)?;
    let tools = workflow_start::invocation_tools(input.get("tools")).map_err(start_failed)?;
    let args = match input.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(args @ Value::Object(_)) => args.clone(),
        Some(_) => return Err(failed("args must be an object")),
    };
    let interactive = ctx.inv.asker.attended();
    let args = ctx
        .host
        .prepare_workflow_args(&owner(ctx), &module, args, interactive)
        .await
        .map_err(|error| failed(error.to_string()))?;
    let shown = serde_json::to_value(&budget).unwrap_or(Value::Null);
    let request = |resource: String| Request {
        action: WORKFLOW_RUN.into(),
        resources: vec![resource],
        ..Default::default()
    };
    ctx.refuse_denied(request(module.meta.name.clone())).await?;
    let resource = workflow_resource(&module);
    ctx.authorize(
        request(resource.clone()),
        vec![resource],
        approval(&module, &shown, &tools),
    )
    .await?;
    let run = runtime
        .start_workflow_run(StartWorkflowRun {
            module,
            args,
            session_id: ctx.inv.session_id.clone(),
            limits: settings.limits,
            tiers: settings.tiers,
            budget,
            share,
            starter: RunStarter::Session { interactive },
            tools,
        })
        .await
        .map_err(|error| failed(error.to_string()))?;
    let mut handle = json!({
        "run_id": run.id,
        "name": run.name,
        "status": run.status,
        "phases": run.phases,
        "budget": run.budget,
        "share": run.share,
        "script_file": run.script_file.as_deref().map(Path::new),
        "message": "The run continues in the background. Its summary arrives as a queued message when it ends; do not wait or poll for it.",
    });
    // Every run slot was taken: the run waits `queued` and starts in turn.
    if let Some(position) = run.queue_position {
        handle["queue_position"] = position.into();
    }
    Ok(handle.to_string())
}
