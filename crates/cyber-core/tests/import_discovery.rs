use cyber_core::import::{SourceKind, SourceLayer, SourceRoots, SourceTool, discover_sources};
use std::path::{Path, PathBuf};

fn write(root: &Path, path: &str) -> PathBuf {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "unparsed private-value").unwrap();
    file
}
fn fixture(root: &Path) -> SourceRoots {
    let project = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(project.join("nested")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    SourceRoots {
        project_root: project.clone(),
        directory: project.join("nested"),
        home,
        codex_home: None,
    }
}

#[test]
fn inventory_retains_canonical_static_sources_layers_and_auto_order_without_parsing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = fixture(&root);
    for path in [
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".claude/agents/review.md",
        ".claude/commands/nested/check.md",
        ".claude/skills/deploy/SKILL.md",
        ".claude/skills/deploy/run.sh",
        ".claude/rules/test.md",
        ".claude/CLAUDE.md",
        "CLAUDE.md",
        ".mcp.json",
        ".codex/config.toml",
        ".codex/fast.config.toml",
        ".codex/agents/review.toml",
        ".codex/rules/git.rules",
        ".codex/hooks.json",
        ".codex/requirements.toml",
        "AGENTS.md",
        "AGENTS.override.md",
        ".agents/skills/shared/SKILL.md",
        "opencode.jsonc",
        ".opencode/agents/build.md",
        ".opencode/commands/test.md",
        ".opencode/modes/review.md",
        ".opencode/plugins/notify.ts",
        ".opencode/tools/custom.js",
        ".opencode/skills/local/SKILL.md",
        "nested/.codex/config.toml",
    ] {
        write(&roots.project_root, path);
    }
    for path in [
        ".claude/settings.json",
        ".claude.json",
        ".claude/agents/global.md",
        ".claude/rules/global.md",
        ".claude/skills/global/SKILL.md",
        ".codex/config.toml",
        ".codex/fast.config.toml",
        ".codex/AGENTS.md",
        ".codex/skills/native/SKILL.md",
        ".codex/rules/git.rules",
        ".codex/hooks.json",
        ".codex/requirements.toml",
        ".agents/skills/common/SKILL.md",
        ".config/opencode/opencode.json",
        ".config/opencode/plugins/global.ts",
    ] {
        write(&roots.home, path);
    }
    write(&roots.project_root, ".claude/agents/nested/ignored.md");
    let inventory = discover_sources(&roots).unwrap();
    assert!(inventory.issues.is_empty(), "{:?}", inventory.issues);
    assert_eq!(inventory.files.len(), 42);
    assert!(
        inventory
            .files
            .windows(2)
            .all(|pair| pair[0].tool <= pair[1].tool)
    );
    assert_file_categories(&inventory);
    let config: Vec<_> = inventory
        .files
        .iter()
        .filter(|f| f.tool == SourceTool::Codex && f.kind == SourceKind::Config)
        .collect();
    assert_eq!(config.len(), 3);
    assert_eq!(config[0].layer, SourceLayer::Global);
    assert_eq!(config[2].path, roots.directory.join(".codex/config.toml"));
    assert!(
        !inventory
            .files
            .iter()
            .any(|f| f.path.ends_with("ignored.md"))
    );
    assert_eq!(
        std::fs::read_to_string(roots.home.join(".codex/config.toml")).unwrap(),
        "unparsed private-value"
    );
    assert!(!roots.project_root.join(".cyber").exists());
    assert!(!roots.project_root.join("cyber.jsonc").exists());
    assert_eq!(inventory.files, discover_sources(&roots).unwrap().files);
}

#[test]
fn explicit_codex_home_retains_provenance_and_does_not_create_missing_locations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = fixture(&root);
    write(&roots.home, ".codex/config.toml");
    let custom = root.join("custom-codex");
    write(&custom, "config.toml");
    write(&custom, "work.config.toml");
    roots.codex_home = Some(custom.clone());
    let inventory = discover_sources(&roots).unwrap();
    assert_eq!(inventory.files.len(), 3);
    assert_eq!(inventory.files[0].layer, SourceLayer::Global);
    assert!(
        inventory.files[1..]
            .iter()
            .all(|f| f.layer == SourceLayer::CodexHome && f.path.starts_with(&custom))
    );
    roots.codex_home = Some(roots.home.join(".codex").canonicalize().unwrap());
    assert_eq!(discover_sources(&roots).unwrap().files.len(), 1);
    roots.codex_home = Some(root.join("missing-codex"));
    let inventory = discover_sources(&roots).unwrap();
    assert_eq!(inventory.files.len(), 1);
    assert!(!roots.codex_home.as_ref().unwrap().exists());
    roots.codex_home = Some(PathBuf::from("relative-codex"));
    assert!(
        discover_sources(&roots)
            .unwrap_err()
            .reason
            .contains("absolute")
    );
}

#[test]
fn discovery_refuses_foreign_locations_and_invalid_roots_before_returning_an_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = fixture(&root);
    roots.directory = roots.home.clone();
    assert!(
        discover_sources(&roots)
            .unwrap_err()
            .reason
            .contains("outside")
    );
    roots.directory = roots.project_root.clone();
    roots.home = root.join("missing-home");
    assert!(
        discover_sources(&roots)
            .unwrap_err()
            .reason
            .contains("unavailable")
    );
    assert!(!roots.home.exists());
}

#[test]
fn discovery_limits_refuse_a_partial_inventory_instead_of_hiding_unvisited_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = fixture(&root);
    let tree = roots.project_root.join(".opencode/plugins");
    let mut deep = tree.clone();
    for _ in 0..34 {
        deep = deep.join("deep");
    }
    write(&deep, "last.ts");
    assert!(
        discover_sources(&roots)
            .unwrap_err()
            .reason
            .contains("depth limit")
    );
    std::fs::remove_dir_all(&tree).unwrap();
    for i in 0..2100 {
        write(&roots.project_root, &format!(".opencode/plugins/{i:04}.ts"));
    }
    assert!(
        discover_sources(&roots)
            .unwrap_err()
            .reason
            .contains("entry limit")
    );
}

#[cfg(unix)]
#[test]
fn symbolic_files_trees_and_intermediate_source_roots_are_reported_without_following() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = fixture(&root);
    let outside = root.join("outside");
    write(&outside, "settings.json");
    write(&outside, "agents/secret.md");
    write(&outside, "git.rules");
    write(&outside, "script.ts");
    symlink(&outside, roots.project_root.join(".claude")).unwrap();
    std::fs::create_dir_all(roots.home.join(".codex")).unwrap();
    symlink(&outside, roots.home.join(".codex/rules")).unwrap();
    std::fs::create_dir_all(roots.project_root.join(".opencode/plugins")).unwrap();
    symlink(
        outside.join("script.ts"),
        roots.project_root.join(".opencode/plugins/linked.ts"),
    )
    .unwrap();
    let inventory = discover_sources(&roots).unwrap();
    assert!(inventory.files.is_empty());
    assert!(inventory.issues.iter().any(
        |i| i.path == roots.project_root.join(".claude") && i.reason.contains("symbolic link")
    ));
    assert!(
        inventory.issues.iter().any(
            |i| i.path == roots.home.join(".codex/rules") && i.reason.contains("symbolic link")
        )
    );
    assert!(
        inventory
            .issues
            .iter()
            .any(|i| i.path.ends_with("linked.ts") && i.reason.contains("symbolic link"))
    );
    assert!(!roots.project_root.join(".cyber").exists());
}

fn assert_file_categories(inventory: &cyber_core::import::SourceInventory) {
    assert!(
        inventory
            .files
            .iter()
            .any(|f| f.kind == SourceKind::SkillAsset && f.path.ends_with("run.sh"))
    );
    assert!(
        inventory
            .files
            .iter()
            .any(|f| f.kind == SourceKind::Plugin && f.layer == SourceLayer::Global)
    );
    assert!(
        inventory
            .files
            .iter()
            .any(|f| f.kind == SourceKind::CustomTool)
    );
}

#[cfg(windows)]
#[test]
fn windows_source_junctions_are_reported_without_inventorying_their_targets() {
    let temp = tempfile::tempdir().unwrap();
    let roots = fixture(temp.path());
    let outside = temp.path().join("outside");
    write(&outside, "settings.json");
    write(&outside, "config.toml");
    write(&outside, "script.ts");
    std::fs::create_dir_all(roots.project_root.join(".opencode")).unwrap();
    for path in [
        roots.project_root.join(".claude"),
        roots.home.join(".codex"),
        roots.project_root.join(".opencode").join("plugins"),
    ] {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&path)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let inventory = discover_sources(&roots).unwrap();
    assert!(inventory.files.is_empty());
    assert!(
        inventory
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".claude") && issue.reason.contains("reparse point"))
    );
    assert!(
        inventory
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".codex") && issue.reason.contains("reparse point"))
    );
    assert!(
        inventory
            .issues
            .iter()
            .any(|issue| issue.path.ends_with("plugins") && issue.reason.contains("reparse point"))
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("settings.json")).unwrap(),
        "unparsed private-value"
    );
}
