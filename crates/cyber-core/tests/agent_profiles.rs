//! Shared profile resolution for the catalogue and future child-runtime admission.

use cyber_core::config::resolve_agents;
use serde_json::json;

#[test]
fn builtins_keep_primary_subagent_and_internal_boundaries() {
    let profiles = resolve_agents(&json!({})).unwrap();
    assert_eq!(profiles.len(), 8);
    assert!(profiles["build"].primary_capable());
    assert!(!profiles["build"].subagent_capable());
    for name in ["explore", "general", "reviewer"] {
        assert!(profiles[name].subagent_capable());
        assert!(!profiles[name].primary_capable());
        assert_eq!(profiles[name].steps, Some(50));
    }
    assert!(profiles["explore"].read_only);
    assert!(
        profiles["explore"]
            .tools
            .allow
            .as_ref()
            .unwrap()
            .contains(&"bash".into())
    );
    assert_eq!(profiles["general"].tools.deny, ["todo"]);
    for name in ["compaction", "title", "summary", "evaluator"] {
        assert!(profiles[name].hidden);
        assert_eq!(profiles[name].tools.deny, ["*"]);
        assert_eq!(profiles[name].permissions, "deny");
    }
}

#[test]
fn model_only_and_nested_patches_retain_unspecified_builtin_fields() {
    let profiles = resolve_agents(&json!({"agents":{
        "max_concurrent":4, "max_depth":0, "result_max_bytes":0,
        "explore":{"model":"local/coder#fast"},
        "general":{"tools":{"allow":["read"]}},
        "reviewer":{"model":"local/audit","permission_mode":"bypass","tools":{"allow":["*"]}}
    }}))
    .unwrap();
    let baseline = resolve_agents(&json!({})).unwrap();
    assert_eq!(
        profiles["explore"].model.as_deref(),
        Some("local/coder#fast")
    );
    assert_eq!(
        profiles["explore"].tools.allow,
        baseline["explore"].tools.allow
    );
    assert!(profiles["explore"].read_only);
    assert_eq!(profiles["general"].tools.deny, ["todo"]);
    assert!(profiles["reviewer"].builtin && profiles["reviewer"].read_only);
    assert_eq!(
        profiles["reviewer"].permission_mode.as_deref(),
        Some("bypass")
    );
    assert_eq!(profiles["reviewer"].model.as_deref(), Some("local/audit"));
    assert_eq!(profiles.len(), 8);
}

#[test]
fn custom_profile_fields_are_materialized_with_all_mode_default() {
    let profiles = resolve_agents(&json!({"agents":{
        "review/security":{"description":"Review changes", "system":"Check security.",
            "model":"hosted/reviewer", "variant":"careful", "permission_mode":"plan",
            "tools":{"deny":["web*"]}, "steps":12, "memory":"project", "skills":["audit"],
            "mcp":["docs"], "request":{"body":{"temperature":0.2}}}
    }}))
    .unwrap();
    let custom = &profiles["review/security"];
    assert_eq!(custom.name, "review/security");
    assert!(!custom.builtin);
    assert_eq!(custom.mode, "all");
    assert!(custom.primary_capable() && custom.subagent_capable());
    assert_eq!(custom.system.as_deref(), Some("Check security."));
    assert_eq!(custom.variant.as_deref(), Some("careful"));
    assert_eq!(custom.permission_mode.as_deref(), Some("plan"));
    assert_eq!(custom.request["body"]["temperature"], 0.2);
    assert_eq!(custom.steps, Some(12));
    assert_eq!(custom.skills, ["audit"]);
    assert_eq!(custom.mcp.as_deref(), Some(["docs".to_string()].as_slice()));
    assert_eq!(profiles.len(), 9);
}

#[test]
fn disabled_profiles_disappear_but_system_profiles_cannot_gain_tools_or_visibility() {
    let profiles = resolve_agents(&json!({"agents":{
        "explore":{"disabled":true}, "custom":{"disabled":true},
        "title":{"hidden":false,"disabled":true,"mode":"all",
            "tools":{"allow":["bash"], "deny":[]},"permissions":"allow"}
    }}))
    .unwrap();
    assert!(!profiles.contains_key("explore"));
    assert!(!profiles.contains_key("custom"));
    let title = &profiles["title"];
    assert!(title.hidden);
    assert!(!title.primary_capable() && !title.subagent_capable());
    assert!(title.tools.allow.is_none());
    assert_eq!(title.tools.deny, ["*"]);
    assert_eq!(title.permissions, "deny");
}

#[test]
fn direct_resolver_call_does_not_bypass_loader_validation() {
    let error = resolve_agents(&json!({"agents":{"review":{"unknown":true}}})).unwrap_err();
    assert!(error.contains("agents.review.unknown"), "{error}");
}

#[test]
fn reviewer_defaults_keep_a_builtin_readonly_identity() {
    let mut profiles = resolve_agents(&json!({})).unwrap();
    let reviewer = profiles.remove("reviewer").unwrap();
    assert!(reviewer.builtin);
    assert!(reviewer.read_only);
    assert_eq!(reviewer.permission_mode.as_deref(), Some("plan"));
    assert_eq!(reviewer.model, None);
}
