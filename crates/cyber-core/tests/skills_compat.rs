//! `compat-import` → Read-time skill compatibility: other tools' skill folders are read
//! in place, with a fixed precedence and a per-tool switch.
use cyber_core::skills::{SkillCompat, SkillScope, discover, roots};
use serde_json::json;
use std::path::{Path, PathBuf};

fn skill(dir: &Path, name: &str, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nBody\n"),
    )
    .unwrap();
}

fn scope(root: &Path, compat: SkillCompat) -> SkillScope {
    SkillScope {
        location: root.join("repo"),
        home: root.join("home"),
        global_config_dir: root.join("home/.config/cyber"),
        compat,
        extra: Vec::new(),
        plugins: Vec::new(),
    }
}

const FOLDERS: [&str; 5] = [
    ".cyber/skills",
    ".agents/skills",
    ".claude/skills",
    ".opencode/skills",
    ".codex/skills",
];

#[test]
fn duplicate_skill_precedence_follows_cyber_agents_claude_opencode_codex() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for folder in FOLDERS {
        skill(&repo.join(folder).join("deploy"), "deploy", folder);
    }
    // Remove the winner one folder at a time: the next folder in precedence order wins,
    // and the duplicate notice names both the winner and the folder it replaced.
    for (index, folder) in FOLDERS.iter().enumerate() {
        let found = discover(&scope(&root, SkillCompat::ALL));
        assert_eq!(found.skills["deploy"].description, *folder);
        if let Some(loser) = FOLDERS.get(index + 1) {
            let winner: PathBuf = repo.join(folder).join("deploy");
            let notice = found
                .diagnostics
                .iter()
                .find(|d| {
                    d.replaces.as_deref() == Some("deploy")
                        && d.path == winner.display().to_string()
                })
                .unwrap_or_else(|| panic!("{:?}", found.diagnostics));
            assert!(
                notice
                    .message
                    .starts_with("duplicate skill: replaces skill deploy from ")
            );
            assert!(
                notice
                    .message
                    .contains(&repo.join(loser).join("deploy").display().to_string()),
                "{}",
                notice.message
            );
        }
        std::fs::remove_dir_all(repo.join(folder)).unwrap();
    }
}

#[test]
fn each_compat_folder_has_its_own_switch_and_skills_compat_turns_all_off() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for (folder, name) in [
        (".cyber/skills", "native"),
        (".agents/skills", "agents"),
        (".claude/skills", "claude"),
        (".opencode/skills", "opencode"),
    ] {
        skill(&repo.join(folder).join(name), name, folder);
    }
    skill(
        &root.join("home/.codex/skills/codex"),
        "codex",
        "user codex",
    );
    skill(
        &root.join("home/.claude/skills/user-claude"),
        "user-claude",
        "user claude",
    );
    let names = |config: serde_json::Value| {
        let found = discover(&scope(&root, SkillCompat::from_config(&config)));
        let mut names: Vec<_> = found
            .skills
            .into_iter()
            .filter(|(_, s)| !s.is_bundled())
            .map(|(name, _)| name)
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        names(json!({})),
        [
            "agents",
            "claude",
            "codex",
            "native",
            "opencode",
            "user-claude"
        ]
    );
    assert_eq!(
        names(json!({"compat":{"opencode":false}})),
        ["agents", "claude", "codex", "native", "user-claude"]
    );
    assert_eq!(
        names(json!({"compat":{"claude":false,"codex":false}})),
        ["agents", "native", "opencode"]
    );
    assert!(!names(json!({"compat":{"agents":false}})).contains(&"agents".to_string()));
    assert_eq!(names(json!({"skills":{"compat":false}})), ["native"]);
}

#[test]
fn user_cyber_skills_outrank_user_compat_folders() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("repo")).unwrap();
    skill(
        &root.join("home/.config/cyber/skills/notes"),
        "notes",
        "cyber",
    );
    skill(&root.join("home/.claude/skills/notes"), "notes", "claude");
    skill(&root.join("home/.agents/skills/notes"), "notes", "agents");
    let scope = scope(&root, SkillCompat::ALL);
    assert_eq!(discover(&scope).skills["notes"].description, "cyber");
    let home: Vec<_> = roots(&scope)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.starts_with(root.join("home")))
        .collect();
    assert_eq!(
        home,
        [
            root.join("home/.codex/skills"),
            root.join("home/.claude/skills"),
            root.join("home/.agents/skills"),
            root.join("home/.config/cyber/skills"),
        ]
    );
}
