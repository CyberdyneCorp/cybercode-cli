//! Instruction packages embedded in every binary; no installation writes are needed.
use super::{Discovery, SkillSource, parse};

const PACKAGES: &[(&str, &str)] = &[
    ("review", include_str!("bundled/review/SKILL.md")),
    ("batch", include_str!("bundled/batch/SKILL.md")),
    ("simplify", include_str!("bundled/simplify/SKILL.md")),
    (
        "security-review",
        include_str!("bundled/security-review/SKILL.md"),
    ),
    (
        "customize-cyber",
        include_str!("bundled/customize-cyber/SKILL.md"),
    ),
];

pub(super) fn discover() -> Discovery {
    static PACKAGED: std::sync::OnceLock<Discovery> = std::sync::OnceLock::new();
    PACKAGED.get_or_init(parse_packages).clone()
}

fn parse_packages() -> Discovery {
    let mut out = Discovery::default();
    for (name, text) in PACKAGES {
        match parse(text, SkillSource::Bundled) {
            Ok(skill) if skill.name == *name => {
                out.skills.insert(skill.name.clone(), skill);
            }
            Ok(_) => out
                .diagnostics
                .push(format!("builtin:{name}: package name mismatch")),
            Err(error) => out.diagnostics.push(format!("builtin:{name}: {error}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        Skill, SkillContext, SkillScope, discover as all, instruction_frame, sibling_files,
    };

    #[test]
    fn embedded_packages_parse_without_installation_or_filesystem_authority() {
        let found = discover();
        assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);
        assert_eq!(
            found.skills.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "batch",
                "customize-cyber",
                "review",
                "security-review",
                "simplify"
            ]
        );
        for skill in found.skills.values() {
            assert!(skill.is_bundled());
            assert!(skill.directory().is_none());
            assert!(skill.user_invocable && !skill.disable_model_invocation);
            assert!(sibling_files(skill, 20).is_empty());
            let frame = instruction_frame(skill, "test target");
            assert!(frame.contains(&format!("base=\"builtin:{}\"", skill.name)));
            assert!(frame.contains("test target"));
        }
        for name in ["review", "security-review", "simplify"] {
            let skill = &found.skills[name];
            assert_eq!(skill.context, SkillContext::Fork);
            assert_eq!(skill.agent.as_deref(), Some("reviewer"));
        }
        assert_eq!(found.skills["batch"].context, SkillContext::Inline);
        assert_eq!(
            found.skills["customize-cyber"].context,
            SkillContext::Inline
        );
    }

    #[test]
    fn local_packages_replace_every_embedded_default_and_invalid_overrides_are_diagnostic() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = SkillScope {
            location: tmp.path().join("repo"),
            home: tmp.path().join("home"),
            global_config_dir: tmp.path().join("global"),
            compat: false,
            extra: vec![],
        };
        let initial = all(&scope);
        assert_eq!(initial.skills.len(), 5);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
        for name in initial.skills.keys() {
            let dir = scope.global_config_dir.join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Local replacement\n---\nLOCAL {name}\n"),
            )
            .unwrap();
        }
        let local = all(&scope);
        assert_eq!(local.skills.len(), 5);
        assert!(
            local
                .skills
                .values()
                .all(|s| !s.is_bundled() && s.body.starts_with("LOCAL"))
        );
        assert_eq!(local.diagnostics.len(), 5);
        assert!(local.diagnostics.iter().all(|d| d.contains("builtin:")));
        std::fs::write(
            scope.global_config_dir.join("skills/review/SKILL.md"),
            "invalid document",
        )
        .unwrap();
        let invalid = all(&scope);
        assert!(invalid.skills["review"].is_bundled());
        assert!(invalid.diagnostics.iter().any(|d| {
            d.contains(
                &scope
                    .global_config_dir
                    .join("skills/review/SKILL.md")
                    .display()
                    .to_string(),
            ) && d.contains("frontmatter")
        }));
        assert_eq!(
            invalid
                .skills
                .values()
                .filter(|s| s.directory().is_some())
                .count(),
            4
        );
        assert!(initial.skills.values().all(Skill::is_bundled));
    }
}
