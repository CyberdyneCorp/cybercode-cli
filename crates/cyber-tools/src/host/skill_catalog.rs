//! Skill inspection and the `core/skills` listing (`skills-commands` → Skill listing in system
//! context, Skill and command inspection).
use super::BuiltinHost;
use crate::permissions::{self, Effect};
use cyber_core::skills::{self, Discovery, SkillDiagnostic};
use cyber_server::http::{CommandDiagnostic, SkillCatalog, SkillInfo};
use std::path::Path;

const HEADER: &str = "Skills you can load with the skill tool:\n<available_skills>\n";
const FOOTER: &str = "\n</available_skills>";
/// Over budget, descriptions are cut to this many characters before names lose them.
const SHORT_DESCRIPTION: usize = 120;

impl BuiltinHost {
    /// Every discovered and skipped skill with the permission effect for `agent`.
    pub fn skill_catalog(&self, location: &Path, agent: &str) -> Result<SkillCatalog, String> {
        let rules = self.session_rules(location, Some(agent), &serde_json::Value::Null)?;
        let found = self.skills(location);
        let mut entries: Vec<SkillInfo> = found
            .skills
            .values()
            .map(|skill| loaded(skill, &rules))
            .collect();
        for diagnostic in &found.diagnostics {
            attach(&mut entries, diagnostic);
        }
        entries.sort_by(|a, b| (&a.name, &a.status).cmp(&(&b.name, &b.status)));
        let command_issues = self
            .command_registry(location)
            .issues
            .into_iter()
            .map(|issue| CommandDiagnostic {
                path: issue.path,
                reason: issue.reason.into(),
            })
            .collect();
        let preloads = self.agent_profile(location, agent)?.skills;
        Ok(SkillCatalog {
            agent: agent.into(),
            skills: entries,
            command_issues,
            preload_issues: preload(&found, &rules, &preloads).1,
        })
    }
}

/// An agent profile's `skills: [..]`: each named skill that is model-invocable and not
/// denied for the agent, framed as the skill tool loads it, and an issue for every other
/// name. Preloading never makes a skill the model could not load itself available.
pub(super) fn preload(
    found: &Discovery,
    rules: &[permissions::Rule],
    names: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut frames = Vec::new();
    let mut issues = Vec::new();
    for name in names {
        let issue = match found.skills.get(name) {
            None => "is not a discovered skill",
            Some(skill) if skill.disable_model_invocation => "disables model invocation",
            Some(_) if permissions::evaluate(rules, "skill", name).0 == Effect::Deny => {
                "is denied for this agent"
            }
            Some(skill) => {
                frames.push(skills::instruction_frame(skill, ""));
                continue;
            }
        };
        issues.push(format!("preloaded skill {name:?} {issue}"));
    }
    (frames, issues)
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Allow => "allow",
        Effect::Ask => "ask",
        Effect::Deny => "deny",
    }
}

fn loaded(skill: &skills::Skill, rules: &[permissions::Rule]) -> SkillInfo {
    SkillInfo {
        name: skill.name.clone(),
        description: skill.description.clone(),
        source: skill.source_label(),
        scope: skill.origin,
        status: "loaded".into(),
        user_invocable: skill.user_invocable,
        model_invocable: !skill.disable_model_invocation,
        argument_hint: skill
            .argument_hint
            .clone()
            .or_else(|| skills::derived_argument_hint(&skill.body)),
        permission: Some(effect_name(permissions::evaluate(rules, "skill", &skill.name).0).into()),
        diagnostics: Vec::new(),
    }
}

/// Duplicate notices join the winning entry; anything else is a skipped skill.
fn attach(entries: &mut Vec<SkillInfo>, diagnostic: &SkillDiagnostic) {
    if let Some(name) = &diagnostic.replaces
        && let Some(entry) = entries.iter_mut().find(|e| &e.name == name)
    {
        entry.diagnostics.push(diagnostic.to_string());
        return;
    }
    entries.push(SkillInfo {
        name: skipped_name(&diagnostic.path),
        description: String::new(),
        source: diagnostic.path.clone(),
        scope: diagnostic.origin,
        status: "invalid".into(),
        user_invocable: false,
        model_invocable: false,
        argument_hint: None,
        permission: None,
        diagnostics: vec![diagnostic.to_string()],
    });
}

/// `builtin:<name>`, `<dir>/SKILL.md` or `<dir>`: the directory name stands in for the name.
fn skipped_name(path: &str) -> String {
    if let Some(name) = path.strip_prefix("builtin:") {
        return name.into();
    }
    let path = Path::new(path);
    let dir = if path.file_name().is_some_and(|n| n == "SKILL.md") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Permitted, model-invocable skill names, sorted.
pub(super) fn permitted<'a>(
    found: &'a Discovery,
    rules: &[permissions::Rule],
) -> Vec<&'a skills::Skill> {
    found
        .skills
        .values()
        .filter(|s| !s.disable_model_invocation)
        .filter(|s| permissions::evaluate(rules, "skill", &s.name).0 != Effect::Deny)
        .collect()
}

/// `<available_skills>` for permitted, model-invocable skills. The whole block never exceeds
/// `budget` characters: over budget, descriptions are cut to 120 characters, then the
/// remaining skills are named without descriptions, then a `... N more` marker ends it.
pub(super) fn skill_listing(
    found: &Discovery,
    rules: &[permissions::Rule],
    budget: usize,
) -> Option<String> {
    let listed = permitted(found, rules);
    if listed.is_empty() {
        return None;
    }
    let available = budget.checked_sub(HEADER.len() + FOOTER.len())?;
    let full: Vec<String> = listed
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect();
    let lines = if joined_len(&full) <= available {
        full
    } else {
        shortened(&listed, available)
    };
    (!lines.is_empty()).then(|| format!("{HEADER}{}{FOOTER}", lines.join("\n")))
}

fn joined_len(lines: &[String]) -> usize {
    lines
        .iter()
        .map(|l| l.len() + 1)
        .sum::<usize>()
        .saturating_sub(1)
}

fn shortened(listed: &[&skills::Skill], available: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    let mut names_only = false;
    for (index, skill) in listed.iter().enumerate() {
        let remaining = listed.len() - index;
        // Room for the marker that would follow if a later skill does not fit.
        let reserve = if remaining > 1 {
            format!("\n... {} more", remaining - 1).len()
        } else {
            0
        };
        let separator = usize::from(!lines.is_empty());
        let short = format!("- {}: {}", skill.name, truncate(&skill.description));
        let name = format!("- {}", skill.name);
        let fits = |line: &str| used + separator + line.len() + reserve <= available;
        let line = if !names_only && fits(&short) {
            short
        } else if fits(&name) {
            names_only = true;
            name
        } else {
            let marker = format!("... {remaining} more");
            if used + separator + marker.len() <= available {
                lines.push(marker);
            }
            break;
        };
        used += separator + line.len();
        lines.push(line);
    }
    lines
}

fn truncate(text: &str) -> String {
    text.chars().take(SHORT_DESCRIPTION).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_core::skills::{Skill, SkillContext, SkillOrigin, SkillSource};

    fn skill(name: String, description: String) -> Skill {
        Skill {
            name,
            description,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            paths: Vec::new(),
            context: SkillContext::Inline,
            agent: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            argument_hint: None,
            source: SkillSource::Bundled,
            origin: SkillOrigin::Project,
            body: String::new(),
        }
    }

    fn many(count: usize, description: usize) -> Discovery {
        let mut found = Discovery::default();
        for i in 0..count {
            let s = skill(format!("skill-{i:03}"), "d".repeat(description));
            found.skills.insert(s.name.clone(), s);
        }
        found
    }

    #[test]
    fn listing_never_exceeds_its_character_budget() {
        let found = many(200, 400);
        for budget in [8000, 4000, 1000, 300, 120] {
            let text = skill_listing(&found, &[], budget).unwrap();
            assert!(text.len() <= budget, "{budget}: {}", text.len());
            assert!(text.starts_with(HEADER) && text.ends_with(FOOTER));
            assert!(text.contains(" more\n</available_skills>"), "{text}");
        }
        // Descriptions are cut to 120 characters before names lose them.
        let text = skill_listing(&found, &[], 8000).unwrap();
        assert!(text.contains(&format!("- skill-000: {}\n", "d".repeat(120))));
        assert!(!text.contains(&"d".repeat(121)));
        // Then the remaining skills are named without descriptions: 59 shortened lines fill
        // 8000 characters, so the 60th is named alone and nothing is cut.
        let text = skill_listing(&many(60, 120), &[], 8000).unwrap();
        assert!(text.len() <= 8000);
        assert!(text.contains(&format!("- skill-058: {}", "d".repeat(120))));
        assert!(text.contains("\n- skill-059\n"), "{text}");
        assert!(!text.contains("more"));
        let lines: Vec<_> = text.lines().filter(|l| l.starts_with("- ")).collect();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
        // Too small for even the frame: nothing is listed.
        assert!(skill_listing(&found, &[], 40).is_none());
    }

    #[test]
    fn listing_within_budget_keeps_full_descriptions() {
        let found = many(3, 300);
        let text = skill_listing(&found, &[], 8000).unwrap();
        assert_eq!(text.matches(&"d".repeat(300)).count(), 3);
        assert!(!text.contains("more"));
    }

    #[test]
    fn skipped_entries_take_their_directory_name() {
        assert_eq!(skipped_name("/r/.cyber/skills/deploy/SKILL.md"), "deploy");
        assert_eq!(skipped_name("/r/.cyber/skills/linked"), "linked");
        assert_eq!(skipped_name("builtin:review"), "review");
    }
}
