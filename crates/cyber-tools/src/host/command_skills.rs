//! Capture user-invoked skill declarations together with their expanded body.
use super::BuiltinHost;
use crate::permissions::{Decision, Request};
use cyber_server::runtime::{CommandPlan, Invocation, SkillCommand, TurnContext};
use serde_json::json;
use std::path::Path;

impl BuiltinHost {
    pub async fn command_plan(
        &self,
        turn: &TurnContext,
        name: &str,
        arguments: &str,
    ) -> Result<Option<CommandPlan>, String> {
        if ["help", "exit", "goal", "loop", "workflows", "mode"].contains(&name) {
            return Ok(None);
        }
        let skill_name = name
            .strip_prefix("project:")
            .filter(|original| cyber_core::commands::invocation_name(original) == name)
            .unwrap_or(name);
        let location = Path::new(&turn.directory);
        let found = self.skills(location);
        let Some(skill) = found
            .skills
            .get(skill_name)
            .filter(|skill| skill.user_invocable)
        else {
            return Ok(self
                .command_registry(location)
                .entries
                .get(name)
                .map(|command| CommandPlan::plain(command.expand(arguments))));
        };
        if let Some(model) = &skill.model {
            if model.len() > 256 || model.chars().any(char::is_control) {
                return Err("Invalid skill model reference".into());
            }
            cyber_llm::catalog::ModelRef::parse(model).map_err(|error| error.to_string())?;
        }
        let inv = Invocation {
            registration: None,
            session_id: turn.session_id.clone(),
            directory: turn.directory.clone(),
            agent: turn.agent.clone(),
            mode: turn.mode.clone(),
            message_id: String::new(),
            call_id: cyber_core::ids::new_id("call"),
            name: "skill".into(),
            input: json!({"name":skill.name}),
            attempt: 1,
            operation_key: cyber_core::ids::new_id("op"),
            asker: cyber_server::runtime::Asker::detached(),
            rules: turn.rules.clone(),
        };
        let policy = self.policy(&inv).await?;
        let request = Request {
            tool: Some("skill".into()),
            action: "skill".into(),
            resources: vec![skill.name.clone()],
            read_only: true,
            ..Default::default()
        };
        if let Decision::Deny(reason) = policy.user_delegation(&request) {
            return Err(format!("Permission denied: {reason}"));
        }
        Ok(Some(CommandPlan {
            text: if skill.context == cyber_core::skills::SkillContext::Fork {
                cyber_core::skills::instruction_frame(skill, arguments)
            } else {
                cyber_core::skills::expand(&skill.body, arguments)
            },
            skill: Some(SkillCommand {
                agent: skill.agent.clone(),
                fork: skill.context == cyber_core::skills::SkillContext::Fork,
                activation: cyber_core::skills::SkillActivation {
                    name: skill.name.clone(),
                    allowed_tools: skill.allowed_tools.clone(),
                    disallowed_tools: skill.disallowed_tools.clone(),
                },
                model: skill.model.clone(),
            }),
        }))
    }
}
