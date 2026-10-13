//! Capture user-invoked skill declarations together with their expanded body.
use super::BuiltinHost;
use crate::permissions::{Decision, Request};
use cyber_server::runtime::{
    CommandPlan, CommandPlanError, CommandRequest, Delivery, Invocation, SkillCommand, TurnContext,
};
use serde_json::json;
use std::path::Path;

impl BuiltinHost {
    /// Discovered skills override commands; embedded defaults do not mask definitions.
    pub(crate) fn command_sources(
        &self,
        location: &Path,
        name: &str,
    ) -> (
        Option<cyber_core::skills::Skill>,
        cyber_core::commands::Commands,
    ) {
        let skill_name = name
            .strip_prefix("project:")
            .filter(|original| cyber_core::commands::invocation_name(original) == name)
            .unwrap_or(name);
        let registry = self.command_registry(location);
        let skill = self
            .skills(location)
            .skills
            .remove(skill_name)
            .filter(|skill| {
                skill.user_invocable
                    && (!skill.is_bundled()
                        || (!registry.entries.contains_key(name)
                            && !registry.unavailable.iter().any(|entry| entry == name)))
            });
        (skill, registry)
    }

    pub async fn command_plan(
        &self,
        turn: &TurnContext,
        name: &str,
        arguments: &str,
    ) -> Result<Option<CommandPlan>, CommandPlanError> {
        if ["help", "exit", "goal", "loop", "workflows", "mode"].contains(&name) {
            return Ok(None);
        }
        let location = Path::new(&turn.directory);
        let (skill, registry) = self.command_sources(location, name);
        // Precedence: skills, then MCP prompts, then configured commands.
        let Some(skill) = skill else {
            if let Some(plan) = self.mcp_prompt_plan(turn, name, arguments).await? {
                return Ok(Some(plan));
            }
            return Ok(registry
                .entries
                .get(name)
                .map(|command| CommandPlan::plain(command.expand(arguments))));
        };
        if let Some(model) = &skill.model {
            if model.len() > 256 || model.chars().any(char::is_control) {
                return Err("Invalid skill model reference".to_string().into());
            }
            cyber_llm::catalog::ModelRef::parse(model).map_err(|error| error.to_string())?;
        }
        let inv = command_invocation(turn, "skill", json!({"name":skill.name}));
        let policy = self.policy(&inv).await?;
        let request = Request {
            tool: Some("skill".into()),
            action: "skill".into(),
            resources: vec![skill.name.clone()],
            read_only: true,
            ..Default::default()
        };
        if let Decision::Deny(reason) = policy.user_delegation(&request) {
            return Err(format!("Permission denied: {reason}").into());
        }
        // Only the bundled review resolves its target; an override runs as defined.
        let review = skill.is_bundled() && skill.name == "review";
        let text = if review {
            let target = cyber_core::review::Target::parse(arguments)?;
            let block = self.review_target(&inv, policy, arguments).await?;
            format!(
                "{block}\n\n{}",
                cyber_core::skills::instruction_frame(&skill, &target.normalized())
            )
        } else if skill.context == cyber_core::skills::SkillContext::Fork {
            cyber_core::skills::instruction_frame(&skill, arguments)
        } else {
            cyber_core::skills::expand(&skill.body, arguments)
        };
        Ok(Some(CommandPlan {
            text,
            source: "user",
            skill: Some(SkillCommand {
                agent: skill.agent.clone(),
                fork: skill.context == cyber_core::skills::SkillContext::Fork,
                activation: cyber_core::skills::SkillActivation {
                    name: skill.name.clone(),
                    allowed_tools: skill.allowed_tools.clone(),
                    disallowed_tools: skill.disallowed_tools.clone(),
                },
                model: skill.model.clone(),
                output_schema: review.then(|| cyber_core::review::FINDINGS_SCHEMA.clone()),
                // Delivery is bound when the plan is admitted.
                request: review.then(|| CommandRequest {
                    name: name.into(),
                    arguments: arguments.into(),
                    delivery: Delivery::Steer,
                }),
            }),
        }))
    }

    async fn review_target(
        &self,
        inv: &Invocation,
        policy: crate::permissions::Policy,
        arguments: &str,
    ) -> Result<String, CommandPlanError> {
        // Withholding follows the reviewer's own ceiling, not the invoker's mode.
        let reader = self.policy_for(inv, Some("reviewer")).await?;
        let ctx = super::Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            host: self,
            inv,
            policy,
            location: super::canonical(Path::new(&inv.directory)),
            cancel: tokio_util::sync::CancellationToken::new(),
            hook_decision: None,
        };
        super::review::target_block(&ctx, &reader, arguments).await
    }
}

/// The detached invocation a user command's permission checks run as.
pub(super) fn command_invocation(
    turn: &TurnContext,
    name: &str,
    input: serde_json::Value,
) -> Invocation {
    Invocation {
        registration: None,
        session_id: turn.session_id.clone(),
        directory: turn.directory.clone(),
        agent: turn.agent.clone(),
        mode: turn.mode.clone(),
        message_id: String::new(),
        call_id: cyber_core::ids::new_id("call"),
        name: name.into(),
        input,
        attempt: 1,
        operation_key: cyber_core::ids::new_id("op"),
        asker: cyber_server::runtime::Asker::detached(),
        rules: turn.rules.clone(),
    }
}
