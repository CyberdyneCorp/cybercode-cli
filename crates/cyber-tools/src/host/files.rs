//! Permission for the file browsing routes (`server-api` → File browsing routes): the
//! same `read`, `list` and `external_directory` checks a model's `read` call gets, so a
//! client can browse only what the Session's rules and Mode allow.

use std::path::{Path, PathBuf};

use cyber_server::runtime::{Asker, FileDecision, Invocation, TurnContext};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{BuiltinHost, Ctx, canonical};
use crate::permissions::{self, Decision, Policy, Request};
use crate::tools::ToolError;

/// A synthetic call of the `read` tool (or `list`) on `path`.
fn invocation(turn: &TurnContext, action: &str, path: &Path, asker: Asker) -> Invocation {
    let name = match action {
        "list" => "list",
        "edit" => "edit",
        _ => "read",
    };
    Invocation {
        tool_output: Default::default(),
        registration: None,
        session_id: turn.session_id.clone(),
        directory: turn.directory.clone(),
        agent: turn.agent.clone(),
        mode: turn.mode.clone(),
        message_id: asker.message_id.clone(),
        call_id: asker.call_id.clone(),
        name: name.into(),
        input: json!({ "path": path.display().to_string() }),
        attempt: 1,
        operation_key: cyber_core::ids::new_id("op"),
        asker,
        rules: turn.rules.clone(),
    }
}

fn file_decision(decision: Decision) -> FileDecision {
    match decision {
        Decision::Allow => FileDecision::Allow,
        Decision::Ask => FileDecision::Ask,
        Decision::Deny(_) => FileDecision::Deny,
    }
}

fn stricter(a: FileDecision, b: FileDecision) -> FileDecision {
    use FileDecision::{Allow, Ask, Deny};
    match (a, b) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        (Allow, Allow) => Allow,
    }
}

/// The decision for `action` on `path`, with `external_directory` when the path leaves
/// the Location.
fn decide(policy: &Policy, location: &Path, tool: &str, action: &str, path: &Path) -> FileDecision {
    let path = canonical(path);
    let resource = path
        .strip_prefix(location)
        .map_or_else(|_| permissions::slash(&path), permissions::slash);
    let edit = action == "edit";
    let request = |action: &str, resource: String| Request {
        tool: Some(tool.into()),
        action: action.into(),
        resources: vec![resource],
        read_only: !edit,
        file_edit: edit,
        mutates: if edit { vec![path.clone()] } else { vec![] },
        ..Request::default()
    };
    let decision = file_decision(policy.decide(&request(action, resource)));
    if path.starts_with(location) {
        return decision;
    }
    let dir = if path.is_dir() {
        path.clone()
    } else {
        path.parent()
            .map_or_else(|| path.clone(), Path::to_path_buf)
    };
    let external = request(
        "external_directory",
        format!("{}/*", permissions::slash(&dir)),
    );
    stricter(decision, file_decision(policy.decide(&external)))
}

impl BuiltinHost {
    /// The policy a browsing request is decided under: the Session's agent and its
    /// ancestors' Modes, or the Location's rules without a Session.
    async fn browse_policy(&self, inv: &Invocation) -> Result<Policy, String> {
        let agent = (!inv.session_id.is_empty()).then_some(inv.agent.as_str());
        self.policy_for(inv, agent).await
    }

    async fn browse_context<'a>(
        &'a self,
        inv: &'a Invocation,
        cancel: CancellationToken,
    ) -> Result<Ctx<'a>, String> {
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            provenance: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: self,
            policy: self.browse_policy(inv).await?,
            location: canonical(Path::new(&inv.directory)),
            inv,
            cancel,
        })
    }

    pub(crate) async fn decide_files(
        &self,
        turn: &TurnContext,
        action: &str,
        paths: &[PathBuf],
    ) -> Vec<FileDecision> {
        if !matches!(action, "read" | "list" | "edit") {
            return vec![FileDecision::Deny; paths.len()];
        }
        let inv = invocation(turn, action, Path::new(&turn.directory), Asker::detached());
        let Ok(ctx) = self.browse_context(&inv, CancellationToken::new()).await else {
            return vec![FileDecision::Deny; paths.len()];
        };
        let location = canonical(Path::new(&turn.directory));
        paths
            .iter()
            .map(|path| {
                if action == "edit"
                    && crate::sandboxing::binary_edit_boundary(&ctx, &canonical(path)).is_err()
                {
                    return FileDecision::Deny;
                }
                decide(&ctx.policy, &location, &inv.name, action, path)
            })
            .collect()
    }

    /// Authorize as the `read` tool does: `external_directory`, then the action, with
    /// permission hooks and the normal request when the rules say `ask`.
    pub(crate) async fn authorize_browse(
        &self,
        turn: &TurnContext,
        action: &str,
        path: &Path,
        asker: Asker,
        cancel: CancellationToken,
        mut metadata: Value,
    ) -> Result<(), String> {
        if !matches!(action, "read" | "list" | "edit") {
            return Err("Invalid file action".into());
        }
        if action == "edit" {
            // An owner editor operation has its own explicit review. A normal
            // permission ask remains manual; it must not start classifier inference.
            if !metadata.is_object() {
                metadata = json!({});
            }
            metadata["requires_confirmation"] = json!(true);
        }
        let inv = invocation(turn, action, path, asker);
        let ctx = self.browse_context(&inv, cancel).await?;
        let path = canonical(path);
        let resource = ctx.resource(&path);
        let request = Request {
            action: action.into(),
            resources: vec![resource.clone()],
            read_only: action != "edit",
            file_edit: action == "edit",
            mutates: if action == "edit" {
                vec![path.clone()]
            } else {
                vec![]
            },
            ..Request::default()
        };
        let outcome = async {
            if action == "edit" {
                crate::sandboxing::binary_edit_boundary(&ctx, &path).map_err(ToolError::Failed)?;
                ctx.check_external_writes(std::slice::from_ref(&path))
                    .await?;
            } else {
                ctx.check_external(std::slice::from_ref(&path)).await?;
            }
            ctx.authorize(request, vec![resource], metadata).await
        };
        match outcome.await {
            Ok(()) => Ok(()),
            Err(ToolError::Failed(message)) => Err(message),
            Err(ToolError::Aborted) => Err("The request was cancelled".into()),
        }
    }
}
