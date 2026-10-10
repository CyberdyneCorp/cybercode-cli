use super::{BuiltinHost, Ctx};
use crate::permissions::{Decision, Request};
use cyber_server::runtime::{SkillSuggestion, ToolOutcome};

impl BuiltinHost {
    pub(super) fn path_skill_suggestions(
        &self,
        ctx: &Ctx<'_>,
        outcome: ToolOutcome,
    ) -> ToolOutcome {
        let paths = ctx
            .skill_paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if paths.is_empty() {
            return outcome;
        }
        let mut invocation = ctx.inv.clone();
        invocation.name = "skill".into();
        if self.check_agent_tool(&invocation).is_err() {
            return outcome;
        }
        let project = cyber_core::config::project_root(&ctx.location);
        let relative: Vec<_> = paths
            .iter()
            .filter_map(|path| path.strip_prefix(&project).ok())
            .collect();
        let skills: Vec<_> = self
            .skills(&ctx.location)
            .skills
            .into_values()
            .filter(|skill| {
                !skill.disable_model_invocation && matches_paths(&skill.paths, &relative)
            })
            .filter(|skill| {
                !matches!(
                    ctx.policy.decide(&Request {
                        tool: Some("skill".into()),
                        action: "skill".into(),
                        resources: vec![skill.name.clone()],
                        read_only: true,
                        ..Request::default()
                    }),
                    Decision::Deny { .. }
                )
            })
            .map(|skill| SkillSuggestion {
                name: skill.name,
                description: skill.description,
            })
            .collect();
        if skills.is_empty() {
            return outcome;
        }
        match outcome {
            ToolOutcome::Ok(output) => ToolOutcome::SkillSuggestions {
                output,
                value: None,
                skills,
            },
            ToolOutcome::Structured { output, value } => ToolOutcome::SkillSuggestions {
                output,
                value: Some(value),
                skills,
            },
            other => other,
        }
    }
}

fn matches_paths(patterns: &[String], paths: &[&std::path::Path]) -> bool {
    patterns
        .iter()
        .filter_map(|pattern| {
            globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .ok()
        })
        .any(|glob| {
            let matcher = glob.compile_matcher();
            paths.iter().any(|path| matcher.is_match(path))
        })
}
