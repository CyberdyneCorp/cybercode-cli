mod support;
// Converted policies must retain actual model-tool authorization decisions.
use cyber_core::import::claude_permissions;
use cyber_tools::permissions::{Effect, evaluate, parse_rules};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn claude_precedence_is_preserved_in_the_shared_permission_engine() {
    let converted = claude_permissions(&json!({"permissions":{
        "allow":["Bash", "Read"],
        "ask":["Bash(git push:*)"],
        "deny":["Bash(git push --force:*)", "Read(./.env)"]
    }}))
    .unwrap();
    let rules = parse_rules(
        &serde_json::to_value(converted.rules).unwrap(),
        &BTreeMap::new(),
    );
    for (action, resource, effect) in [
        ("bash", "npm test", Effect::Allow),
        ("bash", "git push origin main", Effect::Ask),
        ("bash", "git push --force origin main", Effect::Deny),
        ("read", ".env", Effect::Deny),
        ("read", "src/lib.rs", Effect::Allow),
    ] {
        assert_eq!(evaluate(&rules, action, resource).0, effect);
    }
}

#[test]
fn deny_remains_stronger_when_source_object_key_order_is_different() {
    let converted = claude_permissions(&json!({"permissions":{
        "deny":["Read(./.env)"], "allow":["Read"], "ask":["Read(./.env)"]
    }}))
    .unwrap();
    let rules = parse_rules(
        &serde_json::to_value(converted.rules).unwrap(),
        &BTreeMap::new(),
    );
    assert_eq!(evaluate(&rules, "read", ".env").0, Effect::Deny);
}

#[tokio::test]
async fn converted_read_deny_blocks_actual_tools_even_in_bypass_mode() {
    let fixture = support::Fixture::new();
    fixture.write(".env", "private-value");
    fixture.write("public.txt", "public-value");
    let converted = claude_permissions(&json!({"permissions":{
        "allow":["Read"],"deny":["Read(./.env)"]
    }}))
    .unwrap();
    fixture.set_config(json!({"permissions":converted.rules}));
    for mode in ["default", "bypass"] {
        let refused = support::failed(fixture.call(mode, "read", json!({"path":".env"})).await);
        assert!(refused.contains("denied"));
        assert!(!refused.contains("private-value"));
    }
    let allowed = support::ok(
        fixture
            .call("default", "read", json!({"path":"public.txt"}))
            .await,
    );
    assert!(allowed.contains("public-value"));
}
