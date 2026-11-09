//! A root prompt's structured result (`session-runtime` → Structured output, Root prompt
//! structured result lifecycle) through the runtime with a scripted provider: the
//! `structured_output` finalizer, its corrections and limits, and its durable outcome.

mod support;

use cyber_server::runtime::{Admission, Delivery, RuntimeError, StructuredOutcome};
use serde_json::{Value, json};
use support::*;

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"summary": {"type": "string"}},
        "required": ["summary"],
        "additionalProperties": false
    })
}

fn structured(id: &str, input: &str) -> Vec<cyber_llm::adapters::ScriptStep> {
    tools(&[(id, "structured_output", input)])
}

async fn ask(h: &Harness, id: &str, text: &str, schema: Option<Value>) -> String {
    let mut admission = Admission::text(text, Delivery::Queue);
    admission.output_schema = schema;
    h.runtime.admit(id, admission).await.unwrap().message_id
}

async fn result_of(h: &Harness, id: &str, message: &str) -> Option<StructuredOutcome> {
    let state = h.state(id).await;
    let row = state
        .inbox
        .iter()
        .find(|row| row.message_id == message)
        .unwrap();
    row.structured.clone()
}

fn harness(main: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> Harness {
    Harness::new(Setup {
        scripts: vec![("test/main", main)],
        ..Setup::default()
    })
}

#[tokio::test]
async fn interrupted_open_result_is_terminal_and_remains_bound_to_its_input_after_restart() {
    let h = harness(vec![
        tools(&[("work", "read", "{}")]),
        text("later plain answer"),
    ]);
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::UntilCancelled);
    let id = h.session().await;
    let started = h.tools.started.notified();
    let message = ask(&h, &id, "audit", Some(schema())).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap();
    h.runtime.interrupt(&id).await.unwrap();
    h.settle(&id).await;
    let outcome = result_of(&h, &id, &message)
        .await
        .expect("interrupted input must settle");
    assert_eq!(outcome.value, None);
    assert_eq!(
        outcome.attempts, 0,
        "ordinary tools are not result submissions"
    );
    assert!(outcome.error.as_deref().unwrap().contains("interrupted"));
    assert_terminal_result_survives_restart(&h, &id, &message, &outcome).await;
}

#[tokio::test]
async fn a_budget_ending_an_open_correction_records_a_terminal_input_result() {
    let h = harness(vec![
        structured("invalid", r#"{"wrong":1}"#),
        text("must not dispatch"),
    ]);
    let id = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            budget: Some(cyber_core::budget::Budget {
                max_turns: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;
    let outcome = result_of(&h, &id, &message)
        .await
        .expect("budget-ended input must settle");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.attempts, 1);
    assert!(outcome.error.as_deref().unwrap().contains("budget"));
    assert_terminal_result_survives_restart(&h, &id, &message, &outcome).await;
}

async fn assert_terminal_result_survives_restart(
    h: &Harness,
    id: &str,
    message: &str,
    outcome: &StructuredOutcome,
) {
    let events = h.store.read_events(id, -1, 200).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "session.structured_output.settled.1"
                && event.data["message_id"] == message)
            .count(),
        1
    );
    assert_eq!(h.models.requests("test/main").len(), 1);
    let restarted = h.restart();
    let state = restarted.state(id).await.unwrap();
    let recorded = state
        .inbox
        .iter()
        .find(|row| row.message_id == message)
        .unwrap();
    assert_eq!(recorded.structured.as_ref(), Some(outcome));
    assert_eq!(
        h.models.requests("test/main").len(),
        1,
        "lookup never redispatches"
    );
}

#[tokio::test]
async fn shutdown_keeps_the_open_result_for_explicit_restart_resume() {
    let h = harness(vec![
        tools(&[("work", "read", "{}")]),
        structured("final", r#"{"summary":"resumed"}"#),
    ]);
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::UntilCancelled);
    let id = h.session().await;
    let started = h.tools.started.notified();
    let message = ask(&h, &id, "audit", Some(schema())).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap();
    h.runtime.shutdown().await;
    h.settle(&id).await;
    assert!(result_of(&h, &id, &message).await.is_none());
    let restarted = h.restart();
    restarted.resume(&id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), restarted.wait_idle(&id))
        .await
        .unwrap();
    let state = restarted.state(&id).await.unwrap();
    let outcome = state
        .inbox
        .iter()
        .find(|row| row.message_id == message)
        .unwrap()
        .structured
        .clone()
        .unwrap();
    assert_eq!(outcome.value, Some(json!({"summary":"resumed"})));
    assert_eq!(outcome.attempts, 1);
    assert_eq!(h.models.requests("test/main").len(), 2);
}

#[tokio::test]
async fn compaction_during_a_correction_keeps_the_input_schema_and_attempt_count() {
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![
                    text("earlier answer"),
                    structured("invalid", r#"{"wrong":1}"#),
                    error(cyber_llm::ErrorKind::ContextOverflow, "prompt too long"),
                    structured("final", r#"{"summary":"after compaction"}"#),
                ],
            ),
            ("test/summary", vec![text("compressed earlier work")]),
        ],
        compaction: cyber_server::runtime::CompactionConfig {
            auto: true,
            buffer: 100,
            keep_tokens: 10_000,
            keep_turns: Some(1),
            model: None,
        },
        ..Setup::default()
    });
    let id = h.session().await;
    ask(&h, &id, "before", None).await;
    h.settle(&id).await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;
    let outcome = result_of(&h, &id, &message).await.unwrap();
    assert_eq!(outcome.value, Some(json!({"summary":"after compaction"})));
    assert_eq!(outcome.attempts, 2);
    assert!(h.state(&id).await.compacted.is_some());
    assert_eq!(h.models.requests("test/summary").len(), 1);
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 4);
    let retried = &requests[3];
    assert_eq!(
        retried
            .tools
            .iter()
            .find(|tool| tool.name == "structured_output")
            .unwrap()
            .input_schema,
        schema()
    );
    assert!(
        retried
            .system
            .concat()
            .contains("calling structured_output")
    );
}

#[tokio::test]
async fn a_valid_submission_is_the_inputs_result_and_ends_the_answer() {
    let h = harness(vec![structured("c1", r#"{"summary":"all good"}"#)]);
    let id = h.session().await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;

    let outcome = result_of(&h, &id, &message).await.expect("settled");
    assert_eq!(outcome.value, Some(json!({"summary": "all good"})));
    assert_eq!((outcome.error, outcome.attempts), (None, 1));
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 1, "no Turn after a valid result");
    let tool = requests[0]
        .tools
        .iter()
        .find(|t| t.name == "structured_output");
    assert_eq!(tool.map(|t| t.input_schema.clone()), Some(schema()));
    assert!(
        requests[0]
            .system
            .concat()
            .contains("calling structured_output")
    );
}

#[tokio::test]
async fn invalid_submissions_are_corrected_then_fail_without_a_fourth() {
    let h = harness(vec![
        structured("c1", r#"{"wrong":1}"#),
        structured("c2", r#"{"summary":3}"#),
        structured("c3", r#"{}"#),
        text("never sent"),
    ]);
    let id = h.session().await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;

    assert_eq!(
        h.models.requests("test/main").len(),
        3,
        "no fourth submission"
    );
    let outcome = result_of(&h, &id, &message).await.expect("settled");
    assert_eq!((outcome.value.clone(), outcome.attempts), (None, 3));
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("StructuredOutputError: Invalid result")),
        "{outcome:?}"
    );
    let second = h.models.requests("test/main")[1].messages.clone();
    assert!(
        format!("{second:?}").contains("Invalid result"),
        "the model sees why"
    );
}

#[tokio::test]
async fn an_answer_without_the_finalizer_is_asked_again() {
    let h = harness(vec![
        text("here is the summary in prose"),
        structured("c1", r#"{"summary":"ok"}"#),
    ]);
    let id = h.session().await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;
    let outcome = result_of(&h, &id, &message).await.expect("settled");
    assert_eq!(
        (outcome.value, outcome.attempts),
        (Some(json!({"summary": "ok"})), 2)
    );
    let reminder = format!("{:?}", h.models.requests("test/main")[1].messages);
    assert!(
        reminder.contains("must end with a call to structured_output"),
        "{reminder}"
    );

    // Three prose answers fail it.
    let h = harness(vec![text("one"), text("two"), text("three"), text("four")]);
    let id = h.session().await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;
    assert_eq!(h.models.requests("test/main").len(), 3);
    let outcome = result_of(&h, &id, &message).await.expect("settled");
    assert_eq!(
        outcome.error.as_deref(),
        Some("StructuredOutputError: the answer ended without calling structured_output")
    );
}

#[tokio::test]
async fn each_input_has_its_own_schema_result_and_count_across_a_restart() {
    let h = harness(vec![
        structured("c1", r#"{"summary":"first"}"#),
        text("plain answer"),
        tools(&[("c2", "structured_output", r#"{"count":2}"#)]),
    ]);
    let id = h.session().await;
    let first = ask(&h, &id, "one", Some(schema())).await;
    h.settle(&id).await;

    let runtime = h.restart();
    let mut plain = Admission::text("two", Delivery::Queue);
    plain.output_schema = None;
    let second = runtime.admit(&id, plain).await.unwrap().message_id;
    tokio::time::timeout(std::time::Duration::from_secs(30), runtime.wait_idle(&id))
        .await
        .unwrap();
    let state = runtime.state(&id).await.unwrap();
    let row = |m: &str| {
        state
            .inbox
            .iter()
            .find(|r| r.message_id == m)
            .unwrap()
            .clone()
    };
    assert_eq!(
        row(&first).structured.unwrap().value,
        Some(json!({"summary": "first"}))
    );
    assert!(
        row(&second).structured.is_none(),
        "an unstructured prompt has no result"
    );
    let tools = &h.models.requests("test/main")[1].tools;
    assert!(
        tools.iter().all(|t| t.name != "structured_output"),
        "the first input's schema does not govern the second"
    );

    let other = json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]});
    let mut third = Admission::text("three", Delivery::Queue);
    third.output_schema = Some(other.clone());
    let third = runtime.admit(&id, third).await.unwrap().message_id;
    tokio::time::timeout(std::time::Duration::from_secs(30), runtime.wait_idle(&id))
        .await
        .unwrap();
    let state = runtime.state(&id).await.unwrap();
    let outcome = state
        .inbox
        .iter()
        .find(|r| r.message_id == third)
        .unwrap()
        .structured
        .clone();
    let outcome = outcome.expect("settled");
    assert_eq!(
        (outcome.value, outcome.attempts),
        (Some(json!({"count": 2})), 1)
    );
}

#[tokio::test]
async fn schemas_are_checked_before_admission() {
    let h = harness(Vec::new());
    let id = h.session().await;
    let refused = |schema: Value| {
        let mut admission = Admission::text("x", Delivery::Queue);
        admission.output_schema = Some(schema);
        let runtime = h.runtime.clone();
        let id = id.clone();
        async move { runtime.admit(&id, admission).await.unwrap_err() }
    };
    for (schema, needle) in [
        (json!({"type": "string"}), "root must be an object schema"),
        (
            json!({"type": "object", "properties": {"a": {"$ref": "https://example.com/s.json"}}}),
            "only local references",
        ),
        (
            json!({"type": "object", "description": "x".repeat(70 * 1024)}),
            "larger than 64 KiB",
        ),
        (
            json!({"type": "object", "minProperties": "many"}),
            "Invalid output_schema",
        ),
    ] {
        let error = refused(schema).await;
        assert!(
            matches!(&error, RuntimeError::Invalid(m) if m.contains(needle)),
            "{needle}: {error:?}"
        );
    }
    assert!(h.state(&id).await.inbox.is_empty(), "nothing was admitted");

    // A retry with the same ID and another schema is another prompt.
    let mut first = Admission::text("x", Delivery::Hold);
    first.message_id = Some("msg_same".into());
    first.output_schema = Some(schema());
    h.runtime.admit(&id, first).await.unwrap();
    let mut changed = Admission::text("x", Delivery::Hold);
    changed.message_id = Some("msg_same".into());
    changed.output_schema = Some(json!({"type": "object"}));
    assert!(h.runtime.admit(&id, changed).await.is_err());
}
