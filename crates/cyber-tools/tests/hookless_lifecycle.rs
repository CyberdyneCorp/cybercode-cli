//! Without hook definitions, lifecycle dispatch leaves no durable trace across many
//! Turns: no SessionStart marker, hook receipt or execution row. Runs on every
//! platform because each durable write is a synchronous commit.
mod support;

use serde_json::json;
use support::flow::{Flow, call, text};

const TURNS: usize = 20;

fn script() -> Vec<Vec<cyber_llm::adapters::ScriptStep>> {
    let mut script: Vec<_> = (0..TURNS)
        .map(|i| {
            call(
                &format!("r{i}"),
                "read",
                json!({"path":"data.txt", "offset":i + 1, "limit":1}),
            )
        })
        .collect();
    script.push(text("done"));
    script
}

/// Event kinds and hook rows written for the store, excluding ordinary Turn records.
fn hook_trace(flow: &Flow) -> (Vec<String>, i64) {
    flow.f
        .store
        .read(|conn| {
            let mut kinds =
                conn.prepare("SELECT DISTINCT type FROM event WHERE type LIKE '%hook%'")?;
            let kinds = kinds
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let executions: i64 =
                conn.query_row("SELECT count(*) FROM hook_execution", [], |row| row.get(0))?;
            Ok((kinds, executions))
        })
        .unwrap()
}

async fn run_root_session(flow: &Flow) {
    flow.f.write("data.txt", &"content\n".repeat(TURNS));
    let id = flow.session("default").await;
    flow.prompt(&id, "inspect").await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests().len(), TURNS + 1);
}

#[tokio::test]
async fn hosts_without_a_hook_resolver_leave_no_lifecycle_trace() {
    let flow = Flow::new(script(), false);
    run_root_session(&flow).await;
    assert_eq!(hook_trace(&flow), (Vec::new(), 0));
}

#[tokio::test]
async fn an_attached_resolver_without_definitions_leaves_no_lifecycle_trace() {
    let flow = Flow::new(script(), false);
    flow.f.global_hooks(json!({}));
    run_root_session(&flow).await;
    assert_eq!(hook_trace(&flow), (Vec::new(), 0));
}
