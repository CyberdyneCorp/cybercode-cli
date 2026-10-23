//! Workflow agents (`workflows` → agent() call) spawned through the shared agent path.
//!
//! Like a user subtask, a Workflow `agent()` call synthesizes an `agent` invocation for its
//! owner Session, so deny rules, the agent profile, the run Mode ceiling (`child_mode`),
//! SubagentStart/SubagentStop hooks and structured results all apply unchanged. The
//! operation key is derived from the run and call key, so a retried call is idempotent.

use std::path::PathBuf;

use cyber_server::runtime::{
    Asker, Invocation, WorkflowAgentOutput, WorkflowAgentRequest, WorkflowToolRequest,
};
use cyber_workflow::{Diagnostic, DiagnosticKind};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{BuiltinHost, Ctx};
use crate::tools::{self, ToolError};

/// The child's description (3–8 words); the `label` is recorded with the run.
const DESCRIPTION: &str = "Run workflow agent task";

impl BuiltinHost {
    pub(super) async fn workflow_agent(
        &self,
        request: WorkflowAgentRequest,
        cancel: CancellationToken,
    ) -> Result<WorkflowAgentOutput, Diagnostic> {
        let WorkflowAgentRequest {
            run_id,
            agent_id,
            call_key,
            owner,
            prompt,
            agent,
            model,
            schema,
            label: _,
            resume,
            worktree,
            goal,
            slot,
        } = request;
        // An unsettled agent is resumed, but when the server stopped between recording its
        // start and creating its child Session there is no child to continue: spawn one.
        let resume = resume && self.child_exists(&owner.session_id, &agent_id).await;
        let mut input = json!({"prompt": prompt, "background": false, "name": agent_id, "description": DESCRIPTION});
        // A resumed child keeps its agent profile and model; the shared path refuses changes.
        let choices = if resume {
            input["resume"] = input["name"].clone();
            [("agent", None), ("model", None)]
        } else {
            [("agent", agent), ("model", model)]
        };
        for (key, value) in choices {
            if let Some(value) = value {
                input[key] = value.into();
            }
        }
        if let Some(schema) = schema {
            input["output_schema"] = schema;
        }
        // A resumed child keeps the Goal it already has.
        if let Some(goal) = goal.filter(|_| !resume) {
            input["goal"] = goal;
        }
        // A resumed child keeps its checkout; the shared path refuses a change.
        if worktree.is_some() && !resume {
            input["isolation"] = "worktree".into();
        }
        let inv = Invocation {
            registration: None,
            session_id: owner.session_id,
            directory: owner.directory,
            agent: owner.agent,
            // The run Mode: `child_mode` keeps it as the ceiling of the child.
            mode: owner.mode,
            message_id: String::new(),
            call_id: cyber_core::ids::new_id("call"),
            name: "agent".into(),
            input,
            attempt: 1,
            operation_key: format!("{run_id}:{call_key}"),
            asker: Asker::detached(),
            rules: owner.rules,
        };
        self.check_agent_tool(&inv).map_err(denied)?;
        let ctx = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: self.policy(&inv).await.map_err(agent_failed)?,
            location: PathBuf::from(&inv.directory),
            inv: &inv,
            cancel,
        };
        let output = tools::agent::run_workflow(&ctx, run_id, worktree, slot)
            .await
            .map_err(failure)?;
        self.workflow_output(&output)
    }

    /// A Workflow script's direct read-only tool call (`workflows` → Direct read-only
    /// tools), for the owner under the run Mode. A non-interactive run has no approver: an
    /// ask is rejected with `No interactive approver for workflow run`.
    pub(super) async fn direct_tool(
        &self,
        request: WorkflowToolRequest,
        cancel: CancellationToken,
    ) -> Result<Value, Diagnostic> {
        let WorkflowToolRequest {
            run_id,
            call_key,
            owner,
            tool,
            input,
            interactive,
        } = request;
        let asker = match (interactive, self.runtime()) {
            (true, Some(runtime)) => runtime
                .operation_asker(&owner.session_id)
                .await
                .map_err(tool_failed)?,
            _ => Asker::detached(),
        };
        let inv = Invocation {
            registration: None,
            session_id: owner.session_id,
            directory: owner.directory,
            agent: owner.agent,
            mode: owner.mode,
            message_id: String::new(),
            call_id: cyber_core::ids::new_id("call"),
            name: if tool == "__solve" {
                "bash".into()
            } else {
                tool.clone()
            },
            input,
            attempt: 1,
            operation_key: format!("{run_id}:{call_key}"),
            asker,
            rules: owner.rules,
        };
        let ctx = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: self.policy(&inv).await.map_err(tool_failed)?,
            location: PathBuf::from(&inv.directory),
            inv: &inv,
            cancel,
        };
        let value = match tool.as_str() {
            "__solve" => self.verify_solve(&ctx, &run_id).await,
            "read" => tools::fs::read_value(&ctx).await,
            "glob" => tools::search::glob_value(&ctx).await,
            "grep" => tools::search::grep_value(&ctx).await,
            other => return Err(tool_failed(format!("unknown direct tool {other}"))),
        };
        value.map_err(|error| tool_failure(&tool, error))
    }

    /// Whether `owner` has the child Session named `agent_id`.
    async fn child_exists(&self, owner: &str, agent_id: &str) -> bool {
        match self.runtime() {
            Some(runtime) => runtime.resolve_subagent(owner, agent_id).await.is_ok(),
            // Without a runtime the shared path reports the failure itself.
            None => true,
        }
    }

    /// The child's answer plus the usage of its subtree.
    fn workflow_output(&self, output: &str) -> Result<WorkflowAgentOutput, Diagnostic> {
        let value: Value = serde_json::from_str(output).map_err(agent_failed)?;
        let session_id = value["id"]
            .as_str()
            .ok_or_else(|| agent_failed("the workflow agent returned no Session"))?
            .to_owned();
        let usage = self
            .runtime()
            .ok_or_else(|| agent_failed("workflow agents require the runtime"))?
            .job_usage(&session_id)
            .map_err(agent_failed)?;
        Ok(WorkflowAgentOutput {
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            structured: value.get("result").cloned(),
            cost: usage.total_cost(),
            tokens: usage.total_tokens(),
            branch: value["worktree"]["branch"].as_str().map(str::to_owned),
            goal_status: value["goal_status"].as_str().map(str::to_owned),
            session_id,
        })
    }

    /// `workflows` → Write isolation default: the Location is a git checkout, where a
    /// worktree can isolate the agent, and the profile (default `general`) offers an edit
    /// tool. A profile that cannot be resolved counts as a writer, so an unknown profile is
    /// isolated rather than sharing the checkout.
    pub(super) fn workflow_writer(&self, directory: &str, agent: Option<&str>) -> bool {
        let location = std::path::Path::new(directory);
        if cyber_core::project::identify(location).id == "global" {
            return false;
        }
        let Ok((config, _)) = (self.opts.config)(location) else {
            return true;
        };
        let Ok(profiles) = cyber_core::config::resolve_agents(&config) else {
            return true;
        };
        profiles
            .get(agent.unwrap_or("general"))
            .is_none_or(|profile| {
                !profile.read_only
                    && EDIT_TOOLS
                        .iter()
                        .any(|tool| super::profile_allows_tool(profile, tool))
            })
    }
}

impl BuiltinHost {
    /// `agent-teams` → isolation default: the profile offers an edit tool or `bash`. A
    /// profile that cannot be resolved counts as an editor.
    pub(crate) fn team_editor_agent(&self, directory: &str, agent: &str) -> bool {
        let Ok((config, _)) = (self.opts.config)(std::path::Path::new(directory)) else {
            return true;
        };
        let Ok(profiles) = cyber_core::config::resolve_agents(&config) else {
            return true;
        };
        profiles.get(agent).is_none_or(|profile| {
            !profile.read_only
                && EDIT_TOOLS
                    .iter()
                    .chain(&["bash"])
                    .any(|tool| super::profile_allows_tool(profile, tool))
        })
    }
}

/// Tools that edit files (`tools::action_of` maps them to `edit`).
const EDIT_TOOLS: [&str; 4] = ["write", "edit", "apply_patch", "notebook_edit"];

fn tool_failed(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::execution(error.to_string())
}

/// A direct tool's failure: a refused permission is `PermissionDenied`, reported as the
/// unattended workflow refusal when no approver could answer.
fn tool_failure(tool: &str, error: ToolError) -> Diagnostic {
    let message = match error {
        ToolError::Failed(message) => message,
        ToolError::Aborted => return tool_failed(format!("tools.{tool} was interrupted")),
    };
    if super::is_unattended_denial(&message) {
        return denied(cyber_server::runtime::NO_WORKFLOW_APPROVER);
    }
    if message.starts_with("Permission denied") || message.starts_with("Rejected by user") {
        return denied(message);
    }
    tool_failed(message)
}

fn agent_failed(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::AgentFailed, error.to_string())
}

fn denied(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::PermissionDenied, error.to_string())
}

/// Typed failures the script can branch on through `e.kind`.
fn failure(error: ToolError) -> Diagnostic {
    let message = match error {
        ToolError::Failed(message) => message,
        ToolError::Aborted => return agent_failed("the workflow agent was interrupted"),
    };
    if let Some(detail) = message.strip_prefix(tools::agent::SCHEMA_MISMATCH) {
        return Diagnostic::new(DiagnosticKind::SchemaMismatch, detail);
    }
    if message.starts_with("Permission denied") {
        return denied(message);
    }
    agent_failed(message)
}
