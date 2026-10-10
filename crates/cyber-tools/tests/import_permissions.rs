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

#[tokio::test]
async fn write_allow_cannot_authorize_edit_or_patch_calls() {
    let fixture = support::Fixture::new();
    fixture.write("a.txt", "old");
    let converted =
        claude_permissions(&json!({"permissions":{"allow":["Read", "Write(./a.txt)"]}})).unwrap();
    fixture.set_config(json!({"permissions":converted.rules,"lsp":false,"formatters":false}));
    support::ok(
        fixture
            .call("default", "read", json!({"path":"a.txt"}))
            .await,
    );
    support::ok(
        fixture
            .call(
                "default",
                "write",
                json!({"path":"a.txt","content":"written"}),
            )
            .await,
    );
    let edit = support::failed(
        fixture
            .call(
                "default",
                "edit",
                json!({"path":"a.txt","old_string":"written","new_string":"edited"}),
            )
            .await,
    );
    assert!(edit.contains("no interactive approver"), "{edit}");
    let patch=support::failed(fixture.call("default","apply_patch",json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-written\n+patched\n*** End Patch"})).await);
    assert!(patch.contains("no interactive approver"), "{patch}");
    assert_eq!(fixture.read("a.txt"), "written");
}

#[tokio::test]
async fn scoped_write_deny_keeps_other_edit_tools_available() {
    let fixture = support::Fixture::new();
    fixture.write("a.txt", "old");
    let converted =
        claude_permissions(&json!({"permissions":{"allow":["Read", "Edit"],"deny":["Write"]}}))
            .unwrap();
    fixture.set_config(json!({"permissions":converted.rules,"lsp":false,"formatters":false}));
    let tools = fixture.tool_names("default", false);
    assert!(!tools.contains(&"write".into()));
    assert!(tools.contains(&"edit".into()));
    assert!(tools.contains(&"notebook_edit".into()));
    support::ok(
        fixture
            .call("default", "read", json!({"path":"a.txt"}))
            .await,
    );
    for mode in ["default", "bypass", "accept-edits", "auto", "dont-ask"] {
        let denied = support::failed(
            fixture
                .call(mode, "write", json!({"path":"a.txt","content":"forbidden"}))
                .await,
        );
        assert!(denied.contains("denied"), "{mode}: {denied}");
    }
    support::ok(
        fixture
            .call(
                "default",
                "edit",
                json!({"path":"a.txt","old_string":"old","new_string":"edited"}),
            )
            .await,
    );
    assert_eq!(fixture.read("a.txt"), "edited");
}

fn policy(
    rules: Vec<cyber_tools::permissions::Rule>,
    mode: cyber_tools::permissions::Mode,
) -> cyber_tools::permissions::Policy {
    cyber_tools::permissions::Policy {
        rules,
        saved: vec![],
        mode,
        parent_modes: vec![],
        location: "/repo".into(),
        home: "/home/user".into(),
        plan_file: "/repo/.cyber/plans/plan.md".into(),
    }
}

#[test]
fn scoped_rules_require_identity_and_keep_modes_and_user_ceilings() {
    use cyber_tools::permissions::{Decision, Mode, Request, Rule, evaluate_scoped};
    let converted =
        claude_permissions(&json!({"permissions":{"allow":["Write"],"deny":["NotebookEdit"]}}))
            .unwrap();
    let rules = parse_rules(
        &serde_json::to_value(converted.rules).unwrap(),
        &BTreeMap::new(),
    );
    assert_eq!(evaluate(&rules, "edit", "a.txt").0, Effect::Ask);
    assert_eq!(
        evaluate_scoped(&rules, "edit", "a.txt", Some("write")).0,
        Effect::Allow
    );
    assert_eq!(
        evaluate_scoped(&rules, "edit", "a.txt", Some("edit")).0,
        Effect::Ask
    );
    let req = Request {
        action: "edit".into(),
        resources: vec!["a.txt".into()],
        tool: Some("write".into()),
        file_edit: true,
        mutates: vec!["/repo/a.txt".into()],
        ..Request::default()
    };
    let mut p = policy(rules, Mode::Default);
    assert_eq!(p.decide(&req), Decision::Allow);
    p.parent_modes.push(Mode::Plan);
    assert!(matches!(p.decide(&req), Decision::Deny(_)));
    p.parent_modes.clear();
    let mut denied = Rule::new("edit", "*", Effect::Deny, "global");
    denied.tool = Some("write".into());
    p.rules.push(denied);
    p.rules
        .push(Rule::new("edit", "*", Effect::Allow, "session"));
    p.saved.push(Rule::new("edit", "*", Effect::Allow, "saved"));
    for mode in [
        Mode::Default,
        Mode::Bypass,
        Mode::AcceptEdits,
        Mode::Auto,
        Mode::DontAsk,
    ] {
        p.mode = mode;
        assert!(matches!(p.decide(&req), Decision::Deny(_)));
    }
}

#[test]
fn invalid_scope_cannot_turn_into_an_unscoped_allow() {
    for tool in [json!(null), json!(true), json!(""), json!("write\n")] {
        let rules = parse_rules(
            &json!([{"action":"edit","resource":"*","effect":"allow","tool":tool}]),
            &BTreeMap::new(),
        );
        assert_eq!(evaluate(&rules, "edit", "a.txt").0, Effect::Deny);
        assert_eq!(
            cyber_tools::permissions::evaluate_scoped(&rules, "edit", "a.txt", Some("write")).0,
            Effect::Deny
        );
    }
}

#[test]
fn protected_exact_allow_is_bound_to_its_action_and_tool() {
    use cyber_tools::permissions::{Decision, Mode, Request, Rule, slash};
    let root = tempfile::tempdir().unwrap();
    let location = root.path().canonicalize().unwrap();
    let target = location.join("cyber.jsonc");
    let mut p = policy(
        vec![Rule::new("read", &slash(&target), Effect::Allow, "global")],
        Mode::Bypass,
    );
    p.location = location;
    let mut req = Request {
        action: "edit".into(),
        resources: vec!["cyber.jsonc".into()],
        tool: Some("write".into()),
        mutates: vec![target.clone()],
        file_edit: true,
        ..Request::default()
    };
    assert_eq!(p.decide(&req), Decision::Ask);
    let mut scoped = Rule::new("edit", &slash(&target), Effect::Allow, "global");
    scoped.tool = Some("write".into());
    p.rules = vec![scoped];
    assert_eq!(p.decide(&req), Decision::Allow);
    req.tool = Some("edit".into());
    assert_eq!(p.decide(&req), Decision::Ask);
}

#[tokio::test]
async fn notebook_allow_and_write_deny_remain_independent() {
    let fixture = support::Fixture::new();
    fixture.write("work.ipynb",&json!({"nbformat":4,"nbformat_minor":0,"metadata":{},"cells":[{"cell_type":"code","metadata":{},"source":["old"],"outputs":[],"execution_count":null}]}).to_string());
    let converted=claude_permissions(&json!({"permissions":{"allow":["NotebookEdit(./work.ipynb)"],"deny":["Write(./work.ipynb)"]}})).unwrap();
    fixture.set_config(json!({"permissions":converted.rules,"lsp":false,"formatters":false}));
    support::ok(
        fixture
            .call(
                "default",
                "notebook_edit",
                json!({"path":"work.ipynb","mode":"replace","cell_index":0,"new_source":"new"}),
            )
            .await,
    );
    let after = fixture.read("work.ipynb");
    assert!(after.contains("new"));
    let denied = support::failed(
        fixture
            .call(
                "bypass",
                "write",
                json!({"path":"work.ipynb","content":"forbidden"}),
            )
            .await,
    );
    assert!(denied.contains("denied"));
    assert_eq!(fixture.read("work.ipynb"), after);
}

#[test]
fn legacy_rule_wire_shape_is_unchanged_and_old_records_deserialize() {
    use cyber_tools::permissions::Rule;
    let json = json!({"action":"edit","resource":"*","effect":"allow","source":"global"});
    let rule: Rule = serde_json::from_value(json.clone()).unwrap();
    assert!(rule.tool.is_none());
    assert_eq!(serde_json::to_value(rule).unwrap(), json);
}

#[test]
fn opencode_literal_command_patterns_do_not_acquire_bare_prefix_grants() {
    let converted = cyber_core::import::opencode_permissions(&json!({"tools":{"bash":false},"permission":{"bash":{"git *":"allow","git push *":"deny"}}})).unwrap();
    let rules = parse_rules(&serde_json::to_value(converted).unwrap(), &BTreeMap::new());
    for (command, effect) in [
        ("git", Effect::Deny),
        ("git ", Effect::Allow),
        ("git status", Effect::Allow),
        ("git push", Effect::Allow),
        ("git push ", Effect::Deny),
        ("git push origin main", Effect::Deny),
        ("npm test", Effect::Deny),
    ] {
        assert_eq!(evaluate(&rules, "bash", command).0, effect, "{command:?}");
    }
}

#[tokio::test]
async fn opencode_imported_edit_group_denies_all_native_mutations_in_bypass() {
    let fixture = support::Fixture::new();
    fixture.write("a.txt", "original");
    let converted = cyber_core::import::opencode_permissions(
        &json!({"permission":{"read":"allow","edit":"deny"}}),
    )
    .unwrap();
    fixture.set_config(json!({"permissions":converted,"lsp":false,"formatters":false}));
    assert!(
        support::ok(
            fixture
                .call("default", "read", json!({"path":"a.txt"}))
                .await
        )
        .contains("original")
    );
    for (tool, input) in [
        ("write", json!({"path":"a.txt","content":"replacement"})),
        (
            "edit",
            json!({"path":"a.txt","old_string":"original","new_string":"replacement"}),
        ),
        (
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-original\n+replacement\n*** End Patch"}),
        ),
    ] {
        let error = support::failed(fixture.call("bypass", tool, input).await);
        assert!(error.contains("denied"), "{tool}: {error}");
        assert_eq!(fixture.read("a.txt"), "original");
    }
    assert!(
        support::ok(
            fixture
                .call("default", "read", json!({"path":"a.txt"}))
                .await
        )
        .contains("original")
    );
    let names = fixture.tool_names("default", false);
    for hidden in ["write", "edit", "apply_patch", "notebook_edit"] {
        assert!(!names.contains(&hidden.into()), "{hidden}");
    }
}

#[test]
fn codex_strongest_match_survives_later_allow_and_native_prefix_boundaries() {
    let converted = cyber_core::import::codex_prefix_rules(&json!([
        {"pattern":["git","push"],"decision":"forbidden"},
        {"pattern":["git",["fetch","pull"]],"decision":"prompt"},
        {"pattern":["git"],"decision":"allow"}
    ]))
    .unwrap();
    let rules = parse_rules(&serde_json::to_value(converted).unwrap(), &BTreeMap::new());
    for (command, effect) in [
        ("git push", Effect::Deny),
        ("git push origin main", Effect::Deny),
        ("\"git\" 'push' origin main", Effect::Deny),
        ("git   push", Effect::Deny),
        ("g'it' p\"u\"sh", Effect::Deny),
        ("GIT push", Effect::Ask),
        ("git status $UNKNOWN", Effect::Deny),
        ("git pushy", Effect::Allow),
        ("git fetch", Effect::Ask),
        ("git pull origin main", Effect::Ask),
        ("git status", Effect::Allow),
        ("git", Effect::Allow),
        ("github", Effect::Ask),
    ] {
        assert_eq!(evaluate(&rules, "bash", command).0, effect, "{command}");
    }
}

#[tokio::test]
async fn codex_imported_argv_deny_blocks_quoted_and_compound_commands_before_launch() {
    let fixture = support::Fixture::new();
    let converted = cyber_core::import::codex_prefix_rules(&json!([
        {"pattern":["git","push"],"decision":"forbidden"},
        {"pattern":["git"],"decision":"allow"},
        {"pattern":["echo"],"decision":"allow"}
    ]))
    .unwrap();
    fixture.set_config(json!({"permissions":converted}));
    for command in [
        "git push",
        "git  push",
        "'git' \"push\"",
        "echo allowed && 'git' push",
        "g'it' p\"u\"sh",
    ] {
        let error = support::failed(
            fixture
                .call("bypass", "bash", json!({"command":command}))
                .await,
        );
        assert!(error.contains("denied"), "{command}: {error}");
        assert!(!error.contains("Could not start"));
    }
}

#[test]
fn malformed_argv_metadata_cannot_become_an_allow_rule() {
    for prefix in [
        json!(null),
        json!([]),
        json!([42]),
        json!([""]),
        json!(["git\n"]),
    ] {
        let rules = parse_rules(
            &json!([{"action":"bash","resource":"*","effect":"allow","argv_prefix":prefix}]),
            &BTreeMap::new(),
        );
        assert_eq!(evaluate(&rules, "bash", "git push").0, Effect::Deny);
    }
    let rules = parse_rules(
        &json!([{"action":"bash","resource":"echo *","effect":"allow","argv_prefix":["echo"]}]),
        &BTreeMap::new(),
    );
    assert_eq!(evaluate(&rules, "bash", "echo plain").0, Effect::Allow);
    assert_eq!(evaluate(&rules, "bash", "echo $UNKNOWN").0, Effect::Ask);
    assert_eq!(
        evaluate(&rules, "bash", "echo one; git push").0,
        Effect::Ask
    );
}

#[tokio::test]
async fn codex_rules_text_retains_forbidden_prefix_through_the_actual_host() {
    let fixture = support::Fixture::new();
    let plan=cyber_core::import::codex_rules("prefix_rule(pattern=['git','push'], decision='forbidden', match=['git push'])\nprefix_rule(pattern=['git'])\n").unwrap();
    fixture.set_config(json!({"permissions":plan.rules}));
    let error = support::failed(
        fixture
            .call(
                "bypass",
                "bash",
                json!({"command":"'git'  push origin main"}),
            )
            .await,
    );
    assert!(error.contains("denied"), "{error}");
    assert!(!error.contains("Could not start"));
}

#[test]
fn converted_codex_sandbox_policies_resolve_without_bypassing_project_restrictions() {
    use cyber_sandbox::{Policy, SandboxConfig};
    let home = std::path::Path::new("/home/user");
    for (source, expected) in [
        ("read-only", Policy::ReadOnly),
        ("workspace-write", Policy::WorkspaceWrite),
        ("danger-full-access", Policy::FullAccess),
    ] {
        let mapped = cyber_core::import::codex_policy_config(
            &json!({"approval_policy":"never","sandbox_mode":source}),
        )
        .unwrap();
        let global = BTreeMap::from([("/sandbox/policy".into(), "global:cyber.jsonc".into())]);
        assert_eq!(
            SandboxConfig::resolve(&mapped.config, &global, None, home).policy,
            expected
        );
        let project = BTreeMap::from([("/sandbox/policy".into(), "project:cyber.jsonc".into())]);
        assert_eq!(
            SandboxConfig::resolve(&mapped.config, &project, None, home).policy,
            if expected == Policy::FullAccess {
                Policy::WorkspaceWrite
            } else {
                expected
            }
        );
    }
}

#[tokio::test]
async fn migrated_inline_agent_denials_survive_bypass_in_actual_tool_dispatch() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let fixture = support::Fixture::new();
    fixture.write(".env", "private-file-value");
    fixture.write("public.txt", "public-file-value");
    let result = cyber_core::import::opencode_settings_config(
        &json!({"agents":{"review":{"mode":"subagent","permissions":[
            {"action":"read","resource":"*","effect":"allow"},
            {"action":"read","resource":".env","effect":"deny"},
            {"action":"edit","resource":"*","effect":"deny"}
        ]}}}),
    )
    .unwrap();
    fixture.set_config(result.config);
    for (path, allowed) in [(".env", false), ("public.txt", true)] {
        let mut invocation = fixture.invocation("bypass", "read", json!({"path":path}));
        invocation.agent = "review".into();
        let outcome = fixture
            .host
            .execute(invocation, CancellationToken::new())
            .await;
        if allowed {
            assert!(support::ok(outcome).contains("public-file-value"));
        } else {
            let error = support::failed(outcome);
            assert!(error.contains("denied"));
            assert!(!error.contains("private-file-value"));
        }
    }
    let mut invocation = fixture.invocation(
        "bypass",
        "write",
        json!({"path":"public.txt","content":"changed"}),
    );
    invocation.agent = "review".into();
    assert!(
        support::failed(
            fixture
                .host
                .execute(invocation, CancellationToken::new())
                .await
        )
        .contains("denied")
    );
    assert_eq!(fixture.read("public.txt"), "public-file-value");
}

#[tokio::test]
async fn migration_rule_wrapper_enforces_denials_in_actual_tools() {
    let fixture = support::Fixture::new();
    fixture.write(".env", "private-file-value");
    fixture.set_config(
        json!({"permissions":{"rules":[{"action":"read","resource":".env","effect":"deny"}]}}),
    );
    let error = support::failed(fixture.call("bypass", "read", json!({"path":".env"})).await);
    assert!(error.contains("denied"));
    assert!(!error.contains("private-file-value"));
}

#[test]
fn migration_rule_wrapper_retains_written_map_order_and_leaf_sources() {
    let sources = BTreeMap::from([(
        "/permissions/rules/0/effect".into(),
        "imported-policy".into(),
    )]);
    let rules = parse_rules(
        &json!({"read":"allow","rules":[{"action":"read","resource":".env","effect":"deny"}],"bash":"ask"}),
        &sources,
    );
    let (effect, rule) = evaluate(&rules, "read", ".env");
    assert_eq!(effect, Effect::Deny);
    assert_eq!(rule.unwrap().source, "imported-policy");
    assert_eq!(evaluate(&rules, "read", "public.txt").0, Effect::Allow);
    assert_eq!(evaluate(&rules, "bash", "git status").0, Effect::Ask);
}
