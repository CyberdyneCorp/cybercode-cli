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
