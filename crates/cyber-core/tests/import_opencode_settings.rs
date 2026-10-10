use cyber_core::{config, import::opencode_settings_config};
use serde_json::json;

#[test]
fn static_commands_use_native_consumer_and_raw_alias_provenance() {
    for key in ["command", "commands"] {
        let mut source = json!({});
        source[key] = json!({"team/review":{"template":"Review $ARGUMENTS","description":"Review","argument_hint":"<target>"},
            "shell":{"template":"!`echo private-source`"},
            "override":{"template":"private-source","agent":"reviewer"},
            "files":{"template":"Review @private-source"}});
        let result = opencode_settings_config(&source).unwrap();
        let native = cyber_core::commands::configured(&result.config);
        assert_eq!(native.entries["team/review"].expand("code"), "Review code");
        assert_eq!(native.entries.len(), 1);
        assert_eq!(result.not_imported.len(), 3);
        assert_eq!(
            result.field_sources["/commands/team~1review/template"],
            vec![format!("/{key}/team~1review/template")]
        );
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("private-source")
        );
    }
    assert!(opencode_settings_config(&json!({"command":{},"commands":{}})).is_err());
    assert!(
        opencode_settings_config(
            &json!({"commands":{"review":{"template":"private-secret"}},"api_key":"private-secret"})
        )
        .is_err()
    );
}

#[test]
fn v1_agent_aliases_and_v2_defaults_materialize_native_profiles() {
    let v1=opencode_settings_config(&json!({"agent":{"team/review":{"description":"Review","prompt":"Review code only.","model":"corp/coder#deep","disable":false,"maxSteps":8,"hidden":true,"color":"#ff6b6b"}}})).unwrap();
    let profiles = config::resolve_agents(&v1.config).unwrap();
    let agent = &profiles["team/review"];
    assert_eq!(agent.mode, "all");
    assert_eq!(agent.system.as_deref(), Some("Review code only."));
    assert_eq!(agent.model.as_deref(), Some("corp/coder#deep"));
    assert_eq!(agent.steps, Some(8));
    assert!(agent.hidden);
    assert_eq!(
        v1.field_sources["/agents/team~1review/system"],
        vec!["/agent/team~1review/prompt"]
    );
    let v2=opencode_settings_config(&json!({"agents":{"review":{"system":"Review only.","model":{"providerID":"corp","model":"coder/v1","variant":"deep"}},"helper":{"mode":"subagent","disabled":true}}})).unwrap();
    let profiles = config::resolve_agents(&v2.config).unwrap();
    assert_eq!(profiles["review"].mode, "primary");
    assert_eq!(
        profiles["review"].model.as_deref(),
        Some("corp/coder/v1#deep")
    );
    assert!(!profiles.contains_key("helper"));
}

#[test]
fn agent_permissions_share_source_conversion_and_exact_rule_provenance() {
    let result=opencode_settings_config(&json!({"agent":{"review":{"tools":{"write":false},"permission":{"read":"allow","bash":{"git status":"allow","git push *":"deny"}}}}})).unwrap();
    let rules = result.config["agents"]["review"]["permissions"]["rules"]
        .as_array()
        .unwrap();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[0]["action"], "edit");
    assert_eq!(rules[0]["effect"], "deny");
    assert_eq!(rules[3]["resource"], "git push **");
    assert_eq!(
        result.field_sources["/agents/review/permissions/rules/3/effect"],
        vec!["/agent/review/permission/bash/git push *"]
    );
}

#[test]
fn explicit_compaction_maps_legacy_and_current_tokens_without_default_inference() {
    for (source, pointer) in [
        (
            json!({"compaction":{"auto":false,"buffer":1000,"preserve_recent_tokens":0}}),
            "/compaction/preserve_recent_tokens",
        ),
        (
            json!({"compaction":{"auto":false,"buffer":1000,"keep":{"tokens":0}}}),
            "/compaction/keep/tokens",
        ),
    ] {
        let result = opencode_settings_config(&source).unwrap();
        assert_eq!(
            result.config,
            json!({"compaction":{"auto":false,"buffer":1000,"keep":{"tokens":0}}})
        );
        assert_eq!(
            result.field_sources["/compaction/keep/tokens"],
            vec![pointer]
        );
    }
}

#[test]
fn unsupported_agent_and_compaction_fields_remain_pending_without_values() {
    let result=opencode_settings_config(&json!({"agents":{"explore":{"system":"private-source-system"},"review":{"system":"{file:private-prompt}","request":{"headers":{"X-Key":"private-header"}},"temperature":0.1}},"compaction":{"prune":"private-prune","keep":{"turns":3}}})).unwrap();
    assert_eq!(result.not_imported.len(), 6);
    assert!(result.config.get("compaction").is_none());
    assert!(result.config["agents"].get("explore").is_none());
    assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
}

#[test]
fn ambiguous_malformed_and_credential_echoes_fail_without_values() {
    for source in [
        json!({"agent":{},"agents":{}}),
        json!({"agents":{"review":{"prompt":"one","system":"two"}}}),
        json!({"agents":{"review":{"disabled":"private-secret"}}}),
        json!({"agents":{"review":{"steps":0}}}),
        json!({"agents":{"review":{"model":"private-secret"}}}),
        json!({"agents":{"review":{"model":{"providerID":"corp","model":"coder","extra":"private-secret"}}}}),
        json!({"agents":{"max_concurrent":{}}}),
        json!({"agents":{"review":{"permissions":[{"action":"unknown","resource":"private-secret","effect":"deny"}]}}}),
        json!({"agents":{"review":{"system":"{env:PRIVATE}"}}}),
        json!({"agents":{"review":{"system":"private-secret"}},"providers":{"corp":{"settings":{"apiKey":"private-secret"}}}}),
        json!({"agents":{"private-secret":{}},"api_key":"private-secret"}),
        json!({"compaction":{"keep":{"tokens":7},"preserve_recent_tokens":8}}),
        json!({"compaction":{"auto":"private-secret"}}),
        json!({"compaction":{"buffer":-1}}),
        json!({"compaction":{"preserve_recent_tokens":1.5}}),
    ] {
        let error = opencode_settings_config(&source).unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}

#[test]
fn agent_input_limits_refuse_before_native_materialization() {
    let agents: serde_json::Map<_, _> =
        (0..129).map(|i| (format!("agent{i}"), json!({}))).collect();
    assert!(opencode_settings_config(&json!({"agents":agents})).is_err());
    assert!(
        opencode_settings_config(&json!({"agents":{"review":{"description":"x".repeat(65537)}}}))
            .is_err()
    );
    assert!(opencode_settings_config(&json!({"agents":{"review":{"permissions":vec![json!({"action":"read","resource":"*","effect":"deny"});1024]}}})).is_err());
}

#[test]
fn imported_profile_rules_follow_native_project_trust_gating() {
    use cyber_core::{config::LoadRequest, paths::Paths, trust::TrustStore};
    use std::collections::HashMap;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        root.join("cyber-home").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, &root);
    let imported=opencode_settings_config(&json!({"agents":{"review":{"system":"Review code.","permissions":[{"action":"read","resource":"*","effect":"allow"}]}},"compaction":{"preserve_recent_tokens":12000}})).unwrap();
    std::fs::write(
        repo.join("cyber.jsonc"),
        serde_json::to_string(&imported.config).unwrap(),
    )
    .unwrap();
    let request = LoadRequest {
        location: &repo,
        paths: &paths,
        env: &env,
        home: &home,
        profile: None,
        overrides: &[],
        flags: json!({}),
    };
    let untrusted = config::load(&request).unwrap();
    assert!(
        untrusted
            .value
            .pointer("/agents/review/permissions")
            .is_none()
    );
    assert_eq!(
        config::resolve_agents(&untrusted.value).unwrap()["review"]
            .system
            .as_deref(),
        Some("Review code.")
    );
    let report = config::trust_report(&request).unwrap();
    TrustStore::new(paths.trust_file())
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
    let trusted = config::load(&request).unwrap();
    assert_eq!(
        trusted
            .value
            .pointer("/agents/review/permissions/rules/0/effect"),
        Some(&json!("allow"))
    );
    assert_eq!(
        trusted.value.pointer("/compaction/keep/tokens"),
        Some(&json!(12000))
    );
}

#[test]
fn compaction_count_redaction_preserves_only_the_typed_native_count() {
    let input = json!({"compaction":{"keep":{"tokens":12000}},"token":"private-token","headers":{"compaction":{"keep":{"tokens":12000}}}});
    let redacted = config::redact_secrets(&input);
    assert_eq!(redacted["compaction"]["keep"]["tokens"], json!(12000));
    assert_eq!(redacted["token"], "***");
    assert_eq!(redacted["headers"]["compaction"]["keep"]["tokens"], "***");
    assert_eq!(
        config::redact_secrets(&json!({"compaction":{"keep":{"tokens":"private-token"}}}))["compaction"]
            ["keep"]["tokens"],
        "***"
    );
}
