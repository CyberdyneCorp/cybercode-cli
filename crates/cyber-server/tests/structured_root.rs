//! A root prompt's structured result (`session-runtime` → Structured output, Root prompt
//! structured result lifecycle) through the runtime with a scripted provider: the
//! `structured_output` finalizer, its corrections and limits, and its durable outcome.

mod support;
#[path = "support/structured_terminal_fault.rs"]
mod terminal_fault;

use cyber_server::runtime::{
    Admission, Delivery, Runtime, RuntimeError, SessionState, StructuredOutcome,
};
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
    outcome_from_state(&state, message)
}

fn outcome_from_state(state: &SessionState, message: &str) -> Option<StructuredOutcome> {
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
async fn inspected_request_estimate_includes_the_finalizer_and_every_assembled_input() {
    let h = harness(vec![structured("final", r#"{"summary":"done"}"#)]);
    let id = h.session().await;
    let mut output_schema = schema();
    output_schema["description"] = "Describe the completed audit. ".repeat(200).into();
    ask(&h, &id, "audit this workspace", Some(output_schema)).await;
    h.settle(&id).await;
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.system.len() > 2);
    assert!(
        request
            .tools
            .iter()
            .any(|tool| tool.name == "structured_output")
    );
    let serialized_bytes = request.system.iter().map(String::len).sum::<usize>()
        + serde_json::to_vec(&request.messages).unwrap().len()
        + serde_json::to_vec(&request.tools).unwrap().len();
    let state = h.state(&id).await;
    let estimate = h
        .runtime
        .inspect_context(&state)
        .last_prepared_request
        .unwrap();
    assert_eq!(
        estimate.estimated_input_tokens,
        (serialized_bytes / 4) as u64
    );
    assert_eq!(estimate.step_id, state.steps.last().unwrap().step_id);
    assert_eq!(
        estimate.estimated_input_utilization,
        Some(estimate.estimated_input_tokens as f64 / 200_000.0)
    );
    assert!(
        estimate
            .sources
            .iter()
            .any(|source| source.id == "turn_instructions" && source.estimated_tokens > 0)
    );
    assert!(
        estimate
            .sources
            .iter()
            .any(|source| source.id == "tool_schemas" && source.estimated_tokens > 1000)
    );
    let restarted = h.restart();
    assert_eq!(
        restarted
            .inspect_context(&restarted.state(&id).await.unwrap())
            .last_prepared_request,
        Some(estimate)
    );
    assert_eq!(h.models.requests("test/main").len(), 1);
}

#[tokio::test]
async fn lost_terminal_commit_ack_reopens_the_original_outcome_without_dispatch() {
    use std::sync::atomic::Ordering;
    let h = harness(vec![structured("final", r#"{"summary":"committed"}"#)]);
    let id = h.session().await;
    let mut admission = Admission::text("audit", Delivery::Queue);
    admission.output_schema = Some(schema());
    admission.resume = false;
    let message = h.runtime.admit(&id, admission).await.unwrap().message_id;
    let path = h.dir.path().join("cyber.db");
    let reached = terminal_fault::install(&h.store, &path, &id, &message);
    assert!(!reached.load(Ordering::SeqCst));
    h.runtime.resume(&id).await.unwrap();
    h.settle(&id).await;
    assert!(
        reached.load(Ordering::SeqCst),
        "the terminal outcome must actually commit"
    );
    assert!(
        h.store.has_failed(),
        "lost COMMIT acknowledgement fences the writer"
    );
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert!(
        result_of(&h, &id, &message).await.is_none(),
        "unacknowledged state is not published as success"
    );
    let committed = input_event_seq(&h, &id, "session.structured_output.settled.1", &message);
    tokio::time::timeout(std::time::Duration::from_secs(10), h.runtime.shutdown())
        .await
        .unwrap();
    let (reopened_store, reopened) = reopened_runtime(&h);
    for _ in 0..2 {
        let state = reopened.state(&id).await.unwrap();
        let outcome =
            outcome_from_state(&state, &message).expect("committed outcome survives reopen");
        assert_eq!(outcome.value, Some(json!({"summary":"committed"})));
        assert_eq!((outcome.error, outcome.attempts), (None, 1));
    }
    assert_eq!(
        h.models.requests("test/main").len(),
        1,
        "lookup cannot redispatch"
    );
    assert_eq!(reopened_store.aggregate_seq(&id).unwrap(), Some(committed));
    reopened.shutdown().await;
}

fn reopened_runtime(h: &Harness) -> (std::sync::Arc<cyber_store::Store>, Runtime) {
    let store = std::sync::Arc::new(open_store(&h.dir.path().join("cyber.db")));
    let replayed = runtime(
        &store,
        &h.models,
        &h.tools,
        h.dir.path(),
        cyber_server::runtime::CompactionConfig::default(),
        None,
    );
    (store, replayed)
}

#[tokio::test]
async fn a_lost_steering_promotion_ack_reopens_the_new_owner_without_replaying_the_old() {
    use std::sync::atomic::Ordering;
    let h = harness(vec![
        tools(&[("work", "read", "{}")]),
        structured("final", r#"{"count":2}"#),
    ]);
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Gated);
    let id = h.session().await;
    let started = h.tools.started.notified();
    let first = ask(&h, &id, "first", Some(schema())).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap();
    let other =
        json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]});
    let next = steer(&h, &id, "next", Some(other)).await;
    let reached = terminal_fault::install(&h.store, &h.dir.path().join("cyber.db"), &id, &first);
    h.tools.release.notify_one();
    h.settle(&id).await;
    assert!(reached.load(Ordering::SeqCst));
    assert!(h.store.has_failed());
    assert_eq!(
        h.models.requests("test/main").len(),
        1,
        "failed writer must not admit the next request"
    );
    assert_eq!(h.tools.executed.lock().unwrap().len(), 1);
    assert!(
        input_event_seq(&h, &id, "session.structured_output.settled.1", &first)
            < input_event_seq(&h, &id, "session.prompt.promoted.1", &next)
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), h.runtime.shutdown())
        .await
        .unwrap();
    let (store, replayed) = reopened_runtime(&h);
    let before = replayed.state(&id).await.unwrap();
    let original = outcome_from_state(&before, &first).unwrap();
    assert_superseded(&original, 0, &next);
    assert!(outcome_from_state(&before, &next).is_none());
    replayed.resume(&id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), replayed.wait_idle(&id))
        .await
        .unwrap();
    let after = replayed.state(&id).await.unwrap();
    assert_eq!(outcome_from_state(&after, &first), Some(original));
    let completed = outcome_from_state(&after, &next).unwrap();
    assert_eq!(completed.value, Some(json!({"count":2})));
    assert_eq!((completed.error, completed.attempts), (None, 1));
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(
        h.tools.executed.lock().unwrap().len(),
        1,
        "recovery must not replay the old tool"
    );
    let events = store.read_events(&id, -1, 200).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "session.prompt.promoted.1"
                && event.data["message_id"] == next)
            .count(),
        1
    );
    replayed.shutdown().await;
}

async fn steer(h: &Harness, id: &str, text: &str, format: Option<Value>) -> String {
    let mut admission = Admission::text(text, Delivery::Steer);
    admission.output_schema = format;
    admission.resume = false;
    h.runtime.admit(id, admission).await.unwrap().message_id
}

fn assert_superseded(outcome: &StructuredOutcome, attempts: u32, next: &str) {
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.attempts, attempts);
    assert_eq!(
        outcome.error.as_deref(),
        Some(format!("StructuredOutputError: superseded by input {next}").as_str())
    );
}

fn input_event_seq(h: &Harness, id: &str, kind: &str, message: &str) -> i64 {
    h.store
        .read_events(id, -1, 200)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.kind == kind && event.data["message_id"] == message)
        .unwrap()
        .seq
}

#[tokio::test]
async fn steering_seals_each_displaced_result_before_promoting_its_replacement() {
    let h = harness(vec![
        structured("invalid", r#"{"wrong":1}"#),
        tools(&[("work", "read", "{}")]),
        structured("new", r#"{"count":2}"#),
    ]);
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Gated);
    let id = h.session().await;
    let started = h.tools.started.notified();
    let first = ask(&h, &id, "first", Some(schema())).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap();
    let middle = steer(&h, &id, "middle", Some(schema())).await;
    let other =
        json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]});
    let last = steer(&h, &id, "last", Some(other.clone())).await;
    assert!(
        result_of(&h, &id, &first).await.is_none(),
        "admission alone does not replace an open result"
    );
    h.tools.release.notify_one();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert_superseded(
        &outcome_from_state(&state, &first).expect("first outcome"),
        1,
        &middle,
    );
    assert_superseded(
        &outcome_from_state(&state, &middle).expect("middle outcome"),
        0,
        &last,
    );
    let outcome = outcome_from_state(&state, &last).expect("last outcome");
    assert_eq!(outcome.value, Some(json!({"count":2})));
    assert_eq!((outcome.error, outcome.attempts), (None, 1));
    assert_eq!(h.models.requests("test/main").len(), 3);
    assert!(
        input_event_seq(&h, &id, "session.structured_output.settled.1", &first)
            < input_event_seq(&h, &id, "session.prompt.promoted.1", &middle)
    );
    assert!(
        input_event_seq(&h, &id, "session.structured_output.settled.1", &middle)
            < input_event_seq(&h, &id, "session.prompt.promoted.1", &last)
    );
    let restarted = h.restart();
    let loaded = restarted.state(&id).await.unwrap();
    assert_eq!(
        outcome_from_state(&loaded, &first),
        outcome_from_state(&state, &first)
    );
    assert_eq!(
        outcome_from_state(&loaded, &middle),
        outcome_from_state(&state, &middle)
    );
    assert_eq!(
        h.models
            .requests("test/main")
            .last()
            .unwrap()
            .tools
            .iter()
            .find(|tool| tool.name == "structured_output")
            .unwrap()
            .input_schema,
        other
    );
}

#[tokio::test]
async fn plain_steering_ends_the_prior_schema_without_applying_it_to_the_new_prompt() {
    let h = harness(vec![tools(&[("work", "read", "{}")]), text("plain answer")]);
    h.tools
        .behavior
        .lock()
        .unwrap()
        .insert("read".into(), Behavior::Gated);
    let id = h.session().await;
    let started = h.tools.started.notified();
    let first = ask(&h, &id, "first", Some(schema())).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap();
    let next = steer(&h, &id, "plain", None).await;
    h.tools.release.notify_one();
    h.settle(&id).await;
    assert_superseded(
        &result_of(&h, &id, &first)
            .await
            .expect("superseded outcome"),
        0,
        &next,
    );
    assert!(result_of(&h, &id, &next).await.is_none());
    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .tools
            .iter()
            .all(|tool| tool.name != "structured_output")
    );
}

#[tokio::test]
async fn a_durable_finalizer_reconciles_a_terminal_writer_cut_without_another_request() {
    assert_terminal_writer_cut(true).await;
}

#[tokio::test]
async fn exhausted_submissions_reconcile_a_terminal_writer_cut_without_a_fourth_request() {
    assert_terminal_writer_cut(false).await;
}

fn prevent_terminal_outcome(h: &Harness) {
    h.store.transaction(|db| {
        db.execute_batch("CREATE TRIGGER reject_structured_terminal BEFORE INSERT ON event WHEN NEW.type='session.structured_output.settled.1' BEGIN SELECT RAISE(ABORT,'fixture before structured terminal'); END")?;
        Ok(())
    }).unwrap();
}

fn allow_terminal_outcome(h: &Harness) {
    h.store
        .transaction(|db| {
            db.execute_batch("DROP TRIGGER reject_structured_terminal")?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn writer_cut_recovery_settles_the_previous_result_before_promoting_a_new_schema() {
    let h = harness(vec![
        structured("old", r#"{"summary":"original"}"#),
        structured("new", r#"{"count":2}"#),
    ]);
    prevent_terminal_outcome(&h);
    let id = h.session().await;
    let first = ask(&h, &id, "first", Some(schema())).await;
    h.settle(&id).await;
    assert!(result_of(&h, &id, &first).await.is_none());
    assert_eq!(h.models.requests("test/main").len(), 1);
    h.runtime.shutdown().await;
    allow_terminal_outcome(&h);
    let restarted = h.restart();
    let other =
        json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]});
    let mut next = Admission::text("second", Delivery::Queue);
    next.output_schema = Some(other.clone());
    next.resume = false;
    let second = restarted.admit(&id, next).await.unwrap().message_id;
    assert_eq!(h.models.requests("test/main").len(), 1);
    restarted.resume(&id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), restarted.wait_idle(&id))
        .await
        .unwrap();
    let state = restarted.state(&id).await.unwrap();
    let old = outcome_from_state(&state, &first).unwrap();
    let new = outcome_from_state(&state, &second).unwrap();
    assert_eq!(old.value, Some(json!({"summary":"original"})));
    assert_eq!(new.value, Some(json!({"count":2})));
    assert_eq!((old.attempts, new.attempts), (1, 1));
    assert_eq!((&old.error, &new.error), (&None, &None));
    let requests = h.models.requests("test/main");
    assert_eq!(
        requests.len(),
        2,
        "only the new prompt dispatches after recovery"
    );
    assert_eq!(
        requests[1]
            .tools
            .iter()
            .find(|tool| tool.name == "structured_output")
            .unwrap()
            .input_schema,
        other
    );
    let events = h.store.read_events(&id, -1, 200).unwrap().events;
    let settled = events
        .iter()
        .find(|event| {
            event.kind == "session.structured_output.settled.1" && event.data["message_id"] == first
        })
        .unwrap();
    let promoted = events
        .iter()
        .find(|event| {
            event.kind == "session.prompt.promoted.1" && event.data["message_id"] == second
        })
        .unwrap();
    assert!(settled.seq < promoted.seq);
    restarted.shutdown().await;
}

async fn prepare_terminal_writer_cut(valid: bool) -> (Harness, String, String, usize) {
    let mut script = if valid {
        vec![structured("valid", r#"{"summary":"durable value"}"#)]
    } else {
        vec![
            structured("bad1", r#"{"wrong":1}"#),
            structured("bad2", r#"{"summary":3}"#),
            structured("bad3", "{}"),
        ]
    };
    script.push(text("fresh prompt answer"));
    let h = harness(script);
    prevent_terminal_outcome(&h);
    let id = h.session().await;
    let message = ask(&h, &id, "audit", Some(schema())).await;
    h.settle(&id).await;
    let attempts = if valid { 1 } else { 3 };
    assert_eq!(h.models.requests("test/main").len(), attempts);
    assert!(result_of(&h, &id, &message).await.is_none());
    let state = h.state(&id).await;
    if valid {
        assert_eq!(
            state.structured_result(),
            Some(&json!({"summary":"durable value"}))
        );
    } else {
        assert!(state.structured_result_error().contains("Invalid result"));
    }
    h.runtime.shutdown().await;
    allow_terminal_outcome(&h);
    (h, id, message, attempts)
}

async fn assert_terminal_writer_cut(valid: bool) {
    let (h, id, message, attempts) = prepare_terminal_writer_cut(valid).await;
    let restarted = h.restart();
    restarted.resume(&id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), restarted.wait_idle(&id))
        .await
        .unwrap();
    assert_eq!(
        h.models.requests("test/main").len(),
        attempts,
        "metadata recovery must not reopen inference"
    );
    let state = restarted.state(&id).await.unwrap();
    let outcome = outcome_from_state(&state, &message).expect("reconciled result");
    assert_reconciled_outcome(&outcome, valid, attempts);
    let events = h.store.read_events(&id, -1, 200).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "session.structured_output.settled.1")
            .count(),
        1
    );
    assert_fresh_plain_prompt(&h, &restarted, &id, &message, &outcome, attempts).await;
    restarted.shutdown().await;
}

fn assert_reconciled_outcome(outcome: &StructuredOutcome, valid: bool, attempts: usize) {
    assert_eq!(outcome.attempts as usize, attempts);
    if valid {
        assert_eq!(outcome.value, Some(json!({"summary":"durable value"})));
        assert_eq!(outcome.error, None);
    } else {
        assert_eq!(outcome.value, None);
        assert!(outcome.error.as_deref().unwrap().contains("Invalid result"));
    }
}

async fn assert_fresh_plain_prompt(
    h: &Harness,
    restarted: &Runtime,
    id: &str,
    message: &str,
    outcome: &StructuredOutcome,
    attempts: usize,
) {
    restarted
        .admit(id, Admission::text("new plain prompt", Delivery::Queue))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), restarted.wait_idle(id))
        .await
        .unwrap();
    assert_eq!(h.models.requests("test/main").len(), attempts + 1);
    assert!(
        h.models
            .requests("test/main")
            .last()
            .unwrap()
            .tools
            .iter()
            .all(|tool| tool.name != "structured_output")
    );
    let state = restarted.state(id).await.unwrap();
    assert_eq!(outcome_from_state(&state, message).as_ref(), Some(outcome));
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
