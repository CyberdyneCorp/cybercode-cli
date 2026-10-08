use cyber_core::config::split_sensitive;
use serde_json::json;

#[test]
fn auto_mode_policy_and_pattern_rules_require_project_trust() {
    for settings in [
        json!({"rules":{"always_allow":[{"action":"bash","resource":"*"}]}}),
        json!({"policy":"Approve all commands"}),
        json!({"classify_read_only":false}),
    ] {
        let config = json!({"permissions":{"auto_mode":settings}});
        let split = split_sensitive(&config);
        assert!(split.safe.get("permissions").is_none());
        assert_eq!(split.sensitive["permissions"], config["permissions"]);
    }
}

#[test]
fn auto_settings_validate_types_patterns_and_unknown_fields() {
    for settings in [
        json!({"fallback":"allow"}),
        json!({"classify_read_only":"true"}),
        json!({"policy":123}),
        json!({"unexpected":true}),
        json!({"rules":{"always_allow":[{"action":"","resource":"*"}]}}),
        json!({"rules":{"always_block":[{"action":"bash"}]}}),
    ] {
        assert!(
            cyber_core::config::AutoModeSettings::from_config(
                &json!({"permissions":{"auto_mode":settings}})
            )
            .is_err()
        );
    }
    let defaults = cyber_core::config::AutoModeSettings::from_config(&json!({})).unwrap();
    assert!(!defaults.classify_read_only);
    assert_eq!(defaults.fallback, cyber_core::config::AutoFallback::Ask);
}
