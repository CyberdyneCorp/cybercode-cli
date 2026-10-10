use cyber_core::import::claude_permissions;
use serde_json::json;

#[test]
fn canonical_permission_example_and_group_order() {
    let plan = claude_permissions(&json!({"permissions":{
        "deny":["Read(./.env)"], "ask":["Bash(git push:*)"],
        "allow":["Bash(npm run test:*)", "Read"]
    }}))
    .unwrap();
    assert_eq!(
        serde_json::to_value(plan.rules).unwrap(),
        json!([
            {"action":"bash","resource":"npm run test *","effect":"allow"},
            {"action":"read","resource":"*","effect":"allow"},
            {"action":"bash","resource":"git push *","effect":"ask"},
            {"action":"read","resource":".env","effect":"deny"}
        ])
    );
}

#[test]
fn mode_aliases_preserve_configured_intent() {
    for (source, target) in [
        ("default", "default"),
        ("acceptEdits", "accept-edits"),
        ("plan", "plan"),
        ("auto", "auto"),
        ("dontAsk", "dont-ask"),
        ("bypassPermissions", "bypass"),
    ] {
        assert_eq!(
            claude_permissions(&json!({"permissions":{"defaultMode":source}}))
                .unwrap()
                .mode
                .as_deref(),
            Some(target)
        );
    }
    assert!(claude_permissions(&json!({})).unwrap().mode.is_none());
}

#[test]
fn a_rejected_rule_refuses_the_batch_without_echoing_source_secrets() {
    for value in [
        json!(null),
        json!("secret"),
        json!({"deny":"private-token"}),
        json!({"deny":[42]}),
        json!({"deny":["UnknownTool(private-token)"]}),
        json!({"deny":["Read(private-token"]}),
        json!({"deny":["Read()"]}),
        json!({"deny":["Bash(:*)"]}),
        json!({"deny":["Read(.)\nprivate-token"]}),
        json!({"defaultMode":"private-token"}),
    ] {
        let result = claude_permissions(&json!({"permissions":value}));
        let error = result.unwrap_err().to_string();
        assert!(error.starts_with("permissions"));
        assert!(!error.contains("private-token"));
    }
    let error = claude_permissions(
        &json!({"permissions":{"allow":["Bash"],"deny":["UnknownTool(private-token)"]}}),
    )
    .unwrap_err();
    assert_eq!(error.field, "permissions.deny[0]");
}

#[test]
fn command_arguments_are_retained_without_shell_evaluation() {
    let plan = claude_permissions(
        &json!({"permissions":{"allow":["Bash(echo 'value(with parentheses)')"]}}),
    )
    .unwrap();
    assert_eq!(plan.rules[0].resource, "echo 'value(with parentheses)'");
}

#[test]
fn distinct_edit_selectors_retain_exact_native_tool_scope() {
    for (tool, native) in [
        ("Edit", "edit"),
        ("Write", "write"),
        ("MultiEdit", "edit"),
        ("NotebookEdit", "notebook_edit"),
    ] {
        let selector = format!("{tool}(./src/*.rs)");
        let converted = claude_permissions(&json!({"permissions":{"allow":[selector]}})).unwrap();
        assert_eq!(converted.rules.len(), 1);
        assert_eq!(converted.rules[0].action, "edit");
        assert_eq!(converted.rules[0].resource, "src/*.rs");
        assert_eq!(converted.rules[0].tool.as_deref(), Some(native));
        assert_eq!(
            serde_json::to_value(&converted.rules).unwrap()[0]["tool"],
            native
        );
    }
}

#[test]
fn malformed_settings_cannot_appear_to_be_an_empty_policy() {
    for value in [json!(null), json!([]), json!("private-token")] {
        let error = claude_permissions(&value).unwrap_err();
        assert_eq!(error.field, "settings");
        assert!(!error.to_string().contains("private-token"));
    }
}

#[test]
fn opencode_legacy_tools_precede_written_permission_rules() {
    let settings: serde_json::Value = serde_json::from_str(
        r#"{
        "tools":{"shell":false,"write":false,"task":true},
        "permission":{"bash":{"*":"ask","git *":"allow","git push *":"deny"},"read":"allow"}
    }"#,
    )
    .unwrap();
    let rules = cyber_core::import::opencode_permissions(&settings).unwrap();
    assert_eq!(
        serde_json::to_value(rules).unwrap(),
        json!([
            {"action":"bash","resource":"*","effect":"deny"},
            {"action":"edit","resource":"*","effect":"deny"},
            {"action":"agent","resource":"*","effect":"allow"},
            {"action":"bash","resource":"*","effect":"ask"},
            {"action":"bash","resource":"git **","effect":"allow"},
            {"action":"bash","resource":"git push **","effect":"deny"},
            {"action":"read","resource":"*","effect":"allow"}
        ])
    );
}

#[test]
fn opencode_ordered_arrays_and_aliases_retain_source_positions() {
    let rules = cyber_core::import::opencode_permissions(&json!({"permissions":[
        {"action":"*","resource":"*","effect":"ask"},
        {"action":"patch","resource":"src/*","effect":"deny"},
        {"action":"subagent","resource":"review","effect":"allow"},
        {"action":"shell","resource":"echo ?","effect":"allow"}
    ]}))
    .unwrap();
    assert_eq!(
        rules.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(),
        ["*", "edit", "agent", "bash"]
    );
    assert!(rules.iter().all(|r| r.tool.is_none()));
    assert_eq!(rules[3].resource, "echo ?");
}

#[test]
fn opencode_global_effect_and_absent_permissions_are_explicit() {
    for effect in ["allow", "ask", "deny"] {
        let rules =
            cyber_core::import::opencode_permissions(&json!({"permission":effect})).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].effect, effect);
        assert_eq!(rules[0].action, "*");
    }
    assert!(
        cyber_core::import::opencode_permissions(&json!({"model":"private-value"}))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn opencode_invalid_or_unmapped_entries_refuse_the_whole_batch_safely() {
    for settings in [
        json!(null),
        json!([]),
        json!({"permission":"private-token"}),
        json!({"permission":null}),
        json!({"permission":{},"permissions":{}}),
        json!({"tools":{"bash":"private-token"}}),
        json!({"tools":{"private-token":false}}),
        json!({"permission":{"private-token":{}}}),
        json!({"permission":{"read":{"private-token":null}}}),
        json!({"permission":{"read":{"~/private-token":"deny"}}}),
        json!({"permission":{"read":{"$HOME/private-token":"deny"}}}),
        json!({"permission":{"read":{"private-token\\file":"deny"}}}),
        json!({"permissions":[{"action":"read","resource":"*","effect":"allow"},{"action":"private-token","resource":"*","effect":"deny"}]}),
        json!({"permissions":[{"action":"read","resource":"*","effect":"allow","private-token":true}]}),
        json!({"permissions":[{"action":"read","resource":"","effect":"deny"}]}),
        json!({"permissions":[{"action":"read","resource":"private-token\n","effect":"deny"}]}),
    ] {
        let error = cyber_core::import::opencode_permissions(&settings).unwrap_err();
        assert!(!error.to_string().contains("private-token"));
        assert!(error.to_string().len() < 180);
    }
}

#[test]
fn codex_prefix_rules_group_by_strength_and_expand_position_alternatives() {
    let rules = cyber_core::import::codex_prefix_rules(&json!([
        {"pattern":["git","push"],"decision":"forbidden"},
        {"pattern":["git",["fetch","pull"]],"decision":"prompt"},
        {"pattern":["git"]},
        {"pattern":["cargo","test"],"decision":"allow"}
    ]))
    .unwrap();
    assert_eq!(
        serde_json::to_value(rules).unwrap(),
        json!([
            {"action":"bash","resource":"git *","effect":"allow","argv_prefix":["git"]},
            {"action":"bash","resource":"cargo test *","effect":"allow","argv_prefix":["cargo","test"]},
            {"action":"bash","resource":"git fetch *","effect":"ask","argv_prefix":["git","fetch"]},
            {"action":"bash","resource":"git pull *","effect":"ask","argv_prefix":["git","pull"]},
            {"action":"bash","resource":"git push *","effect":"deny","argv_prefix":["git","push"]}
        ])
    );
}

#[test]
fn codex_prefix_conversion_refuses_unsafe_literal_tokens_without_echoing_them() {
    for token in [
        "",
        "private-token*",
        "private-token?",
        "private-token\\path",
        "private-token value",
        "private-token\n",
        "$(private-token)",
        "'private-token'",
        "private-token;",
        "private-token|",
        "private-tokené",
    ] {
        let error = cyber_core::import::codex_prefix_rules(&json!([
            {"pattern":["git"],"decision":"allow"},
            {"pattern":[token],"decision":"forbidden"}
        ]))
        .unwrap_err();
        assert_eq!(error.field, "rules[1].pattern[0]");
        assert!(!error.to_string().contains("private-token"));
    }
    for rule in [
        json!(null),
        json!({}),
        json!({"pattern":[]}),
        json!({"pattern":[[]]}),
        json!({"pattern":[["git",42]]}),
        json!({"pattern":["git"],"decision":null}),
        json!({"pattern":["git"],"decision":"private-token"}),
        json!({"pattern":["git"],"private-token":true}),
    ] {
        let error = cyber_core::import::codex_prefix_rules(&json!([rule])).unwrap_err();
        assert!(!error.to_string().contains("private-token"));
    }
}

#[test]
fn codex_prefix_expansion_is_bounded_before_allocating_the_cartesian_product() {
    let pattern = vec![json!(["a", "b"]); 13];
    assert!(
        cyber_core::import::codex_prefix_rules(&json!([{"pattern":pattern}]))
            .unwrap_err()
            .reason
            .contains("expansion limit")
    );
    let large_choices = vec!["a".repeat(4096); 257];
    assert!(
        cyber_core::import::codex_prefix_rules(&json!([{"pattern":[large_choices]}]))
            .unwrap_err()
            .reason
            .contains("text limit")
    );
    assert!(
        cyber_core::import::codex_prefix_rules(&json!([{"pattern":vec!["git";129]}]))
            .unwrap_err()
            .reason
            .contains("length limit")
    );
    assert!(
        cyber_core::import::codex_prefix_rules(&json!(vec![json!({"pattern":["git"]}); 4097]))
            .unwrap_err()
            .reason
            .contains("expansion limit")
    );
}
