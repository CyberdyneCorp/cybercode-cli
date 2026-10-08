//! Selection and decision contracts exercised before effectful hook dispatch.
use std::collections::HashMap;

use cyber_core::config::{self, LoadRequest};
use cyber_core::hooks::{HookAction, HookCatalog, HookDecision};
use cyber_core::paths::Paths;
use serde_json::{Value, json};

fn catalog(hooks: Value) -> HookCatalog {
    let dir = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        dir.path().join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, dir.path());
    std::fs::create_dir_all(&paths.config).unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"hooks":hooks}).to_string(),
    )
    .unwrap();
    let request = LoadRequest {
        location: dir.path(),
        paths: &paths,
        env: &env,
        home: dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    };
    HookCatalog::from_config(&config::load(&request).unwrap()).unwrap()
}

#[test]
fn selectors_match_mcp_regex_paths_and_dotted_conditions() {
    let catalog = catalog(
        json!({"PreToolUse":[{"matcher":"/^mcp__github__.*/","paths":["migrations/**"],"hooks":[
            {"type":"command","command":"review","if":{"field":"tool_input.action","matches":"^write$"}}
        ]}]}),
    );
    let hook = &catalog.definitions[0];
    let payload = json!({"tool_input":{"action":"write"}});
    assert!(hook.matches(
        "PreToolUse",
        "mcp__github__create_issue",
        &["migrations/001.sql".into()],
        &payload
    ));
    assert!(hook.matches(
        "PreToolUse",
        "mcp__github__create_issue",
        &[".\\migrations\\001.sql".into()],
        &payload
    ));
    assert!(!hook.matches(
        "PostToolUse",
        "mcp__github__create_issue",
        &["migrations/001.sql".into()],
        &payload
    ));
    assert!(!hook.matches(
        "PreToolUse",
        "bash",
        &["migrations/001.sql".into()],
        &payload
    ));
    assert!(!hook.matches(
        "PreToolUse",
        "mcp__github__create_issue",
        &["src/main.rs".into()],
        &payload
    ));
    for path in [
        "../migrations/001.sql",
        "/migrations/001.sql",
        "C:\\migrations\\001.sql",
        "\\\\server\\migrations\\001.sql",
    ] {
        assert!(
            !hook.matches(
                "PreToolUse",
                "mcp__github__create_issue",
                &[path.into()],
                &payload
            ),
            "{path}"
        );
    }
    assert!(!hook.matches(
        "PreToolUse",
        "mcp__github__create_issue",
        &["migrations/001.sql".into()],
        &json!({})
    ));
    assert!(!hook.matches(
        "PreToolUse",
        "mcp__github__create_issue",
        &["migrations/001.sql".into()],
        &json!({"tool_input":{"action":"read"}})
    ));
}

#[test]
fn absent_matchers_and_scalar_conditions_select_without_file_targets() {
    let catalog = catalog(
        json!({"Notification":[{"hooks":[{"type":"command","command":"notify","if":{"field":"enabled","matches":"^true$"}}]}]}),
    );
    assert!(catalog.definitions[0].matches(
        "Notification",
        "anything",
        &[],
        &json!({"enabled":true})
    ));
    assert!(!catalog.definitions[0].matches(
        "Notification",
        "anything",
        &[],
        &json!({"enabled":false})
    ));
}

#[test]
fn decision_validation_ignores_event_invalid_fields_and_accepts_explicit_exceptions() {
    let parsed = HookDecision::parse(
        "PostToolUse",
        &json!({"updated_input":{"x":1},"unknown":true,"additional_context":"note"}),
    )
    .unwrap();
    assert!(parsed.decision.updated_input.is_none());
    assert!(parsed.ignored_fields.contains(&"updated_input".into()));
    assert!(parsed.ignored_fields.contains(&"unknown".into()));
    assert_eq!(
        HookDecision::parse("Stop", &json!({"decision":"block","reason":"tests failed"}))
            .unwrap()
            .decision
            .decision,
        Some(HookAction::Block)
    );
    assert_eq!(
        HookDecision::parse("PreToolUse", &json!({"decision":"block"}))
            .unwrap()
            .ignored_fields,
        ["decision"]
    );
    let retry = HookDecision::parse(
        "PermissionDenied",
        &json!({"decision":"retry","updated_input":{"command":"npm test"}}),
    )
    .unwrap();
    assert_eq!(retry.decision.decision, Some(HookAction::Retry));
    assert!(retry.decision.updated_input.is_some());
    for value in [
        json!([]),
        json!({"decision":"approve-everything"}),
        json!({"continue":"false"}),
        json!({"reason":42}),
    ] {
        assert!(HookDecision::parse("PreToolUse", &value).is_err());
    }
    assert!(HookDecision::parse("UnknownEvent", &json!({})).is_err());
}

#[test]
fn ordered_decisions_chain_rewrites_and_preserve_denial_and_stop() {
    let original = json!({"command":"original"});
    let mut merged = HookDecision::default();
    for value in [
        json!({"decision":"allow","updated_input":{"command":"first"},"additional_context":"one"}),
        json!({"decision":"ask","reason":"review","continue":false,"stop_reason":"halt","suppress_output":true}),
        json!({"decision":"deny","reason":"policy","updated_input":{"command":"second"},"additional_context":"two"}),
        json!({"decision":"allow","reason":"later allow","continue":true,"suppress_output":false}),
    ] {
        let next = HookDecision::parse("PreToolUse", &value).unwrap().decision;
        merged.merge(next);
    }
    assert_eq!(merged.decision, Some(HookAction::Deny));
    assert_eq!(merged.reason.as_deref(), Some("policy"));
    assert_eq!(merged.input(&original), &json!({"command":"second"}));
    assert_eq!(merged.additional_context.as_deref(), Some("one\ntwo"));
    assert_eq!(merged.continuation, Some(false));
    assert_eq!(merged.stop_reason.as_deref(), Some("halt"));
    assert_eq!(merged.suppress_output, Some(true));
    merged.merge(
        HookDecision::parse(
            "PreToolUse",
            &json!({"decision":"deny","reason":"later deny"}),
        )
        .unwrap()
        .decision,
    );
    assert_eq!(merged.reason.as_deref(), Some("later deny"));
}

#[test]
fn glob_subjects_and_explicit_wildcards_match_independently_of_conditions() {
    let catalog = catalog(json!({"PreToolUse":[
        {"matcher":"edit*","hooks":[{"type":"command","command":"guard"}]},
        {"matcher":"*","hooks":[{"type":"command","command":"observe"}]}
    ]}));
    assert!(catalog.definitions[0].matches("PreToolUse", "edit_file", &[], &json!({})));
    assert!(!catalog.definitions[0].matches("PreToolUse", "bash", &[], &json!({})));
    assert!(catalog.definitions[1].matches("PreToolUse", "bash", &[], &json!({})));
}

#[test]
fn rewritten_envelope_drives_later_handler_conditions_without_replacing_identity() {
    use cyber_core::hooks::{HookEvent, HookIdentity, HookLocation};
    let directory = tempfile::tempdir().unwrap();
    let catalog = catalog(json!({"PreToolUse":[{"matcher":"bash","hooks":[
        {"type":"command","command":"check","if":{"field":"tool_input.command","matches":"^second$"}}
    ]}]}));
    let fields = json!({"tool_name":"bash","tool_input":{"command":"first"}})
        .as_object()
        .unwrap()
        .clone();
    let event = HookEvent::new(
        "PreToolUse",
        HookIdentity {
            session_id: "ses_owned".into(),
            location: HookLocation {
                directory: directory.path().into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "coder".into(),
            mode: "default".into(),
        },
        123,
        fields,
    )
    .unwrap();
    assert!(!catalog.definitions[0].matches_event(&event, "bash", &[]));
    let decision =
        HookDecision::parse("PreToolUse", &json!({"updated_input":{"command":"second"}}))
            .unwrap()
            .decision;
    let rewritten = event
        .with_tool_input(decision.input(&event.as_json()["tool_input"]).clone())
        .unwrap();
    assert!(catalog.definitions[0].matches_event(&rewritten, "bash", &[]));
    assert_eq!(rewritten.identity().session_id, event.identity().session_id);
    assert_eq!(rewritten.identity().mode, event.identity().mode);
    assert_eq!(event.as_json()["tool_input"]["command"], "first");
}
