//! Capture user-invoked skill declarations together with their expanded body.
use super::BuiltinHost;
use crate::permissions::{Decision, Request};
use cyber_core::skills::{Expanded, SkillOrigin};
use cyber_server::runtime::{
    CommandPlan, CommandPlanError, CommandRequest, Delivery, Invocation, PromptTemplate,
    SkillCommand, TemplateInjection, TurnContext,
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
        if cyber_core::commands::RESERVED.contains(&name) {
            return Ok(None);
        }
        let location = Path::new(&turn.directory);
        let (skill, registry) = self.command_sources(location, name);
        // Precedence: skills, then MCP prompts, then configured commands.
        let Some(skill) = skill else {
            if let Some(plan) = self.mcp_prompt_plan(turn, name, arguments).await? {
                return Ok(Some(plan));
            }
            let Some(command) = registry.entries.get(name) else {
                return Ok(None);
            };
            return self
                .static_command_plan(turn, &registry, name, command, arguments)
                .await
                .map(Some);
        };
        validate_model(skill.model.as_deref())?;
        let inv = command_invocation(turn, "skill", json!({"name":skill.name}));
        let policy = self.policy(&inv).await?;
        if let Decision::Deny(reason) = policy.user_delegation(&skill_request(&skill.name)) {
            return Err(format!("Permission denied: {reason}").into());
        }
        // Only the bundled review resolves its target; an override runs as defined.
        let review = skill.is_bundled() && skill.name == "review";
        let command = skill_command(&skill, review, name, arguments);
        // The review target block is resolved data: it is never expanded.
        if review {
            let target = cyber_core::review::Target::parse(arguments)?;
            let block = self.review_target(&inv, policy, arguments).await?;
            let text = format!(
                "{block}\n\n{}",
                cyber_core::skills::instruction_frame(&skill, &target.normalized())
            );
            return Ok(Some(CommandPlan {
                skill: Some(command),
                ..CommandPlan::plain(text)
            }));
        }
        let source = TemplateSource::skill(&skill);
        let mut plan = self.template_plan(location, source, skill_text(&skill, arguments))?;
        plan.skill = Some(command);
        Ok(Some(plan))
    }

    /// A configured command's template plan; agent, model, variant and subtask
    /// overrides capture a [`SkillCommand`], so a child run shares the forked-skill
    /// route, retry identity and model validation.
    async fn static_command_plan(
        &self,
        turn: &TurnContext,
        registry: &cyber_core::commands::Commands,
        name: &str,
        command: &cyber_core::commands::StaticCommand,
        arguments: &str,
    ) -> Result<CommandPlan, CommandPlanError> {
        let skill = if command.has_overrides() {
            let (config, _) = (self.opts.config)(Path::new(&turn.directory))
                .map_err(CommandPlanError::Unavailable)?;
            let profiles = cyber_core::config::resolve_agents(&config)
                .map_err(CommandPlanError::Unavailable)?;
            let profile = command.agent.as_ref().and_then(|agent| profiles.get(agent));
            let session = self.session_model(&turn.session_id).await?;
            Some(command_overrides(
                name,
                command,
                &turn.agent,
                profile,
                &session,
            )?)
        } else {
            None
        };
        let source = TemplateSource::command(registry, name, command);
        let location = Path::new(&turn.directory);
        let mut plan = self.template_plan(location, source, command.expand_template(arguments))?;
        plan.skill = skill;
        Ok(plan)
    }

    /// The model the Session's next Turn would use.
    async fn session_model(&self, session: &str) -> Result<String, CommandPlanError> {
        let runtime = self.runtime().ok_or_else(|| {
            CommandPlanError::Unavailable("Command planning requires the runtime".into())
        })?;
        let state = runtime
            .state(session)
            .await
            .map_err(|error| CommandPlanError::Unavailable(error.to_string()))?;
        Ok(state
            .command_model()
            .map_or_else(|| state.info.model.clone(), str::to_owned))
    }

    /// Approve the current project template of `/name` for this checkout
    /// (`cyber commands trust`); a changed template has a new digest and needs a new
    /// approval. Returns the approved digest.
    pub fn trust_template(&self, location: &Path, name: &str) -> Result<String, String> {
        let (skill, registry) = self.command_sources(location, name);
        let source = match (&skill, registry.entries.get(name)) {
            (Some(skill), _) => TemplateSource::skill(skill),
            (None, Some(command)) => TemplateSource::command(&registry, name, command),
            (None, None) => return Err(format!("No command or skill named {name}")),
        };
        if !source.project {
            return Err(format!(
                "/{name} is not defined by the project and needs no approval"
            ));
        }
        let trust = self
            .hook_config
            .get()
            .ok_or("Trust storage is unavailable")?;
        let root = cyber_core::config::project_root(location);
        trust
            .trust
            .approve_template(&root, &source.digest)
            .map_err(|error| error.to_string())?;
        Ok(source.digest)
    }

    /// A template plan; a project template that runs shell commands is refused until
    /// its exact digest is approved for the checkout.
    fn template_plan(
        &self,
        location: &Path,
        source: TemplateSource,
        expanded: Expanded,
    ) -> Result<CommandPlan, CommandPlanError> {
        let TemplateSource {
            kind,
            name,
            project,
            digest,
        } = source;
        if project && !expanded.injections.is_empty() && !self.template_trusted(location, &digest) {
            return Err(CommandPlanError::Forbidden(format!(
                "Project {kind} /{name} runs shell commands and is not trusted for this checkout; \
                 review it, then approve it with `cyber commands trust {name}` (digest {digest})"
            )));
        }
        let template = PromptTemplate {
            kind: kind.into(),
            name,
            project,
            digest,
            injections: expanded
                .injections
                .into_iter()
                .map(|injection| TemplateInjection {
                    start: injection.start,
                    end: injection.end,
                    command: injection.command,
                })
                .collect(),
        };
        Ok(CommandPlan {
            template: Some(template),
            ..CommandPlan::plain(expanded.text)
        })
    }

    async fn review_target(
        &self,
        inv: &Invocation,
        policy: crate::permissions::Policy,
        arguments: &str,
    ) -> Result<String, CommandPlanError> {
        let reader = self.policy_for(inv, Some("reviewer")).await?;
        let ctx = self.review_ctx(inv, policy);
        super::review::target_block(&ctx, &reader, arguments).await
    }

    /// The diff of a review target as data, resolved as `/review` resolves it.
    pub(super) async fn review_diff(
        &self,
        inv: &Invocation,
        policy: crate::permissions::Policy,
        arguments: &str,
    ) -> Result<serde_json::Value, CommandPlanError> {
        let reader = self.policy_for(inv, Some("reviewer")).await?;
        let ctx = self.review_ctx(inv, policy);
        super::review::target_diff(&ctx, &reader, arguments).await
    }

    fn review_ctx<'a>(
        &'a self,
        inv: &'a Invocation,
        policy: crate::permissions::Policy,
    ) -> super::Ctx<'a> {
        // Withholding follows the reviewer's own ceiling, not the invoker's mode.
        super::Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            provenance: Default::default(),
            compiler_feedback: Default::default(),
            host: self,
            inv,
            policy,
            location: super::canonical(Path::new(&inv.directory)),
            cancel: tokio_util::sync::CancellationToken::new(),
            hook_decision: None,
            denial: Default::default(),
        }
    }
}

fn validate_model(model: Option<&str>) -> Result<(), CommandPlanError> {
    let Some(model) = model else {
        return Ok(());
    };
    if model.len() > 256 || model.chars().any(char::is_control) {
        return Err("Invalid skill model reference".to_string().into());
    }
    cyber_llm::catalog::ModelRef::parse(model).map_err(|error| error.to_string())?;
    Ok(())
}

/// The captured declaration of a command with overrides. A `subtask`, or an agent
/// whose mode is `subagent`, runs in a child Session; otherwise the command runs inline,
/// which keeps the Session's agent, so naming a different agent needs `subtask: true`.
fn command_overrides(
    name: &str,
    command: &cyber_core::commands::StaticCommand,
    caller: &str,
    profile: Option<&cyber_core::config::AgentProfile>,
    session_model: &str,
) -> Result<SkillCommand, CommandPlanError> {
    let agent = command.agent.as_deref();
    if agent.is_some() && profile.is_none_or(|profile| profile.hidden) {
        return Err(CommandPlanError::Invalid(format!(
            "Command /{name} names agent {:?}, which is not configured",
            agent.unwrap_or_default()
        )));
    }
    let fork = command.subtask || profile.is_some_and(|profile| profile.mode == "subagent");
    if fork && profile.is_some_and(|profile| !profile.subagent_capable()) {
        return Err(CommandPlanError::Invalid(format!(
            "Command /{name} runs as a subtask, but agent {:?} cannot run as a subagent",
            agent.unwrap_or_default()
        )));
    }
    if !fork && agent.is_some_and(|agent| agent != caller) {
        return Err(CommandPlanError::Invalid(format!(
            "Command /{name} selects agent {:?}, but an inline command keeps the Session's \
             agent {caller:?}; set `subtask: true` to run it in a child Session",
            agent.unwrap_or_default()
        )));
    }
    Ok(SkillCommand {
        agent: agent.filter(|_| fork).map(str::to_owned),
        fork,
        activation: cyber_core::skills::SkillActivation {
            name: name.into(),
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        },
        model: override_model(command, profile, session_model)?,
        output_schema: None,
        request: None,
    })
}

/// The command `model`, then the command agent's model (with the agent's variant),
/// then the Session's. A command `variant` applies to whichever model is selected;
/// `None` keeps the Session model unchanged.
fn override_model(
    command: &cyber_core::commands::StaticCommand,
    profile: Option<&cyber_core::config::AgentProfile>,
    session_model: &str,
) -> Result<Option<String>, CommandPlanError> {
    let agent = profile.filter(|_| command.agent.is_some());
    let (base, default_variant) = match (&command.model, agent.and_then(|a| a.model.as_ref())) {
        (Some(model), _) => (model.as_str(), None),
        (None, Some(model)) => (model.as_str(), agent.and_then(|a| a.variant.as_ref())),
        (None, None) if command.variant.is_some() => (session_model, None),
        (None, None) => return Ok(None),
    };
    validate_model(Some(base))?;
    let mut model = cyber_llm::catalog::ModelRef::parse(base).map_err(|error| error.to_string())?;
    if let Some(variant) = command
        .variant
        .as_ref()
        .or(default_variant.filter(|_| model.variant.is_none()))
    {
        model.variant = Some(variant.clone());
    }
    Ok(Some(model.to_string()))
}

fn skill_request(name: &str) -> Request {
    Request {
        tool: Some("skill".into()),
        action: "skill".into(),
        resources: vec![name.into()],
        read_only: true,
        ..Default::default()
    }
}

/// Where a planned template came from, with its trust digest.
struct TemplateSource {
    kind: &'static str,
    name: String,
    project: bool,
    digest: String,
}

impl TemplateSource {
    fn command(
        registry: &cyber_core::commands::Commands,
        name: &str,
        command: &cyber_core::commands::StaticCommand,
    ) -> Self {
        let project = registry.provenance.get(name).is_some_and(|provenance| {
            provenance.winner.scope == cyber_core::commands::CommandSourceScope::Project
        });
        Self::new("command", name, &command.template, project)
    }

    /// Configured skill paths may come from project configuration, so they count as
    /// project-controlled.
    fn skill(skill: &cyber_core::skills::Skill) -> Self {
        let project = matches!(skill.origin, SkillOrigin::Project | SkillOrigin::Config);
        Self::new("skill", &skill.name, &skill.body, project)
    }

    fn new(kind: &'static str, name: &str, template: &str, project: bool) -> Self {
        Self {
            kind,
            name: name.into(),
            project,
            digest: cyber_core::commands::template_digest(kind, name, template),
        }
    }
}

/// Forked skills carry their frame to the child; inline skills submit the expanded body.
fn skill_text(skill: &cyber_core::skills::Skill, arguments: &str) -> Expanded {
    if skill.context == cyber_core::skills::SkillContext::Fork {
        cyber_core::skills::instruction_frame_template(skill, arguments)
    } else {
        cyber_core::skills::expand_template(
            &skill.body,
            arguments,
            cyber_core::skills::Append::Labeled,
        )
    }
}

fn skill_command(
    skill: &cyber_core::skills::Skill,
    review: bool,
    name: &str,
    arguments: &str,
) -> SkillCommand {
    SkillCommand {
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
    }
}

/// The detached invocation a user command's permission checks run as.
pub(super) fn command_invocation(
    turn: &TurnContext,
    name: &str,
    input: serde_json::Value,
) -> Invocation {
    Invocation {
        tool_output: Default::default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_core::commands::StaticCommand;
    use cyber_core::config::AgentProfile;

    fn command(value: serde_json::Value) -> StaticCommand {
        let mut value = value;
        value["template"] = "Go".into();
        StaticCommand::parse_template(&value).unwrap()
    }

    fn agent(name: &str, mode: &str, model: Option<&str>) -> AgentProfile {
        AgentProfile {
            name: name.into(),
            mode: mode.into(),
            model: model.map(Into::into),
            ..Default::default()
        }
    }

    /// `skills-commands` → Agent, model and subtask overrides: the command model, then
    /// the command agent's model, then the Session model.
    #[test]
    fn command_model_resolves_command_then_agent_then_session() {
        let reviewer = agent("reviewer", "subagent", Some("corp/agent"));
        let resolve = |value, profile: Option<&AgentProfile>| {
            override_model(&command(value), profile, "corp/session")
        };
        assert_eq!(
            resolve(json!({"model":"corp/command"}), Some(&reviewer)).unwrap(),
            Some("corp/command".into())
        );
        assert_eq!(
            resolve(json!({"agent":"reviewer"}), Some(&reviewer)).unwrap(),
            Some("corp/agent".into())
        );
        // No command or agent model: the Session model stays in effect.
        let plain = agent("general", "subagent", None);
        assert_eq!(
            resolve(json!({"agent":"general"}), Some(&plain)).unwrap(),
            None
        );
        assert_eq!(resolve(json!({"subtask":true}), None).unwrap(), None);
        // A command variant applies to whichever model is selected.
        assert_eq!(
            resolve(
                json!({"agent":"reviewer","variant":"high"}),
                Some(&reviewer)
            )
            .unwrap(),
            Some("corp/agent#high".into())
        );
        assert_eq!(
            resolve(json!({"variant":"high"}), None).unwrap(),
            Some("corp/session#high".into())
        );
        let mut tuned = agent("reviewer", "subagent", Some("corp/agent"));
        tuned.variant = Some("low".into());
        assert_eq!(
            resolve(json!({"agent":"reviewer"}), Some(&tuned)).unwrap(),
            Some("corp/agent#low".into())
        );
    }

    /// A subagent-mode agent forces a child Session even without `subtask`; a
    /// different primary agent cannot take over the caller's inline Turn.
    #[test]
    fn subagent_agents_and_subtasks_fork_and_foreign_inline_agents_are_refused() {
        let plan = |value, profile: Option<&AgentProfile>| {
            command_overrides("cmd", &command(value), "build", profile, "corp/session")
        };
        let reviewer = agent("reviewer", "subagent", None);
        let forked = plan(json!({"agent":"reviewer"}), Some(&reviewer)).unwrap();
        assert!(forked.fork);
        assert_eq!(forked.agent.as_deref(), Some("reviewer"));
        assert_eq!(forked.activation.name, "cmd");
        assert!(forked.activation.allowed_tools.is_empty());
        assert!(forked.request.is_none() && forked.output_schema.is_none());
        let all = agent("scribe", "all", None);
        assert!(
            plan(json!({"agent":"scribe","subtask":true}), Some(&all))
                .unwrap()
                .fork
        );
        assert!(plan(json!({"subtask":true}), None).unwrap().fork);
        let inline = plan(json!({"model":"corp/fast"}), None).unwrap();
        assert!(!inline.fork);
        assert_eq!(inline.model.as_deref(), Some("corp/fast"));
        let build = agent("build", "primary", None);
        assert!(!plan(json!({"agent":"build"}), Some(&build)).unwrap().fork);
        let error = plan(json!({"agent":"scribe"}), Some(&all)).unwrap_err();
        assert!(error.to_string().contains("subtask"), "{error}");
        let lead = agent("lead", "primary", None);
        let error = plan(json!({"agent":"lead","subtask":true}), Some(&lead)).unwrap_err();
        assert!(error.to_string().contains("subagent"), "{error}");
    }
}
