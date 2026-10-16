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
use cyber_server::runtime::{RetrySafety, RunStarter, StartWorkflowRun, ToolDef};
use cyber_workflow::{Origin, WorkflowModule};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed};
use crate::host::Ctx;
use crate::permissions::{Request, WORKFLOW_RUN, workflow_resource};

pub(crate) struct Workflow;

impl Tool for Workflow {
    fn def(&self) -> ToolDef {
        def(
            "workflow",
            "Start a Workflow Run: a saved Workflow by name, or an inline JavaScript module (export const meta = {name, description, phases?}; export default async () => { ... } using agent(), parallel(), pipeline(), phase(), log() and checkpoint()). The run executes in the background on the server; its summary arrives later as a queued message. Returns the run handle.",
            json!({"type":"object", "additionalProperties":false, "properties":{
                "name":{"type":"string", "description":"A saved Workflow's meta.name"},
                "script":{"type":"string", "description":"An inline JavaScript Workflow module"},
                "args":{"type":"object", "description":"The script's frozen args"},
                "budget":{"type":"object", "description":"Run budget fields: max_cost_usd, max_tokens, max_turns, max_wall_seconds, max_agents"}
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
            let project = !ctx.host.opts.env.flag("CYBER_DISABLE_PROJECT_CONFIG");
            workflow_start::saved(
                &ctx.host.opts.global_config_dir,
                &ctx.location,
                name,
                project,
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

/// What the approval shows: the workflow, its phases and the budgets it runs under.
fn approval(module: &WorkflowModule, budget: &Value) -> Value {
    let origin = match &module.origin {
        Origin::Saved { path, .. } => json!(path),
        Origin::Inline => json!("inline"),
    };
    json!({
        "name": module.meta.name,
        "description": module.meta.description,
        "phases": module.meta.phases.iter().map(|phase| phase.title.clone()).collect::<Vec<_>>(),
        "budget": budget,
        "origin": origin,
        "digest": module.digest,
    })
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
        approval(&module, &shown),
    )
    .await?;
    let args = match input.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(args @ Value::Object(_)) => args.clone(),
        Some(_) => return Err(failed("args must be an object")),
    };
    let run = runtime
        .start_workflow_run(StartWorkflowRun {
            module,
            args,
            session_id: ctx.inv.session_id.clone(),
            limits: settings.limits,
            tiers: settings.tiers,
            budget,
            starter: RunStarter::Session {
                interactive: ctx.inv.asker.attended(),
            },
        })
        .await
        .map_err(|error| failed(error.to_string()))?;
    Ok(json!({
        "run_id": run.id,
        "name": run.name,
        "status": run.status,
        "phases": run.phases,
        "budget": run.budget,
        "script_file": run.script_file.as_deref().map(Path::new),
        "message": "The run continues in the background. Its summary arrives as a queued message when it ends; do not wait or poll for it.",
    })
    .to_string())
}
