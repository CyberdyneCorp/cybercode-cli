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

#[test]
fn inherited_provider_leaves_and_credentials_keep_their_own_source_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".codex/config.toml",
        "model='first'\nmodel_provider='local'\n[model_providers.local]\nwire_api='chat'\nbase_url='http://localhost:8000/v1'\napi_key='private-source-key'\n",
    );
    write(&roots.directory, ".codex/config.toml", "model='second'\n");
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let view = preview.output();
    let model = view
        .report
        .iter()
        .find(|r| r.field == "/model" && r.status == "imported")
        .unwrap();
    assert!(
        model
            .sources
            .iter()
            .any(|s| s.source == roots.directory.join(".codex/config.toml") && s.field == "/model")
    );
    assert!(
        model
            .sources
            .iter()
            .any(|s| s.source == roots.home.join(".codex/config.toml")
                && s.field == "/model_provider")
    );
    let protocol = view
        .report
        .iter()
        .find(|r| r.field == "/providers/local/api/type")
        .unwrap();
    assert_eq!(protocol.sources.len(), 1);
    assert_eq!(
        protocol.sources[0].source,
        roots.home.join(".codex/config.toml")
    );
    assert_eq!(protocol.sources[0].field, "/model_providers/local/wire_api");
    let required = &view.required_environment[0];
    assert_eq!(required.sources.len(), 1);
    assert_eq!(
        required.sources[0].source,
        roots.home.join(".codex/config.toml")
    );
    assert_eq!(required.sources[0].field, "/model_providers/local/api_key");
    assert!(
        !serde_json::to_string(view)
            .unwrap()
            .contains("private-source-key")
    );
}

#[test]
fn appended_claude_permissions_and_sorted_codex_rules_keep_individual_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".claude/settings.json",
        r#"{"permissions":{"allow":[],"deny":["Bash(git push:*)"]}}"#,
    );
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(git status)"]}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let allow = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/0/resource")
        .unwrap();
    let deny = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/1/resource")
        .unwrap();
    assert_eq!(allow.sources.len(), 1);
    assert_eq!(
        allow.sources[0].source,
        roots.directory.join(".claude/settings.json")
    );
    assert_eq!(allow.sources[0].field, "/permissions/allow/0");
    assert_eq!(
        deny.sources[0].source,
        roots.home.join(".claude/settings.json")
    );
    write(
        &roots.home,
        ".codex/rules/a.rules",
        "prefix_rule(pattern=['git','push'], decision='forbidden')\n",
    );
    write(
        &roots.directory,
        ".codex/rules/b.rules",
        "prefix_rule(pattern=['git','status'], decision='allow')\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let allow = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/0/resource")
        .unwrap();
    let deny = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/1/resource")
        .unwrap();
    assert_eq!(
        allow.sources[0].source,
        roots.directory.join(".codex/rules/b.rules")
    );
    assert_eq!(
        deny.sources[0].source,
        roots.home.join(".codex/rules/a.rules")
    );
}

#[test]
fn ignored_permission_pattern_secrets_do_not_leak_through_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"permissions":{"rules":[]}}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"env":{"API_KEY":"private-source-key"},"permission":{"read":{"private-source-key":"deny"}}}"#,
    );
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    assert!(
        !serde_json::to_string(preview.output())
            .unwrap()
            .contains("private-source-key")
    );
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.status == "merged" && r.field == "/permissions/rules")
    );
}

#[test]
fn appended_selector_indices_and_pending_inherited_fields_reference_original_documents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Bash(git status)"]},"env":{"EXAMPLE":"global-only"}}"#,
    );
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Read(src/*)"]}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let second = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/1/resource")
        .unwrap();
    assert_eq!(second.sources[0].field, "/permissions/allow/0");
    assert_eq!(
        second.sources[0].source,
        roots.directory.join(".claude/settings.json")
    );
    let pending = preview
        .output()
        .report
        .iter()
        .find(|r| r.status == "not imported" && r.sources.iter().any(|s| s.field == "/env/EXAMPLE"))
        .unwrap();
    assert_eq!(pending.source, roots.home.join(".claude/settings.json"));
    assert!(
        !serde_json::to_string(preview.output())
            .unwrap()
            .contains("global-only")
    );
}

#[test]
fn source_provenance_limit_refuses_without_creating_native_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let source = serde_json::json!({"unsupported":vec!["small";16_385]}).to_string();
    write(&roots.directory, "opencode.json", &source);
    let error = preview_import(&roots, None, ImportScope::Project, &global(&root))
        .err()
        .unwrap();
    assert!(error.reason.contains("provenance"));
    assert!(!roots.directory.join("cyber.jsonc").exists());
}

#[test]
fn empty_converted_config_does_not_contaminate_codex_rule_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".codex/config.toml",
        "unknown_policy='future-policy'\n",
    );
    write(
        &roots.directory,
        ".codex/rules/git.rules",
        "prefix_rule(pattern=['git'], decision='prompt')\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let rule = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/permissions/rules/0/resource")
        .unwrap();
    assert!(
        rule.sources
            .iter()
            .all(|s| s.source == roots.directory.join(".codex/rules/git.rules"))
    );
    assert!(
        rule.sources
            .iter()
            .all(|s| s.field.starts_with("converted:"))
    );
}

#[test]
fn layered_codex_approval_and_sandbox_are_proposed_with_exact_field_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".codex/config.toml",
        "approval_policy='never'\nsandbox_mode='read-only'\n",
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "sandbox_mode='workspace-write'\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("dont-ask"));
    assert!(preview.output().diff.contains("workspace-write"));
    let policy = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/sandbox/policy")
        .unwrap();
    assert_eq!(
        policy.sources[0].source,
        roots.directory.join(".codex/config.toml")
    );
    assert_eq!(policy.sources[0].field, "/sandbox_mode");
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"mode":"plan","sandbox":{"policy":"read-only"}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.is_empty());
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.field == "/mode" && r.status == "merged")
    );
}

#[test]
fn retired_codex_approval_emits_a_note_and_unsupported_policy_refuses_the_whole_preview() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        ".codex/config.toml",
        "approval_policy='untrusted'\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.reason.contains("deprecation note"))
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "model='coder'\napproval_policy='private-source-secret'\n",
    );
    let error = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .err()
    .unwrap();
    assert!(!error.to_string().contains("private-source-secret"));
    assert!(!roots.directory.join("cyber.jsonc").exists());
}
