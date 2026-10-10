use cyber_core::import::codex_policy_config;
use serde_json::json;

#[test]
fn explicit_approval_and_sandbox_combinations_follow_the_canonical_mode_mapping() {
    for (approval, mode) in [
        ("on-request", "default"),
        ("never", "dont-ask"),
        ("untrusted", "default"),
    ] {
        for (source, policy) in [
            ("read-only", "read-only"),
            ("workspace-write", "workspace-write"),
            ("danger-full-access", "full-access"),
        ] {
            let mapped =
                codex_policy_config(&json!({"approval_policy":approval,"sandbox_mode":source}))
                    .unwrap();
            assert_eq!(mapped.config["sandbox"]["policy"], policy);
            assert_eq!(
                mapped.config["mode"],
                if policy == "full-access" {
                    "bypass"
                } else {
                    mode
                }
            );
            assert_eq!(mapped.deprecated_untrusted, approval == "untrusted");
        }
    }
}
#[test]
fn approval_defaults_are_only_introduced_for_explicit_source_policy() {
    let absent = codex_policy_config(&json!({"model":"coder"})).unwrap();
    assert_eq!(absent.config, json!({}));
    let explicit = codex_policy_config(&json!({"approval_policy":"never"})).unwrap();
    assert_eq!(
        explicit.config,
        json!({"mode":"dont-ask","sandbox":{"policy":"read-only"}})
    );
    let confined = codex_policy_config(&json!({"sandbox_mode":"workspace-write"})).unwrap();
    assert!(confined.config.get("mode").is_none());
}
#[test]
fn advanced_and_malformed_policy_forms_refuse_without_returning_source_values() {
    for value in [
        json!({"approval_policy":{"granular":{"rules":false}}}),
        json!({"approval_policy":"on-failure"}),
        json!({"approval_policy":"private-source-secret"}),
        json!({"approval_policy":null}),
        json!({"sandbox_mode":"private-source-secret"}),
        json!({"sandbox_mode":true}),
        json!({"sandbox_mode":"workspace-write","default_permissions":":workspace"}),
    ] {
        let error = codex_policy_config(&value).unwrap_err();
        assert!(!error.to_string().contains("private-source-secret"));
    }
}
