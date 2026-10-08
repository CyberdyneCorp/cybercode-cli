//! Event identity cannot be overwritten by payload data or chained tool rewrites.
use cyber_core::hooks::{HookEvent, HookIdentity, HookLocation};
use serde_json::{Value, json};

fn identity(directory: &std::path::Path) -> HookIdentity {
    HookIdentity {
        session_id: "ses_owned".into(),
        location: HookLocation {
            directory: directory.into(),
            workspace: None,
        },
        project_id: "prj_example".into(),
        agent: "coder".into(),
        mode: "accept-edits".into(),
    }
}

fn fields(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn post_tool_envelope_contains_identity_and_flat_event_fields() {
    let directory = tempfile::tempdir().unwrap();
    let identity = identity(directory.path());
    let event = HookEvent::new("PostToolUse", identity.clone(),123,
        fields(json!({"tool_name":"edit","tool_input":{"path":"src/main.rs"},"tool_output":{"ok":true},"call_id":"call_1","duration_ms":7}))).unwrap();
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["event"], "PostToolUse");
    assert_eq!(value["session_id"], "ses_owned");
    assert_eq!(
        value["location"]["directory"],
        serde_json::to_value(directory.path()).unwrap()
    );
    assert!(value["location"].get("workspace").is_none());
    assert_eq!(value["project_id"], "prj_example");
    assert_eq!(value["agent"], "coder");
    assert_eq!(value["mode"], "accept-edits");
    assert_eq!(value["timestamp"], 123);
    assert_eq!(value["tool_name"], "edit");
    assert_eq!(value["duration_ms"], 7);
    assert_eq!(event.identity().session_id, identity.session_id);
}

#[test]
fn payload_cannot_spoof_any_common_envelope_field() {
    let directory = tempfile::tempdir().unwrap();
    for key in [
        "event",
        "session_id",
        "location",
        "project_id",
        "agent",
        "mode",
        "timestamp",
    ] {
        let mut payload = serde_json::Map::new();
        payload.insert(key.into(), json!("spoofed"));
        let error =
            HookEvent::new("PreToolUse", identity(directory.path()), 1, payload).unwrap_err();
        assert!(error.contains(key), "{error}");
    }
}

#[test]
fn rewrites_preserve_owned_identity_and_leave_prior_event_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut identity = identity(directory.path());
    identity.location.workspace = Some("wtr_example".into());
    let event = HookEvent::new(
        "PreToolUse",
        identity,
        123,
        fields(json!({"tool_name":"bash","tool_input":{"command":"first"},"call_id":"call_1"})),
    )
    .unwrap();
    let rewritten = event.with_tool_input(json!({"command":"second"})).unwrap();
    assert_eq!(event.as_json()["tool_input"]["command"], "first");
    assert_eq!(rewritten.as_json()["tool_input"]["command"], "second");
    for key in [
        "event",
        "session_id",
        "location",
        "project_id",
        "agent",
        "mode",
        "timestamp",
        "call_id",
    ] {
        assert_eq!(event.as_json()[key], rewritten.as_json()[key], "{key}");
    }
    assert_eq!(rewritten.as_json()["location"]["workspace"], "wtr_example");
    assert!(
        HookEvent::new(
            "PostToolUse",
            event.identity().clone(),
            1,
            fields(json!({}))
        )
        .unwrap()
        .with_tool_input(json!({}))
        .is_err()
    );
}

#[test]
fn malformed_identity_and_unknown_events_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        HookEvent::new(
            "BeforeEverything",
            identity(directory.path()),
            1,
            fields(json!({}))
        )
        .is_err()
    );
    assert!(HookEvent::new("Stop", identity(directory.path()), -1, fields(json!({}))).is_err());
    for field in [
        "session",
        "project",
        "agent",
        "mode",
        "directory",
        "workspace",
    ] {
        let mut identity = identity(directory.path());
        match field {
            "session" => identity.session_id = "foreign".into(),
            "project" => identity.project_id = "foreign".into(),
            "agent" => identity.agent.clear(),
            "mode" => identity.mode = "invalid".into(),
            "directory" => identity.location.directory = "relative".into(),
            "workspace" => identity.location.workspace = Some("".into()),
            _ => unreachable!(),
        }
        assert!(
            HookEvent::new("Stop", identity, 1, fields(json!({}))).is_err(),
            "{field}"
        );
    }
}
