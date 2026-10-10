//! `skill` (`builtin-tools` → skill tool, `skills-commands` → Skill tool).

use cyber_core::skills;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::permissions::Request;

pub(crate) struct SkillTool;

impl Tool for SkillTool {
    fn def(&self) -> ToolDef {
        def(
            "skill",
            "Load a skill listed in <available_skills> by name. arguments are substituted into the skill body. Forked skills run in background children and deliver only their summaries.",
            json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}, "arguments": {"type": "string"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let name = text(&ctx.inv.input, "name");
            let found = ctx.host.skills(&ctx.location);
            let model_invocable = |s: &&skills::Skill| !s.disable_model_invocation;
            let Some(skill) = found.skills.get(name).filter(model_invocable) else {
                let names: Vec<&str> = found
                    .skills
                    .values()
                    .filter(model_invocable)
                    .map(|s| s.name.as_str())
                    .collect();
                return Err(failed(format!(
                    "Skill \"{name}\" not found. Available: {}",
                    names.join(", ")
                )));
            };
            let req = Request {
                action: "skill".into(),
                resources: vec![name.into()],
                read_only: true,
                ..Request::default()
            };
            ctx.authorize(req, vec![name.into()], Value::Null).await?;
            let body = skills::instruction_frame(skill, text(&ctx.inv.input, "arguments"));
            if skill.context == skills::SkillContext::Fork {
                return super::agent::fork_skill(ctx, skill, body).await;
            }
            *ctx.loaded_skill
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(skills::SkillActivation {
                    name: skill.name.clone(),
                    allowed_tools: skill.allowed_tools.clone(),
                    disallowed_tools: skill.disallowed_tools.clone(),
                });
            Ok(body)
        })
    }
}
