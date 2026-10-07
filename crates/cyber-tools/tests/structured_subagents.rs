//! Schema-governed child results through the built-in agent path.
mod support;

use serde_json::{Value, json};
use support::flow::{Flow, call, text};

fn schema() -> Value {
    json!({"type":"object","required":["findings"],"additionalProperties":false,"properties":{
        "findings":{"type":"array","items":{"type":"string"},"minItems":1}
    }})
}

#[tokio::test]
async fn valid_result_ends_the_child_and_returns_the_validated_value() {
    let value = json!({"findings":["parser is iterative"]});
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            call("result", "return_result", value.clone()),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    let result: Value = serde_json::from_str(&output).expect(&output);
    assert_eq!(result["result"], value);
    assert_eq!(flow.main.requests().len(), 3);
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.parent_id.as_deref(), Some(parent.as_str()));
    assert!(!flow.runtime.is_running(&child.info.id));
    assert!(
        flow.main.requests()[1]
            .tools
            .iter()
            .any(|tool| tool.name == "return_result" && tool.input_schema == schema())
    );
}

#[tokio::test]
async fn missing_results_receive_exactly_one_reprompt_then_schema_mismatch() {
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            text("first text-only answer"),
            text("second text-only answer"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    assert!(output.contains("SchemaMismatch"), "{output}");
    assert!(output.contains("return_result"), "{output}");
    assert_eq!(flow.main.requests().len(), 4);
}

#[tokio::test]
async fn invalid_schema_is_refused_without_a_child() {
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":{"type":7}}),
            ),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    assert!(output.contains("Invalid output_schema"), "{output}");
    let count = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn invalid_values_are_rejected_and_the_single_retry_can_return_a_valid_result() {
    use cyber_server::runtime::{CallStatus, SessionState};
    let value = json!({"findings":["valid"]});
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            call("invalid", "return_result", json!({"findings":[]})),
            text("text only"),
            call("valid", "return_result", value.clone()),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "spawn").await).unwrap();
    let id = result["id"].as_str().unwrap();
    let child = flow.runtime.state(id).await.unwrap();
    assert_eq!(child.calls["invalid"].status, CallStatus::Error);
    assert!(
        child.calls["invalid"]
            .output
            .as_deref()
            .unwrap()
            .contains("/findings")
    );
    assert_eq!(child.inbox.len(), 2);
    assert_eq!(child.structured_result(), Some(&value));
    assert_eq!(
        flow.runtime.state(&parent).await.unwrap().calls["spawn"].structured_output,
        Some(value.clone())
    );
    assert_eq!(flow.main.requests().len(), 5);
    let replayed =
        SessionState::replay(&flow.f.store.read_events(id, -1, 200).unwrap().events).unwrap();
    assert_eq!(replayed.structured_result(), Some(&value));
    let replayed =
        SessionState::replay(&flow.f.store.read_events(&parent, -1, 200).unwrap().events).unwrap();
    assert_eq!(replayed.calls["spawn"].structured_output, Some(value));
}

#[tokio::test]
async fn json_null_is_a_result_and_survives_durable_replay() {
    use cyber_server::runtime::SessionState;
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":{"type":"null"}}),
            ),
            call("result", "return_result", Value::Null),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "spawn").await).unwrap();
    assert!(result.get("result").is_some_and(Value::is_null));
    let id = result["id"].as_str().unwrap();
    let child =
        SessionState::replay(&flow.f.store.read_events(id, -1, 200).unwrap().events).unwrap();
    assert_eq!(child.structured_result(), Some(&Value::Null));
    let parent =
        SessionState::replay(&flow.f.store.read_events(&parent, -1, 200).unwrap().events).unwrap();
    assert_eq!(parent.calls["spawn"].structured_output, Some(Value::Null));
    let wire = serde_json::to_value(&parent.calls["spawn"]).unwrap();
    assert!(wire.get("structured_output").is_some_and(Value::is_null));
}

#[tokio::test]
async fn a_valid_return_stops_later_calls_in_the_same_response() {
    use cyber_server::runtime::CallStatus;
    let value = json!({"findings":["first result"]});
    let mut finish = call("result", "return_result", value.clone());
    finish.truncate(1);
    finish.extend(
        call(
            "write",
            "write",
            json!({"path":"must-not-exist.txt","content":"unexpected"}),
        )
        .into_iter()
        .take(1),
    );
    finish.extend(call(
        "second",
        "return_result",
        json!({"findings":["replacement"]}),
    ));
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            finish,
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "spawn").await).unwrap();
    assert_eq!(result["result"], value);
    assert!(!flow.f.repo.join("must-not-exist.txt").exists());
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.calls["write"].status, CallStatus::Interrupted);
    assert_eq!(child.calls["write"].attempt, 0);
    assert_eq!(child.calls["second"].attempt, 0);
    assert_eq!(flow.main.requests().len(), 3);
}

#[tokio::test]
async fn invalid_second_result_reports_the_last_validation_errors() {
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            text("first text-only answer"),
            call("bad", "return_result", json!({"findings":[]})),
            text("second text-only answer"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    assert!(
        output.contains("SchemaMismatch") && output.contains("/findings"),
        "{output}"
    );
    assert_eq!(flow.main.requests().len(), 5);
    assert!(
        flow.runtime.state(&parent).await.unwrap().calls["spawn"]
            .structured_output
            .is_none()
    );
}

#[tokio::test]
async fn full_schema_constraints_and_local_references_are_enforced() {
    use cyber_server::runtime::StructuredSchema;
    let schema=StructuredSchema::new(json!({"$defs":{"finding":{"type":"string","pattern":"^src/"}},
        "type":"object","required":["paths"],"additionalProperties":false,"properties":{
            "paths":{"type":"array","items":{"$ref":"#/$defs/finding"},"minItems":1,"uniqueItems":true}
    }})).unwrap();
    assert!(schema.validate(&json!({"paths":["src/parser.rs"]})).is_ok());
    for value in [
        json!({"paths":[]}),
        json!({"paths":["elsewhere"]}),
        json!({"paths":["src/a","src/a"]}),
        json!({"paths":["src/a"],"extra":true}),
    ] {
        assert!(schema.validate(&value).is_err());
    }
    assert!(StructuredSchema::new(json!({"$ref":"file:///nonexistent/private.json"})).is_err());
    assert!(StructuredSchema::new(json!({"$ref":"https://127.0.0.1/private.json"})).is_err());
}

#[tokio::test]
async fn typed_result_survives_the_general_tool_output_budget() {
    let value = json!({"findings":["a".repeat(60_000)]});
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            call("result", "return_result", value.clone()),
            text("parent done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"tool_output":{"max_bytes":128}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let state = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(state.calls["spawn"].structured_output, Some(value));
    assert!(
        state.calls["spawn"]
            .output
            .as_deref()
            .unwrap()
            .contains("full output at")
    );
}

#[tokio::test]
async fn parent_interrupt_during_the_result_retry_stops_the_child() {
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            text("text only"),
            call("read", "read", json!({"path":".env"})),
            text("must not run"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    let request = flow.pending(&parent).await;
    let child = request.session_id.clone();
    assert_eq!(flow.runtime.state(&child).await.unwrap().inbox.len(), 2);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        flow.runtime.interrupt(&parent),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!flow.runtime.is_running(&child));
    assert!(!flow.runtime.is_running(&parent));
    assert_eq!(flow.main.requests().len(), 3);
    assert!(
        flow.runtime
            .state(&child)
            .await
            .unwrap()
            .structured_result()
            .is_none()
    );
}

#[tokio::test]
async fn rejecting_a_child_does_not_restart_it_for_schema_correction() {
    use cyber_server::runtime::PermissionReply;
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            call("read", "read", json!({"path":".env"})),
            text("parent done"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    let request = flow.pending(&parent).await;
    let child = request.session_id.clone();
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Reject { message: None })
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert_eq!(flow.runtime.state(&child).await.unwrap().inbox.len(), 1);
    assert_eq!(flow.main.requests().len(), 3);
    assert!(
        flow.output(&parent, "spawn")
            .await
            .contains("Subagent stopped")
    );
}

#[tokio::test]
async fn duplicate_call_ids_cannot_replace_a_result_with_a_side_effect() {
    let mut finish = call("duplicate", "return_result", json!({"findings":["result"]}));
    finish.truncate(1);
    finish.extend(call(
        "duplicate",
        "write",
        json!({"path":"must-not-exist.txt","content":"unexpected"}),
    ));
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect","output_schema":schema()}),
            ),
            finish,
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    assert!(!flow.f.repo.join("must-not-exist.txt").exists());
    let output = flow.output(&parent, "spawn").await;
    assert!(output.contains("Subagent stopped"), "{output}");
    assert_eq!(flow.main.requests().len(), 3);
}
