use cyber_core::import::{ImportScope, SourceRoots, SourceTool, preview_import};
use std::path::{Path, PathBuf};
fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn roots(root: &Path) -> SourceRoots {
    let project = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    SourceRoots {
        project_root: project.clone(),
        directory: project,
        home,
        codex_home: None,
    }
}
fn global(root: &Path) -> PathBuf {
    root.join("native-global")
}
#[test]
fn preview_auto_keeps_existing_model_and_first_source_rules_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"model":"native/model","providers":{"native":{"api":{"settings":{"api_key":"private-native-key"}}}}}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"first/model","permissions":{"read":"deny"}}"#,
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "model='coder'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:8000/v1'\napi_key='private-source-key'\n",
    );
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(git status)"]}}"#,
    );
    let original = std::fs::read(roots.directory.join("cyber.jsonc")).unwrap();
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    let output = preview.output();
    assert!(output.diff.contains("@@"));
    assert!(!output.diff.contains("private-"));
    assert!(output.report.iter().any(|r| r.status == "merged"
        && r.field == "/model"
        && r.source.ends_with("opencode.json")));
    assert_eq!(output.required_environment.len(), 1);
    assert!(!output.complete);
    assert_eq!(
        std::fs::read(roots.directory.join("cyber.jsonc")).unwrap(),
        original
    );
    assert!(!roots.directory.join(".cyber").exists());
    preview.verify().unwrap();
}
#[test]
fn codex_layers_inherit_provider_protocol_and_claude_denies_remain_strongest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".codex/config.toml",
        "model='first'\nmodel_provider='local'\n[model_providers.local]\nwire_api='chat'\nbase_url='http://localhost:8000/v1'\n",
    );
    write(&roots.directory, ".codex/config.toml", "model='second'\n");
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("local/second"));
    assert!(preview.output().diff.contains("openai-compatible"));
    write(
        &roots.home,
        ".claude/settings.json",
        r#"{"permissions":{"deny":["Bash(git push:*)"]}}"#,
    );
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(git:*)"]}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let diff = &preview.output().diff;
    assert!(
        diff.find("\"effect\": \"allow\"").unwrap() < diff.find("\"effect\": \"deny\"").unwrap()
    );
}
#[test]
fn changed_sources_destinations_and_new_native_files_invalidate_review() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"first/model"}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    write(&roots.directory, "cyber.jsonc", "{}");
    assert!(preview.verify().is_err());
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"changed/model"}"#,
    );
    assert!(preview.verify().is_err());
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    write(&roots.directory, "cyber.jsonc", r#"{"model":"new/native"}"#);
    assert!(preview.verify().is_err());
}
#[test]
fn executable_placeholders_and_declared_secrets_in_permission_patterns_refuse() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    for text in [
        r#"{"permissions":{"allow":["Read({file:/outside})"]}}"#,
        r#"{"permissions":{"allow":["Read({env:PRIVATE_TOKEN})"]}}"#,
        r#"{"env":{"API_KEY":"private-source-key"},"permissions":{"allow":["Bash(echo private-source-key)"]}}"#,
    ] {
        write(&roots.directory, ".claude/settings.json", text);
        let error = preview_import(&roots, None, ImportScope::Project, &global(&root))
            .err()
            .unwrap();
        assert!(!error.to_string().contains("private-source-key"));
    }
    assert!(!roots.directory.join("cyber.jsonc").exists());
}

#[test]
fn escaped_declared_secrets_are_redacted_from_unrecognized_native_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let secret = "private\"key\\with\nescapes";
    let native = serde_json::json!({"api_key": secret, "description": secret}).to_string();
    write(&roots.directory, "cyber.jsonc", &native);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"first/model"}"#,
    );
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    let diff = &preview.output().diff;
    let encoded = serde_json::to_string(secret).unwrap();
    assert!(!diff.contains(secret));
    assert!(!diff.contains(&encoded[1..encoded.len() - 1]));
    assert!(diff.contains("***"));
    assert_eq!(
        std::fs::read_to_string(roots.directory.join("cyber.jsonc")).unwrap(),
        native
    );
}

#[test]
fn global_scope_excludes_project_settings_and_rules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(&roots.home, ".codex/config.toml", "model='global-model'\n");
    write(
        &roots.directory,
        ".codex/config.toml",
        "model='project-model'\n",
    );
    write(
        &roots.directory,
        ".codex/rules/default.rules",
        "prefix_rule(pattern=['git'], decision='forbidden')\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Global,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("openai/global-model"));
    assert!(!preview.output().diff.contains("project-model"));
    assert!(!preview.output().diff.contains("argv_prefix"));
    assert!(
        preview
            .output()
            .report
            .iter()
            .all(|r| !r.source.starts_with(&roots.directory))
    );
    assert!(!global(&root).exists());
}

#[test]
fn native_global_ancestor_and_local_layers_keep_explicit_keys_in_runtime_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = roots(&root);
    let repo = roots.project_root.clone();
    roots.directory = repo.join("nested");
    std::fs::create_dir_all(&roots.directory).unwrap();
    let native_global = global(&root);
    write(
        &native_global,
        "cyber.jsonc",
        r#"{"model":"native/global","instructions":["global.md"]}"#,
    );
    write(
        &repo,
        "cyber.jsonc",
        r#"{"model":"native/root","instructions":["root.md"],"permissions":{"rules":[{"action":"read","resource":"*","effect":"deny"}]}}"#,
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"instructions":["nested.md"]}"#,
    );
    write(
        &repo,
        ".cyber/cyber.local.jsonc",
        r#"{"model":"native/local"}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"source/ignored","permissions":{"read":"allow"}}"#,
    );
    let preview = preview_import(&roots, None, ImportScope::Project, &native_global).unwrap();
    assert!(preview.output().diff.is_empty());
    assert_eq!(
        preview.output().native_layers,
        vec![
            native_global.join("cyber.jsonc"),
            repo.join("cyber.jsonc"),
            roots.directory.join("cyber.jsonc"),
            repo.join(".cyber/cyber.local.jsonc")
        ]
    );
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.field == "/model" && r.status == "merged")
    );
    write(
        &repo,
        ".cyber/cyber.local.jsonc",
        r#"{"model":"native/changed"}"#,
    );
    assert!(preview.verify().is_err());
}

#[test]
fn newly_appeared_ancestor_global_or_local_native_layers_invalidate_preview() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = roots(&root);
    let repo = roots.project_root.clone();
    roots.directory = repo.join("nested");
    std::fs::create_dir_all(&roots.directory).unwrap();
    let native_global = global(&root);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"source/model"}"#,
    );
    for path in [
        native_global.join("cyber.jsonc"),
        repo.join("cyber.json"),
        roots.directory.join(".cyber/cyber.local.jsonc"),
    ] {
        let preview = preview_import(&roots, None, ImportScope::Project, &native_global).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        assert!(preview.verify().is_err(), "{}", path.display());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn preview_global_scope_does_not_read_project_native_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        "malformed-project-configuration",
    );
    write(
        &roots.directory,
        ".cyber/cyber.local.jsonc",
        "also-malformed",
    );
    write(&roots.home, ".codex/config.toml", "model='coder'\n");
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Global,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("openai/coder"));
    assert!(preview.output().native_layers.is_empty());
    preview.verify().unwrap();
}

#[test]
fn native_layers_and_source_documents_share_one_aggregate_parse_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = roots(&root);
    let mut directory = roots.project_root.clone();
    let native = serde_json::json!({"description":"x".repeat(900_000)}).to_string();
    let source = serde_json::json!({"unsupported":"y".repeat(900_000)}).to_string();
    for _ in 0..7 {
        write(&directory, "cyber.json", &native);
        write(&directory, "cyber.jsonc", &native);
        write(&directory, ".claude/settings.json", &source);
        directory = directory.join("nested");
    }
    roots.directory = directory.parent().unwrap().to_owned();
    let error = preview_import(&roots, None, ImportScope::Project, &global(&root))
        .err()
        .unwrap();
    assert!(error.reason.contains("aggregate sixteen MiB"));
    assert!(!error.to_string().contains("yyyy"));
    assert!(!global(&root).exists());
}

#[cfg(unix)]
#[test]
fn linked_native_layers_refuse_without_returning_external_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &root,
        "outside/cyber.local.jsonc",
        r#"{"model":"outside/private-secret"}"#,
    );
    std::os::unix::fs::symlink(root.join("outside"), roots.directory.join(".cyber")).unwrap();
    let error = preview_import(&roots, None, ImportScope::Project, &global(&root))
        .err()
        .unwrap();
    assert!(error.reason.contains("linked native"));
    assert!(!error.to_string().contains("private-secret"));
}
