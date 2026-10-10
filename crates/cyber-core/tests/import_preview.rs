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

#[test]
fn mcp_file_preview_withholds_credentials_tracks_inputs_and_verifies_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        ".mcp.json",
        r#"{"mcpServers":{"audit":{"command":"server","env":{"API_KEY":"private-mcp-value"},"disabled":true,"custom":"private-unsupported"}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    assert!(output.diff.contains("server"));
    assert!(output.diff.contains("disabled"));
    let serialized = serde_json::to_string(output).unwrap();
    assert!(!serialized.contains("private-mcp-value"));
    assert!(!serialized.contains("private-unsupported"));
    assert_eq!(output.required_environment.len(), 1);
    assert!(
        output.required_environment[0]
            .sources
            .iter()
            .any(|s| s.source.ends_with(".mcp.json") && s.field == "/mcpServers/audit/env/API_KEY")
    );
    assert!(output.report.iter().any(|r| {
        r.status == "imported"
            && r.field == "/mcp/audit/command"
            && r.sources
                .iter()
                .any(|s| s.field == "/mcpServers/audit/command")
    }));
    assert!(output.report.iter().any(|r| {
        r.status == "not imported"
            && r.sources
                .iter()
                .any(|s| s.field == "/mcpServers/audit/custom")
    }));
    assert!(!roots.directory.join("cyber.jsonc").exists());
    preview.verify().unwrap();
    write(&roots.directory, ".mcp.json", r#"{"mcpServers":{}}"#);
    assert!(preview.verify().is_err());
}

#[test]
fn opencode_array_and_codex_server_previews_trace_actual_source_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"mcp":{"servers":{"audit":{"type":"local","command":["server","--stdio"],"environment":{"API_KEY":"private-mcp-value"},"enabled":false}}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    for (output, input) in [
        ("/mcp/audit/command", "/mcp/servers/audit/command/0"),
        ("/mcp/audit/args/0", "/mcp/servers/audit/command/1"),
        ("/mcp/audit/disabled", "/mcp/servers/audit/enabled"),
    ] {
        assert!(
            preview
                .output()
                .report
                .iter()
                .any(|r| r.field == output && r.sources.iter().any(|s| s.field == input))
        );
    }
    assert_eq!(
        preview.output().required_environment[0].sources[0].field,
        "/mcp/servers/audit/environment/API_KEY"
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "[mcp_servers.audit]\ncommand='server'\nstartup_timeout_sec=10\n[mcp_servers.audit.env]\nAPI_KEY='private-codex-mcp'\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("server"));
    assert!(preview.output().report.iter().any(|r| {
        r.status == "not imported"
            && r.sources
                .iter()
                .any(|s| s.field == "/mcp_servers/audit/startup_timeout_sec")
    }));
    assert_eq!(
        preview.output().required_environment[0].sources[0].field,
        "/mcp_servers/audit/env/API_KEY"
    );
}

#[test]
fn claude_global_state_does_not_import_unrelated_settings_or_project_associations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".claude.json",
        r#"{"model":"claude-unrelated","projects":{"elsewhere":{"mcpServers":{"wrong":{"command":"wrong"}}}},"mcpServers":{"audit":{"command":"server"}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Global,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.contains("server"));
    assert!(!preview.output().diff.contains("claude-unrelated"));
    assert!(!preview.output().diff.contains("wrong"));
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.reason.contains("project association"))
    );
}

#[test]
fn codex_remote_mcp_preview_attributes_static_and_environment_headers_to_their_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        ".codex/config.toml",
        "[mcp_servers.audit]\nurl='https://example.test/mcp'\nrequired=true\n[mcp_servers.audit.http_headers]\nAuthorization='Bearer private-token'\n[mcp_servers.audit.env_http_headers]\nX-Organization='ORGANIZATION'\n",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    assert_eq!(output.required_environment.len(), 2);
    for (variable, pointer) in [
        (
            "ORGANIZATION",
            "/mcp_servers/audit/env_http_headers/X-Organization",
        ),
        (
            "CYBER_IMPORT_MCP_CODEX_6175646974_HEADER_417574686F72697A6174696F6E",
            "/mcp_servers/audit/http_headers/Authorization",
        ),
    ] {
        assert!(
            output
                .required_environment
                .iter()
                .any(|r| r.requirement.variable == variable
                    && r.sources.iter().any(|s| s.field == pointer))
        );
    }
    assert!(output.report.iter().any(|r| {
        r.field == "/mcp/audit/required"
            && r.sources
                .iter()
                .any(|s| s.field == "/mcp_servers/audit/required")
    }));
    assert!(
        !serde_json::to_string(output)
            .unwrap()
            .contains("private-token")
    );
    assert!(!roots.directory.join("cyber.jsonc").exists());
}

#[test]
fn generated_mcp_bindings_cannot_collide_with_other_source_provider_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let source = serde_json::json!({"mcp":{"audit":{"type":"local","command":["server"],"environment":{"API_KEY":"private-mcp-key"}}}});
    let converted = cyber_core::import::mcp_config(SourceTool::OpenCode, &source).unwrap();
    let binding = &converted.required_environment[0].variable;
    write(
        &roots.directory,
        "opencode.json",
        &serde_json::to_string(&source).unwrap(),
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        &format!(
            "[model_providers.local]\nbase_url='https://example.test/v1'\nenv_key='{binding}'\n"
        ),
    );
    let error = match preview_import(&roots, None, ImportScope::Project, &global(&root)) {
        Ok(_) => panic!("colliding source bindings must refuse the preview"),
        Err(error) => error,
    };
    assert!(!error.to_string().contains("private-mcp-key"));
    assert!(!roots.directory.join("cyber.jsonc").exists());
}

#[test]
fn merged_model_report_shows_both_normalized_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"model":"native/kept"}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"source/ignored"}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let report = serde_json::to_value(&preview.output().report).unwrap();
    let model = report
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["field"] == "/model" && r["status"] == "merged")
        .unwrap();
    assert_eq!(model["comparison"]["kept"], "native/kept");
    assert_eq!(model["comparison"]["ignored"], "source/ignored");
    assert!(preview.output().diff.is_empty());
}

#[test]
fn merged_reports_redact_credential_fields_and_later_source_credential_echoes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"model":"native/private-later-value","providers":{"local":{"api":{"settings":{"api_key":"private-native-key"}},"request":{"headers":{"Authorization":"private-native-header"}}}}}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"source/ignored"}"#,
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "[model_providers.local]\nbase_url='https://example.test/v1'\napi_key='private-source-key'\n[model_providers.local.http_headers]\nAuthorization='private-source-header'\n",
    );
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"env":{"API_KEY":"private-later-value"}}"#,
    );
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    let output = serde_json::to_value(preview.output()).unwrap();
    let records = output["report"].as_array().unwrap();
    let credential = records
        .iter()
        .find(|r| r["field"] == "/providers/local/api/settings/api_key" && r["status"] == "merged")
        .unwrap();
    assert_eq!(credential["comparison"]["kept"], "***");
    assert_eq!(credential["comparison"]["ignored"], "***");
    let header = records
        .iter()
        .find(|r| {
            r["field"] == "/providers/local/request/headers/Authorization"
                && r["status"] == "merged"
        })
        .unwrap();
    assert_eq!(header["comparison"]["kept"], "***");
    assert_eq!(header["comparison"]["ignored"], "***");
    let model = records
        .iter()
        .find(|r| r["field"] == "/model" && r["status"] == "merged")
        .unwrap();
    assert_eq!(model["comparison"]["kept"], "native/***");
    assert_eq!(model["comparison"]["ignored"], "source/ignored");
    assert!(!output.to_string().contains("private-"));
}

#[test]
fn merged_permission_arrays_show_redacted_values_without_changing_kept_rules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let native = r#"{"permissions":{"rules":[{"action":"read","resource":"private-token","effect":"deny"}]}}"#;
    write(&roots.directory, "cyber.jsonc", native);
    write(
        &roots.directory,
        ".claude/settings.json",
        r#"{"permissions":{"allow":["Read(src/*)"]},"env":{"API_KEY":"private-token"}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let report = serde_json::to_value(&preview.output().report).unwrap();
    let rules = report
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["field"] == "/permissions/rules" && r["status"] == "merged")
        .unwrap();
    assert_eq!(rules["comparison"]["kept"][0]["resource"], "***");
    assert_eq!(rules["comparison"]["ignored"][0]["resource"], "src/*");
    assert!(preview.output().diff.is_empty());
    assert_eq!(
        std::fs::read_to_string(roots.directory.join("cyber.jsonc")).unwrap(),
        native
    );
}

#[test]
fn kept_mcp_credentials_do_not_require_setting_ignored_source_variables() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"mcp":{"audit":{"type":"local","command":"kept-server","args":[],"env":{"API_KEY":"{env:KEPT_KEY}"}}}}"#,
    );
    write(
        &roots.directory,
        ".mcp.json",
        r#"{"mcpServers":{"audit":{"command":"ignored-server","env":{"API_KEY":"private-ignored-key"}}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(preview.output().diff.is_empty());
    assert!(preview.output().required_environment.is_empty());
}

#[test]
fn generated_source_bindings_cannot_repurpose_native_environment_references() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let source = serde_json::json!({"mcpServers":{"audit":{"command":"server","env":{"API_KEY":"private-source-key"}}}});
    let converted = cyber_core::import::mcp_config(SourceTool::Claude, &source).unwrap();
    let variable = &converted.required_environment[0].variable;
    write(&roots.directory, "cyber.jsonc", &serde_json::to_string(&serde_json::json!({"description":format!("prefix {{env:{variable}:-fallback}} suffix")})).unwrap());
    write(&roots.directory, ".mcp.json", &source.to_string());
    assert!(
        preview_import(
            &roots,
            Some(SourceTool::Claude),
            ImportScope::Project,
            &global(&root)
        )
        .is_err()
    );
}

#[test]
fn partial_merges_only_report_accepted_environment_fields_and_coalesce_shared_variables() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"mcp":{"audit":{"type":"local","command":"kept-server","args":[],"env":{"IGNORED":"{env:KEPT_KEY}"}}}}"#,
    );
    write(
        &roots.directory,
        ".mcp.json",
        r#"{"mcpServers":{"audit":{"command":"ignored-server","env":{"IGNORED":"{env:SHARED}","ADDED":"{env:SHARED}"}},"other":{"command":"server","env":{"TOKEN":"{env:SHARED}"}}}}"#,
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        "[model_providers.local]\nbase_url='https://example.test/v1'\nenv_key='SHARED'\n",
    );
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    assert_eq!(preview.output().required_environment.len(), 1);
    let required = &preview.output().required_environment[0];
    assert_eq!(required.requirement.variable, "SHARED");
    assert!(!required.requirement.from_literal);
    let fields: Vec<_> = required.sources.iter().map(|s| s.field.as_str()).collect();
    assert_eq!(fields.len(), 3);
    assert!(fields.contains(&"/mcpServers/audit/env/ADDED"));
    assert!(fields.contains(&"/mcpServers/other/env/TOKEN"));
    assert!(fields.contains(&"/model_providers/local/env_key"));
    assert!(!fields.contains(&"/mcpServers/audit/env/IGNORED"));
    assert!(fields.contains(&required.requirement.field.as_str()));
}

#[test]
fn ignored_credential_bindings_do_not_block_unrelated_accepted_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let source = serde_json::json!({"mcpServers":{"audit":{"command":"server","env":{"API_KEY":"private-new-key"}}}});
    let mapped = cyber_core::import::mcp_config(SourceTool::Claude, &source).unwrap();
    let variable = &mapped.required_environment[0].variable;
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"providers":{"local":{"api":{"settings":{"api_key":"{env:KEPT_KEY}"}}}}}"#,
    );
    write(
        &roots.directory,
        ".codex/config.toml",
        &format!(
            "[model_providers.local]\nbase_url='https://example.test/v1'\nenv_key='{variable}'\n"
        ),
    );
    write(&roots.directory, ".mcp.json", &source.to_string());
    let preview = preview_import(&roots, None, ImportScope::Project, &global(&root)).unwrap();
    assert_eq!(preview.output().required_environment.len(), 1);
    assert_eq!(
        preview.output().required_environment[0]
            .requirement
            .variable,
        *variable
    );
    assert!(
        preview.output().required_environment[0]
            .requirement
            .from_literal
    );
}

#[test]
fn same_family_provider_and_mcp_collisions_are_checked_before_shared_setup_coalescing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    let mcp = serde_json::json!({"mcp_servers":{"audit":{"command":"server","env":{"API_KEY":"private-mcp-key"}}}});
    let mapped = cyber_core::import::mcp_config(SourceTool::Codex, &mcp).unwrap();
    let variable = &mapped.required_environment[0].variable;
    write(
        &roots.directory,
        ".codex/config.toml",
        &format!(
            "[model_providers.local]\nbase_url='https://example.test/v1'\nenv_key='{variable}'\n[mcp_servers.audit]\ncommand='server'\n[mcp_servers.audit.env]\nAPI_KEY='private-mcp-key'\n"
        ),
    );
    assert!(
        preview_import(
            &roots,
            Some(SourceTool::Codex),
            ImportScope::Project,
            &global(&root)
        )
        .is_err()
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"mcp":{"audit":{"env":{"API_KEY":"{env:KEPT_KEY}"}}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::Codex),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert_eq!(preview.output().required_environment.len(), 1);
    let requirement = &preview.output().required_environment[0];
    assert!(!requirement.requirement.from_literal);
    assert_eq!(
        requirement.requirement.field,
        "/model_providers/local/env_key"
    );
    assert_eq!(requirement.sources.len(), 1);
}

#[test]
fn opencode_provider_preview_tracks_escaped_model_fields_and_accepted_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"model":"corp/coder/v1","provider":{"corp":{"npm":"@ai-sdk/openai-compatible","options":{"apiKey":"private-source-key","headers":{"X-Account":"{env:ACCOUNT}"}},"models":{"coder/v1":{"name":"Coder","limit":{"context":8192}}}}}}"#,
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"providers":{"corp":{"api":{"settings":{"api_key":"{env:NATIVE_KEY}"}}}}}"#,
    );
    let original = std::fs::read(roots.directory.join("cyber.jsonc")).unwrap();
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    assert_eq!(output.required_environment.len(), 1);
    assert_eq!(
        output.required_environment[0].requirement.variable,
        "ACCOUNT"
    );
    assert_eq!(
        output.required_environment[0].sources[0].field,
        "/provider/corp/options/headers/X-Account"
    );
    assert!(output.report.iter().any(|r| {
        r.field == "/providers/corp/models/coder~1v1/name"
            && r.sources
                .iter()
                .any(|s| s.field == "/provider/corp/models/coder~1v1/name")
    }));
    assert!(
        !serde_json::to_string(output)
            .unwrap()
            .contains("private-source-key")
    );
    assert_eq!(
        std::fs::read(roots.directory.join("cyber.jsonc")).unwrap(),
        original
    );
    preview.verify().unwrap();
}

#[test]
fn accepted_variant_header_setup_uses_raw_array_indices_and_excludes_native_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        "opencode.json",
        r#"{"providers":{"corp":{"package":"@opencode/ai/providers/openai/chat","models":{"coder":{"headers":{"X-Team":"private-model"},"variants":[{"id":"deep","headers":{"X-Team":"private-variant"},"body":{"store":false}}]}}}}}"#,
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"providers":{"corp":{"models":{"coder":{"request":{"headers":{"X-Team":"{env:NATIVE_TEAM}"}}}}}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    assert_eq!(output.required_environment.len(), 1);
    assert_eq!(
        output.required_environment[0].sources[0].field,
        "/providers/corp/models/coder/variants/0/headers/X-Team"
    );
    assert!(output.report.iter().any(|r| {
        r.field == "/providers/corp/models/coder/variants/deep/request/body/store"
            && r.sources
                .iter()
                .any(|s| s.field == "/providers/corp/models/coder/variants/0/body/store")
    }));
    assert!(!serde_json::to_string(output).unwrap().contains("private-"));
    preview.verify().unwrap();
}

#[test]
fn layered_opencode_agents_append_permissions_with_original_indices_and_keep_native_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".config/opencode/opencode.json",
        r#"{"agents":{"review":{"description":"Global reviewer","permissions":[{"action":"shell","resource":"*","effect":"ask"}]}},"permissions":[{"action":"read","resource":"*","effect":"ask"}]}"#,
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"agents":{"review":{"system":"Review the code.","permissions":[{"action":"shell","resource":"git push *","effect":"deny"}]}},"permissions":[{"action":"read","resource":".env","effect":"deny"}],"compaction":{"preserve_recent_tokens":12000},"instructions":["private-instruction-path"],"commands":{"audit":{"template":"private-command-template"}}}"#,
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"agents":{"review":{"description":"Native reviewer"}},"compaction":{"keep":{"tokens":2000}}}"#,
    );
    let original = std::fs::read(roots.directory.join("cyber.jsonc")).unwrap();
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    let inherited = output
        .report
        .iter()
        .find(|r| r.field == "/agents/review/permissions/rules/0/effect")
        .unwrap();
    assert!(
        inherited
            .sources
            .iter()
            .any(|s| s.source.starts_with(&roots.home)
                && s.field == "/agents/review/permissions/0/effect")
    );
    let project = output
        .report
        .iter()
        .find(|r| r.field == "/agents/review/permissions/rules/1/effect")
        .unwrap();
    assert!(
        project
            .sources
            .iter()
            .any(|s| s.source.starts_with(&roots.directory)
                && s.field == "/agents/review/permissions/0/effect")
    );
    assert!(
        output
            .report
            .iter()
            .any(|r| r.field == "/permissions/rules/1/effect" && r.status == "imported")
    );
    assert!(
        output
            .report
            .iter()
            .any(|r| r.field == "/compaction/keep/tokens" && r.status == "merged")
    );
    assert!(!output.diff.contains("private-instruction-path"));
    assert!(output.diff.contains("private-command-template"));
    assert!(output.report.iter().any(|r| {
        r.field == "/commands/audit/template"
            && r.sources
                .iter()
                .any(|s| s.field == "/commands/audit/template")
    }));
    assert_eq!(
        std::fs::read(roots.directory.join("cyber.jsonc")).unwrap(),
        original
    );
    preview.verify().unwrap();
}

#[test]
fn markdown_commands_keep_body_frontmatter_provenance_and_source_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut roots = roots(&root);
    roots.directory = roots.project_root.join("nested");
    std::fs::create_dir_all(&roots.directory).unwrap();
    write(
        &roots.home,
        ".config/opencode/commands/db/migrate.sql.md",
        "Global migration $ARGUMENTS",
    );
    write(
        &roots.project_root,
        "opencode.json",
        r#"{"commands":{"db/migrate.sql":{"template":"Inline migration"},"inline-wins":{"template":"Project inline"}}}"#,
    );
    write(
        &roots.home,
        ".config/opencode/commands/inline-wins.md",
        "Global lower priority",
    );
    write(
        &roots.project_root,
        ".opencode/commands/db/migrate.sql.md",
        "Ancestor migration",
    );
    write(
        &roots.directory,
        ".opencode/commands/db/migrate.sql.md",
        "---\ndescription: Migration\nargument-hint: '<targets>'\n---\nNearest migration $ARGUMENTS",
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    let output = preview.output();
    assert!(output.diff.contains("Nearest migration"));
    assert!(output.diff.contains("Project inline"));
    assert!(
        !output.diff.contains("Global migration")
            && !output.diff.contains("Ancestor migration")
            && !output.diff.contains("Global lower priority")
    );
    let template = output
        .report
        .iter()
        .find(|r| r.field == "/commands/db~1migrate.sql/template")
        .unwrap();
    assert_eq!(template.sources[0].field, "body");
    assert_eq!(
        template.sources[0].source,
        roots.directory.join(".opencode/commands/db/migrate.sql.md")
    );
    let hint = output
        .report
        .iter()
        .find(|r| r.field == "/commands/db~1migrate.sql/argument_hint")
        .unwrap();
    assert_eq!(hint.sources[0].field, "frontmatter:argument-hint");
    preview.verify().unwrap();
    write(
        &roots.home,
        ".config/opencode/commands/inline-wins.md",
        "Changed lower priority source",
    );
    assert!(preview.verify().is_err());
    assert!(!output.complete);
}

#[test]
fn claude_markdown_commands_keep_native_values_and_pending_advanced_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        ".claude/commands/team/review.md",
        "---\ndescription: Reviewer\n---\nReview $ARGUMENTS",
    );
    write(
        &roots.directory,
        ".claude/commands/advanced.md",
        "---\nallowed-tools: bash\n---\nprivate-source-template",
    );
    write(
        &roots.directory,
        "cyber.jsonc",
        r#"{"commands":{"team/review":{"template":"Native review"}}}"#,
    );
    let before = std::fs::read(roots.directory.join("cyber.jsonc")).unwrap();
    let preview = preview_import(
        &roots,
        Some(SourceTool::Claude),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(!preview.output().diff.contains("private-source-template"));
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.source.ends_with("advanced.md") && r.status == "not imported")
    );
    let kept = preview
        .output()
        .report
        .iter()
        .find(|r| r.field == "/commands/team~1review/template")
        .unwrap();
    assert_eq!(kept.status, "merged");
    assert_eq!(kept.sources[0].field, "body");
    assert_eq!(
        std::fs::read(roots.directory.join("cyber.jsonc")).unwrap(),
        before
    );
    preview.verify().unwrap();
}

#[test]
fn markdown_command_credential_echo_and_combined_registry_bounds_refuse() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.directory,
        ".opencode/commands/review.md",
        "Review private-source-secret",
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"providers":{"corp":{"package":"@ai-sdk/openai","settings":{"apiKey":"private-source-secret"}}}}"#,
    );
    let error = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .err()
    .unwrap();
    assert!(!error.to_string().contains("private-source-secret"));
    std::fs::remove_file(roots.directory.join("opencode.json")).unwrap();
    for index in 0..128 {
        write(
            &roots.directory,
            &format!(".opencode/commands/cmd{index}.md"),
            "Review code",
        );
    }
    assert!(
        preview_import(
            &roots,
            Some(SourceTool::OpenCode),
            ImportScope::Project,
            &global(&root)
        )
        .is_err()
    );
}

#[test]
fn unsupported_higher_priority_commands_suppress_static_fallbacks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let roots = roots(&root);
    write(
        &roots.home,
        ".config/opencode/commands/suppressed.md",
        "Old global template",
    );
    write(
        &roots.directory,
        ".opencode/commands/suppressed.md",
        "---\nmodel: private-unsupported-model\n---\nprivate-new-body",
    );
    write(
        &roots.home,
        ".config/opencode/commands/inline-blocked.md",
        "Old fallback for unsupported inline",
    );
    write(
        &roots.directory,
        "opencode.json",
        r#"{"commands":{"inline-blocked":{"template":"private-inline-body","agent":"reviewer"}}}"#,
    );
    let preview = preview_import(
        &roots,
        Some(SourceTool::OpenCode),
        ImportScope::Project,
        &global(&root),
    )
    .unwrap();
    assert!(!preview.output().diff.contains("commands"));
    assert!(!preview.output().diff.contains("Old") && !preview.output().diff.contains("private-"));
    assert!(
        !preview
            .output()
            .report
            .iter()
            .any(|r| r.field.starts_with("/commands/") && r.status == "imported")
    );
    assert!(
        preview
            .output()
            .report
            .iter()
            .any(|r| r.source.ends_with("suppressed.md") && r.status == "not imported")
    );
    preview.verify().unwrap();
    write(
        &roots.directory,
        ".opencode/commands/ambiguous.md",
        "Plural command",
    );
    write(
        &roots.directory,
        ".opencode/command/ambiguous.md",
        "Legacy command",
    );
    assert!(
        preview_import(
            &roots,
            Some(SourceTool::OpenCode),
            ImportScope::Project,
            &global(&root)
        )
        .is_err()
    );
}
