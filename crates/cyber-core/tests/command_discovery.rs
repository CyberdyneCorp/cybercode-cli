use cyber_core::commands::{CommandScope, discover};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn fixture(root: &Path) -> CommandScope {
    let project = root.join("repo");
    let location = project.join("nested");
    let home = root.join("home");
    let global = home.join(".config/cyber");
    std::fs::create_dir_all(&location).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    git2::Repository::init(&project).unwrap();
    CommandScope {
        location,
        home,
        global_config_dir: global,
        project: true,
        compat: true,
    }
}
fn write(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn native_and_compat_files_follow_scope_precedence_and_release_handles() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let project = root.join("repo");
    write(
        &scope.home,
        ".claude/commands/review.md",
        "Global compatibility",
    );
    write(
        &scope.global_config_dir,
        "command/review.md",
        "Global native",
    );
    write(
        &project,
        ".claude/commands/review.md",
        "Ancestor compatibility",
    );
    write(&project, ".cyber/commands/review.md", "Ancestor native");
    write(
        &scope.location,
        ".cyber/commands/review.md",
        "Nearest native $ARGUMENTS",
    );
    write(
        &scope.location,
        ".cyber/command/db/migrate.sql.md",
        "---\ndescription: Migration\nargument-hint: '<step>'\n---\nMigrate $1",
    );
    write(&scope.location, ".cyber/commands/goal.md", "Project goal");
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.entries["review"].expand("src"), "Nearest native src");
    assert_eq!(result.entries["db/migrate.sql"].expand("42"), "Migrate 42");
    assert_eq!(
        result.entries["db/migrate.sql"].argument_hint.as_deref(),
        Some("<step>")
    );
    assert!(result.entries.contains_key("project:goal") && !result.entries.contains_key("goal"));
    std::fs::rename(&project, root.join("moved-repo")).unwrap();
    assert_eq!(
        result.entries["review"].expand("old observation"),
        "Nearest native old observation"
    );
}

#[test]
fn source_attribution_places_inline_values_and_unsupported_winners_without_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let config_path = write(&scope.global_config_dir, "cyber.json", "{}");
    let config = json!({"commands":{"review":{"template":"Global inline"},"blocked":{"template":"Global fallback"}}});
    let sources = BTreeMap::from([
        (
            "/commands/review/template".into(),
            config_path.display().to_string(),
        ),
        (
            "/commands/blocked/template".into(),
            config_path.display().to_string(),
        ),
    ]);
    write(
        &scope.location,
        ".claude/commands/review.md",
        "Project compatibility",
    );
    write(
        &scope.location,
        ".cyber/commands/blocked.md",
        "---\nagent: reviewer\n---\nprivate-unsupported-body",
    );
    let result = discover(&scope, &config, &sources);
    assert_eq!(result.entries["review"].template, "Project compatibility");
    assert!(!result.entries.contains_key("blocked"));
    assert_eq!(result.unavailable, ["blocked"]);
    assert!(result.issues.iter().all(|i| !i.reason.contains("private-")));
    let result = discover(&scope, &config, &BTreeMap::new());
    assert_eq!(result.entries["review"].template, "Global inline");
    assert_eq!(result.entries["blocked"].template, "Global fallback");
}

#[test]
fn disabled_project_and_compat_scopes_preserve_global_native_commands() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut scope = fixture(&root);
    write(
        &scope.global_config_dir,
        "commands/native.md",
        "Global native",
    );
    write(
        &scope.home,
        ".claude/commands/home.md",
        "Home compatibility",
    );
    write(
        &scope.location,
        ".claude/commands/project.md",
        "Project compatibility",
    );
    write(&scope.location, ".cyber/commands/local.md", "Local native");
    scope.project = false;
    scope.compat = false;
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert_eq!(
        result.entries.keys().cloned().collect::<Vec<_>>(),
        ["native"]
    );
}

#[test]
fn hard_links_and_advanced_definitions_never_return_body_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let secret = write(&root, "private.md", "private-source-body");
    let command = scope.location.join(".cyber/commands/private.md");
    std::fs::create_dir_all(command.parent().unwrap()).unwrap();
    std::fs::hard_link(secret, command).unwrap();
    write(
        &scope.location,
        ".cyber/commands/shell.md",
        "!`touch sentinel`",
    );
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert!(result.entries.is_empty());
    assert_eq!(result.unavailable, ["private", "shell"]);
    assert!(
        result
            .issues
            .iter()
            .all(|i| !i.reason.contains("private-source-body") && !i.reason.contains("sentinel"))
    );
    assert!(!scope.location.join("sentinel").exists());
}

#[test]
fn registry_and_depth_limits_refuse_partial_command_catalogues() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    for index in 0..129 {
        write(
            &scope.location,
            &format!(".cyber/commands/cmd{index}.md"),
            "Review",
        );
    }
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert!(result.entries.is_empty() && result.unavailable.len() == 129);
    std::fs::remove_dir_all(scope.location.join(".cyber/commands")).unwrap();
    let deep = format!(".cyber/commands/{}/review.md", vec!["nested"; 33].join("/"));
    write(&scope.location, &deep, "Review");
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert!(result.entries.is_empty());
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.reason.contains("depth bound"))
    );
}

#[cfg(unix)]
#[test]
fn linked_command_tree_refuses_before_listing_or_reading_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let outside = root.join("outside");
    write(&outside, "hidden-name.md", "private-outside-body");
    let commands = scope.location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::os::unix::fs::symlink(&outside, commands.join("linked")).unwrap();
    let result = discover(
        &scope,
        &json!({"commands":{"safe":{"template":"Safe"}}}),
        &BTreeMap::new(),
    );
    assert!(result.entries.is_empty());
    assert!(
        result
            .issues
            .iter()
            .all(|i| !i.path.ends_with("hidden-name.md")
                && !i.reason.contains("private-outside-body"))
    );
}

#[cfg(windows)]
#[test]
fn command_junction_refuses_before_listing_or_reading_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let outside = root.join("outside");
    write(&outside, "hidden-name.md", "private-outside-body");
    let commands = scope.location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(commands.join("linked"))
        .arg(outside)
        .output()
        .unwrap();
    assert!(output.status.success());
    let result = discover(&scope, &json!({}), &BTreeMap::new());
    assert!(result.entries.is_empty());
    assert!(
        result
            .issues
            .iter()
            .all(|i| !i.path.ends_with("hidden-name.md")
                && !i.reason.contains("private-outside-body"))
    );
}

#[test]
fn entry_and_aggregate_byte_limits_refuse_even_explicit_inline_fallbacks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let commands = scope.location.join(".cyber/commands");
    std::fs::create_dir_all(&commands).unwrap();
    for index in 0..4097 {
        std::fs::write(commands.join(format!("ignored{index}.txt")), "").unwrap();
    }
    let config = json!({"commands":{"safe":{"template":"Safe"}}});
    let result = discover(&scope, &config, &BTreeMap::new());
    assert!(result.entries.is_empty());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.reason.contains("entry bound"))
    );
    std::fs::remove_dir_all(&commands).unwrap();
    // Unsupported documents still consume the shared read budget.
    let body = "x".repeat(1024 * 1024 - 1);
    for index in 0..17 {
        write(&commands, &format!("large{index}.md"), &body);
    }
    let result = discover(&scope, &config, &BTreeMap::new());
    assert!(result.entries.is_empty());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.reason.contains("aggregate byte bound"))
    );
}

#[test]
fn provenance_tracks_scope_labelled_winners_shadowed_sources_and_unavailable_files() {
    use cyber_core::commands::{CommandSourceKind, CommandSourceScope};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let scope = fixture(&root);
    let global = write(&scope.global_config_dir, "cyber.json", "{}");
    let ancestor = write(&root.join("repo"), "cyber.json", "{}");
    let nearest = write(
        &scope.location,
        ".cyber/commands/review.md",
        "Nearest review",
    );
    let unavailable = write(
        &scope.location,
        ".cyber/commands/blocked.md",
        "---\nagent: private-agent\n---\nprivate-body",
    );
    let config = json!({"commands":{
        "review":{"template":"private-global-template"},
        "blocked":{"template":"private-ancestor-template"},
        "runtime":{"template":"Explicit override"}
    }});
    let sources = BTreeMap::from([
        (
            "/commands/review/template".into(),
            format!("global:{}", global.display()),
        ),
        (
            "/commands/blocked/template".into(),
            format!("project:{}", ancestor.display()),
        ),
        (
            "/commands/runtime/template".into(),
            "profile:private-profile".into(),
        ),
    ]);
    let result = discover(&scope, &config, &sources);
    assert_eq!(result.entries["review"].template, "Nearest review");
    assert!(!result.entries.contains_key("blocked"));
    let provenance = &result.provenance["review"];
    assert_eq!(provenance.winner.scope, CommandSourceScope::Project);
    assert_eq!(provenance.winner.kind, CommandSourceKind::NativeMarkdown);
    assert_eq!(provenance.winner.paths, [nearest]);
    assert_eq!(provenance.shadowed.len(), 1);
    assert_eq!(provenance.shadowed[0].scope, CommandSourceScope::Global);
    assert_eq!(provenance.shadowed[0].paths, [global]);
    assert_eq!(result.provenance["blocked"].winner.paths, [unavailable]);
    assert_eq!(result.provenance["blocked"].shadowed[0].paths, [ancestor]);
    assert_eq!(
        result.provenance["runtime"].winner.scope,
        CommandSourceScope::Runtime
    );
    assert!(result.provenance["runtime"].winner.paths.is_empty());
    let report = serde_json::to_string(&result.provenance).unwrap();
    for value in [
        "private-body",
        "private-global-template",
        "private-ancestor-template",
        "private-profile",
        "private-agent",
    ] {
        assert!(!report.contains(value));
    }
}

#[test]
fn explicit_project_layer_scope_wins_even_inside_the_global_config_directory() {
    use cyber_core::commands::CommandSourceScope;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut scope = fixture(&root);
    scope.global_config_dir = scope.location.join(".cyber");
    let path = write(&scope.global_config_dir, "cyber.json", "{}");
    let sources = BTreeMap::from([(
        "/commands/review/template".into(),
        format!("project:{}", path.display()),
    )]);
    let config = json!({"commands":{"review":{"template":"Project inline"}}});
    let result = discover(&scope, &config, &sources);
    assert_eq!(
        result.provenance["review"].winner.scope,
        CommandSourceScope::Project
    );
    scope.project = false;
    assert!(
        !discover(&scope, &config, &sources)
            .entries
            .contains_key("review")
    );
}
